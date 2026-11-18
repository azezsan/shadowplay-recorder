//! Sends ShadowPlay's own record command through the NVIDIA overlay.
//!
//! The NVIDIA App's overlay is a Chromium (CEF) page. Its Record button calls
//! `window.cefQuery(...)` with a message for the native `ShareServer` module.
//! With the overlay's debug port open (see `overlay_setup`), this module
//! connects over the Chrome DevTools Protocol and makes that same call, so no
//! keypress is simulated and the game does not need keyboard focus.
//!
//! `docs/apis/nvidia-overlay.md` records the protocol details and where they
//! came from. The names below must change if a future NVIDIA App renames them.

use std::fmt;
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::{Value, json};
use tungstenite::Message;

pub const DEBUG_HOST: &str = "127.0.0.1";
pub const DEBUG_PORT: u16 = 49222;

const WRAPPER_COMMAND: &str = "QUERY_IPC_EXTENSION_MESSAGE";
const SYSTEM: &str = "CrimsonNative";
const MODULE: &str = "ShareServer";
const RECORD_METHOD: &str = "EnableRecord";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// The overlay's manual-record toggle: start, or stop and save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordCommand {
    Start,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayError {
    /// The overlay could not be reached, so nothing was sent.
    Unavailable(String),
    /// The command was sent but no reply came back. It very likely took
    /// effect, so callers treat it as sent and let the recording file confirm.
    NoAck(String),
    /// The overlay answered and refused the command.
    Rejected(String),
}

impl fmt::Display for OverlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "overlay unavailable: {m}"),
            Self::NoAck(m) => write!(f, "no reply from the overlay: {m}"),
            Self::Rejected(m) => write!(f, "overlay rejected the command: {m}"),
        }
    }
}

impl std::error::Error for OverlayError {}

/// What a probe of the debug port found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// A page exposing `window.cefQuery` answered; commands will work.
    Ready,
    /// The port answered but no page can take the record command yet.
    NoRecordPage,
    /// Nothing is listening on the debug port.
    Unreachable(String),
}

/// Anything that can carry a record command to ShadowPlay.
pub trait RecordChannel {
    fn send(&mut self, command: RecordCommand) -> Result<(), OverlayError>;
}

/// Client for the overlay's debug port. Opens a fresh connection per call:
/// commands are rare, and a kept socket goes stale when the overlay reloads.
pub struct Overlay {
    host: String,
    port: u16,
    connect_timeout: Duration,
    call_timeout: Duration,
    /// Last page known to expose `window.cefQuery`.
    record_page: Option<String>,
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new(DEBUG_HOST, DEBUG_PORT)
    }
}

impl Overlay {
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            host: host.to_string(),
            port,
            connect_timeout: CONNECT_TIMEOUT,
            call_timeout: CALL_TIMEOUT,
            record_page: None,
        }
    }

    pub fn with_call_timeout(mut self, timeout: Duration) -> Self {
        self.call_timeout = timeout;
        self
    }

    pub fn probe(&mut self) -> Probe {
        if let Err(e) = self.list_pages() {
            return Probe::Unreachable(e.to_string());
        }
        match self.find_record_page(true) {
            Ok(_) => Probe::Ready,
            Err(_) => Probe::NoRecordPage,
        }
    }

    /// WebSocket URLs of every debuggable page, the overlay's own page first.
    fn list_pages(&self) -> Result<Vec<String>, OverlayError> {
        let url = format!("http://{}:{}/json", self.host, self.port);
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(self.connect_timeout))
            .build()
            .into();
        let body = agent
            .get(&url)
            .call()
            .and_then(|mut r| r.body_mut().read_to_string())
            .map_err(|e| OverlayError::Unavailable(format!("cannot reach {url}: {e}")))?;
        let targets: Value = serde_json::from_str(&body)
            .map_err(|e| OverlayError::Unavailable(format!("{url} returned invalid JSON: {e}")))?;

        // NVIDIA's target `type` values are unreliable, so every target with a
        // debugger URL is probed. The overlay ("osc") page holds the record
        // controls and is tried first.
        let mut pages: Vec<(bool, String)> = targets
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| {
                let ws = t.get("webSocketDebuggerUrl")?.as_str()?.to_string();
                let is_overlay = t
                    .get("url")
                    .and_then(Value::as_str)
                    .is_some_and(|u| u.to_lowercase().contains("osc"));
                Some((!is_overlay, ws))
            })
            .collect();
        pages.sort_by_key(|(not_overlay, _)| *not_overlay);
        Ok(pages.into_iter().map(|(_, ws)| ws).collect())
    }

    fn find_record_page(&mut self, force: bool) -> Result<String, OverlayError> {
        if !force && let Some(page) = &self.record_page {
            return Ok(page.clone());
        }
        self.record_page = None;
        for page in self.list_pages()? {
            let has_cef_query = self
                .evaluate(&page, "typeof window.cefQuery", false)
                .is_ok_and(|v| v == "function");
            if has_cef_query {
                self.record_page = Some(page.clone());
                return Ok(page);
            }
        }
        Err(OverlayError::Unavailable(
            "the debug port answered but no page exposes window.cefQuery".into(),
        ))
    }

    /// Runs one `Runtime.evaluate` on a page and returns the JavaScript value.
    fn evaluate(
        &self,
        ws_url: &str,
        expression: &str,
        await_promise: bool,
    ) -> Result<Value, OverlayError> {
        let unavailable =
            |e: String| OverlayError::Unavailable(format!("cannot open {ws_url}: {e}"));
        let authority = ws_url
            .strip_prefix("ws://")
            .and_then(|rest| rest.split('/').next())
            .ok_or_else(|| unavailable("not a ws:// URL".to_string()))?;
        let addr: SocketAddr = authority.parse().map_err(|e| unavailable(format!("{e}")))?;
        let stream = TcpStream::connect_timeout(&addr, self.connect_timeout)
            .map_err(|e| unavailable(e.to_string()))?;
        stream
            .set_read_timeout(Some(self.call_timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.call_timeout)))
            .map_err(|e| unavailable(e.to_string()))?;

        // tungstenite sends no Origin header. Chromium rejects DevTools
        // upgrades whose Origin is not allow-listed, and accepts none at all.
        let (mut socket, _) =
            tungstenite::client(ws_url, stream).map_err(|e| unavailable(e.to_string()))?;

        let request = json!({
            "id": 1,
            "method": "Runtime.evaluate",
            "params": {
                "expression": expression,
                "awaitPromise": await_promise,
                "returnByValue": true,
                // Longer than the socket read timeout on purpose: a command
                // that never calls back must surface as NoAck ("sent"), not as
                // a DevTools evaluation timeout that would look like a failure.
                "timeout": (self.call_timeout + Duration::from_secs(2)).as_millis() as u64,
            },
        });
        socket
            .send(Message::text(request.to_string()))
            .map_err(|e| unavailable(e.to_string()))?;

        let reply = loop {
            let message = socket.read().map_err(|e| {
                OverlayError::NoAck(format!("no reply within {:?}: {e}", self.call_timeout))
            })?;
            let Message::Text(text) = message else {
                continue;
            };
            match serde_json::from_str::<Value>(text.as_str()) {
                Ok(v) if v.get("id") == Some(&json!(1)) => break v,
                _ => {}
            }
        };
        let _ = socket.close(None);

        if let Some(error) = reply.get("error") {
            return Err(OverlayError::Rejected(format!("DevTools error: {error}")));
        }
        let result = &reply["result"];
        if let Some(details) = result.get("exceptionDetails") {
            let description = details["exception"]["description"]
                .as_str()
                .or(details["text"].as_str())
                .unwrap_or("unknown");
            return Err(OverlayError::Rejected(format!(
                "JavaScript exception in the overlay: {description}"
            )));
        }
        Ok(result["result"]["value"].clone())
    }
}

impl RecordChannel for Overlay {
    fn send(&mut self, command: RecordCommand) -> Result<(), OverlayError> {
        let expression = cef_query_expression(&record_request(command));
        let page = self.find_record_page(false)?;
        let value = match self.evaluate(&page, &expression, true) {
            // The remembered page may be stale after an overlay reload.
            Err(OverlayError::Unavailable(_)) => {
                let page = self.find_record_page(true)?;
                self.evaluate(&page, &expression, true)?
            }
            other => other?,
        };

        if value.is_null() {
            return Err(OverlayError::NoAck("overlay returned no result".into()));
        }
        if value.get("ok") == Some(&Value::Bool(false)) {
            return Err(OverlayError::Rejected(format!(
                "code={} message={}",
                value["code"], value["message"]
            )));
        }
        Ok(())
    }
}

/// The JSON string the overlay hands to `cefQuery` for a record command.
fn record_request(command: RecordCommand) -> String {
    json!({
        "command": WRAPPER_COMMAND,
        "system": SYSTEM,
        "module": MODULE,
        "method": RECORD_METHOD,
        "payload": { "enable": command == RecordCommand::Start },
    })
    .to_string()
}

/// JavaScript that runs one `cefQuery` and resolves to `{ok, response}` or
/// `{ok: false, code, message}`.
fn cef_query_expression(request: &str) -> String {
    // The request is itself a JSON string; encoding it again yields a safe
    // JavaScript string literal.
    let literal = Value::String(request.to_string()).to_string();
    format!(
        "new Promise(function(resolve){{try{{window.cefQuery({{request:{literal},persistent:false,\
         onSuccess:function(response){{resolve({{ok:true,response:response}});}},\
         onFailure:function(code,message){{resolve({{ok:false,code:code,message:message}});}}\
         }});}}catch(e){{resolve({{ok:false,code:-1,message:String(e)}});}}}})"
    )
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    use super::*;

    #[test]
    fn record_request_matches_the_overlays_own_call() {
        let request: Value = serde_json::from_str(&record_request(RecordCommand::Start)).unwrap();
        assert_eq!(
            request,
            json!({
                "command": "QUERY_IPC_EXTENSION_MESSAGE",
                "system": "CrimsonNative",
                "module": "ShareServer",
                "method": "EnableRecord",
                "payload": { "enable": true },
            })
        );
        let stop: Value = serde_json::from_str(&record_request(RecordCommand::Stop)).unwrap();
        assert_eq!(stop["payload"], json!({ "enable": false }));
    }

    #[test]
    fn expression_embeds_the_request_as_a_string_literal() {
        let request = record_request(RecordCommand::Start);
        let expression = cef_query_expression(&request);
        assert!(expression.contains("window.cefQuery"));
        assert!(expression.contains("onSuccess") && expression.contains("onFailure"));
        assert!(expression.contains(&Value::String(request).to_string()));
    }

    /// How the fake overlay page answers an awaited `cefQuery` call.
    #[derive(Clone, Copy)]
    enum Reply {
        Ok,
        Rejected,
        Silent,
    }

    /// A stand-in for the overlay's debug port: serves `/json` over HTTP and
    /// answers `Runtime.evaluate` over WebSocket on the same listener.
    /// Returns the port and a channel of every expression it evaluated.
    fn fake_overlay(has_cef_query: bool, reply: Reply) -> (u16, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (seen, expressions) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut peek = [0u8; 16];
                let n = stream.peek(&mut peek).unwrap_or(0);
                if peek[..n].starts_with(b"GET /json") {
                    let mut request = [0u8; 1024];
                    let _ = stream.read(&mut request);
                    let body = json!([
                        { "url": "https://app/main", "webSocketDebuggerUrl":
                            format!("ws://127.0.0.1:{port}/devtools/page/main") },
                        { "url": "https://app/OSC/index.html", "webSocketDebuggerUrl":
                            format!("ws://127.0.0.1:{port}/devtools/page/osc") },
                        { "url": "https://app/no-debugger" },
                    ])
                    .to_string();
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    continue;
                }
                let seen = seen.clone();
                thread::spawn(move || {
                    let Ok(mut socket) = tungstenite::accept(stream) else {
                        return;
                    };
                    let Ok(Message::Text(text)) = socket.read() else {
                        return;
                    };
                    let request: Value = serde_json::from_str(text.as_str()).unwrap();
                    let expression = request["params"]["expression"].as_str().unwrap();
                    let _ = seen.send(expression.to_string());
                    let value = if expression == "typeof window.cefQuery" {
                        json!(if has_cef_query {
                            "function"
                        } else {
                            "undefined"
                        })
                    } else {
                        match reply {
                            Reply::Ok => json!({ "ok": true, "response": "" }),
                            Reply::Rejected => json!({ "ok": false, "code": 7, "message": "busy" }),
                            Reply::Silent => {
                                thread::sleep(Duration::from_millis(600));
                                return;
                            }
                        }
                    };
                    // A stray event first: the client must skip it.
                    let _ = socket.send(Message::text(
                        json!({ "method": "Runtime.consoleAPICalled" }).to_string(),
                    ));
                    let _ = socket.send(Message::text(
                        json!({ "id": 1, "result": { "result": { "value": value } } }).to_string(),
                    ));
                    let _ = socket.read();
                });
            }
        });
        (port, expressions)
    }

    #[test]
    fn sends_the_record_command_to_the_overlay_page() {
        let (port, expressions) = fake_overlay(true, Reply::Ok);
        let mut overlay = Overlay::new("127.0.0.1", port);
        assert_eq!(overlay.probe(), Probe::Ready);
        assert_eq!(
            overlay.record_page.as_deref(),
            Some(format!("ws://127.0.0.1:{port}/devtools/page/osc").as_str()),
            "the overlay page is preferred over other pages"
        );

        overlay.send(RecordCommand::Start).unwrap();
        let sent: Vec<String> = expressions.try_iter().collect();
        assert_eq!(
            sent.last().unwrap(),
            &cef_query_expression(&record_request(RecordCommand::Start))
        );
    }

    #[test]
    fn a_refusal_is_rejected_and_silence_is_no_ack() {
        let (port, _) = fake_overlay(true, Reply::Rejected);
        let rejected = Overlay::new("127.0.0.1", port).send(RecordCommand::Stop);
        assert!(matches!(rejected, Err(OverlayError::Rejected(m)) if m.contains("busy")));

        let (port, _) = fake_overlay(true, Reply::Silent);
        let silent = Overlay::new("127.0.0.1", port)
            .with_call_timeout(Duration::from_millis(200))
            .send(RecordCommand::Start);
        assert!(matches!(silent, Err(OverlayError::NoAck(_))));
    }

    #[test]
    fn probe_tells_a_closed_port_from_a_missing_page() {
        let (port, _) = fake_overlay(false, Reply::Ok);
        assert_eq!(Overlay::new("127.0.0.1", port).probe(), Probe::NoRecordPage);

        let closed = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        assert!(matches!(
            Overlay::new("127.0.0.1", port).probe(),
            Probe::Unreachable(_)
        ));
        assert!(matches!(
            Overlay::new("127.0.0.1", port).send(RecordCommand::Start),
            Err(OverlayError::Unavailable(_))
        ));
    }
}
