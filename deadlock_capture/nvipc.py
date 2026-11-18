"""Programmatic control of NVIDIA ShadowPlay via the NVIDIA App's own IPC.

Why this exists
---------------
The old approach injects the record *hotkey* (F10) and hopes ShadowPlay sees it.
That's fragile: keystrokes get dropped during the fullscreen load, so we had to
retry, gate on focus, and confirm by watching for a file. This module replaces
the keystroke with the *real* control channel ShadowPlay uses internally.

How it works
------------
The modern NVIDIA App is a CEF (Chromium) app. Its overlay starts/stops a
recording by calling ``window.cefQuery(...)`` with a ``QUERY_IPC_EXTENSION_MESSAGE``
that routes to the native ``ShareServer`` module over NVIDIA's MessageBus — the
same bus the F10 press ends up on. We reach that channel from outside the app by:

1. Enabling CEF remote debugging on the overlay (a one-time edit to
   ``NVIDIA Overlay.json`` — see README, "Programmatic control (IPC backend)").
2. Connecting to that debug port over the Chrome DevTools Protocol (CDP).
3. Using ``Runtime.evaluate`` to run ``window.cefQuery(...)`` in the overlay's
   page, issuing the ShadowPlay start/stop command and awaiting its reply.

The method names (``StartDiskCapture`` / ``StopAndSaveDiskCapture``), the module
(``ShareServer``) and the wrapper (``QUERY_IPC_EXTENSION_MESSAGE``) were taken
from the NVIDIA App's own JavaScript; they live in config.toml so they can be
retuned (or rediscovered with ``--discover-ipc``) if a future App renames them.

Everything here is loopback-only and best-effort: any failure raises an
``IpcError`` subclass so the caller can fall back to the hotkey.
"""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from typing import Any

from . import log

try:  # optional dep: the daemon still runs (hotkey backend) without it
    import websocket  # from the "websocket-client" package
except ImportError:  # pragma: no cover - exercised only where the dep is absent
    websocket = None  # type: ignore[assignment]


IPC_COMMAND = "QUERY_IPC_EXTENSION_MESSAGE"


# ── Errors ────────────────────────────────────────────────────────────────────


class IpcError(Exception):
    """Base for anything that goes wrong talking to the overlay."""


class IpcUnavailable(IpcError):
    """Can't reach the overlay at all (port closed, overlay down, no cefQuery,
    or the websocket-client dep is missing). Caller may fall back to the hotkey."""


class IpcNoAck(IpcError):
    """The command was sent but the overlay never acked (the cefQuery callback
    didn't fire before the timeout). It very likely *did* take effect, so the
    caller should treat this as 'sent' — NOT retry via the hotkey, or it would
    risk toggling the recording back off."""


# ── Low-level CDP helpers ─────────────────────────────────────────────────────


def _http_get_json(url: str, timeout: float) -> Any:
    try:
        with urllib.request.urlopen(url, timeout=timeout) as resp:  # noqa: S310 - loopback only
            return json.loads(resp.read().decode("utf-8", errors="replace"))
    except (urllib.error.URLError, OSError) as err:
        raise IpcUnavailable(f"cannot reach CDP endpoint {url}: {err}") from err
    except ValueError as err:
        raise IpcUnavailable(f"CDP endpoint {url} returned invalid JSON: {err}") from err


def _cefquery_expression(request_str: str) -> str:
    """JS that runs one cefQuery and resolves to {ok, response} / {ok, code, message}.

    ``request_str`` is the exact string handed to cefQuery's ``request`` field
    (ShadowPlay expects a JSON *string*). We json.dumps it again to embed it as a
    safe JS string literal.
    """
    req_literal = json.dumps(request_str)
    return (
        "new Promise(function(resolve){try{"
        "window.cefQuery({request:" + req_literal + ",persistent:false,"
        "onSuccess:function(response){resolve({ok:true,response:response});},"
        "onFailure:function(code,message){resolve({ok:false,code:code,message:message});}"
        "});}catch(e){resolve({ok:false,code:-1,message:String(e)});}})"
    )


def _share_request_str(system: str, module: str, method: str, payload: dict) -> str:
    return json.dumps(
        {
            "command": IPC_COMMAND,
            "system": system,
            "module": module,
            "method": method,
            "payload": payload or {},
        }
    )


class NvOverlayIpc:
    """Thin CDP client for the NVIDIA overlay. Opens a fresh websocket per call
    (start/stop happen only a couple of times per match, so connection reuse
    isn't worth the stale-socket risk)."""

    def __init__(
        self,
        host: str = "127.0.0.1",
        port: int = 49222,
        connect_timeout_s: float = 2.0,
        call_timeout_s: float = 5.0,
    ) -> None:
        self.host = host
        self.port = port
        self.connect_timeout_s = connect_timeout_s
        self.call_timeout_s = call_timeout_s
        self._cached_ws_url: str | None = None  # last target known to have cefQuery

    # ── target discovery ────────────────────────────────────────────────────

    @property
    def _base(self) -> str:
        return f"http://{self.host}:{self.port}"

    def list_targets(self) -> list[dict]:
        data = _http_get_json(f"{self._base}/json", self.connect_timeout_s)
        return data if isinstance(data, list) else []

    def _evaluate(self, ws_url: str, expression: str, await_promise: bool) -> Any:
        """Run one Runtime.evaluate over a fresh websocket; return the JS value."""
        if websocket is None:
            raise IpcUnavailable(
                "the 'websocket-client' package is not installed (run: uv sync)"
            )
        try:
            # suppress_origin: Chromium's DevTools endpoint (Chrome 111+) rejects
            # WebSocket upgrades whose Origin header isn't allow-listed, with a
            # 403. Sending *no* Origin (like Puppeteer/Playwright do) is always
            # allowed, so we don't have to add --remote-allow-origins to the App.
            ws = websocket.create_connection(  # type: ignore[union-attr]
                ws_url, timeout=self.call_timeout_s, suppress_origin=True
            )
        except Exception as err:  # noqa: BLE001 - any connect failure is "unavailable"
            self._cached_ws_url = None
            raise IpcUnavailable(f"cannot open CDP websocket: {err}") from err

        try:
            ws.send(
                json.dumps(
                    {
                        "id": 1,
                        "method": "Runtime.evaluate",
                        "params": {
                            "expression": expression,
                            "awaitPromise": await_promise,
                            "returnByValue": True,
                            # Longer than our socket read (below) on purpose: a
                            # command that never calls back should surface as a
                            # recv timeout -> IpcNoAck ("sent"), not a CDP
                            # "evaluation timed out" error that would trigger a
                            # needless hotkey fallback and risk a double-trigger.
                            "timeout": int((self.call_timeout_s + 2.0) * 1000),
                        },
                    }
                )
            )
            while True:  # skip any stray events; ours is id==1
                try:
                    raw = ws.recv()
                except Exception as err:  # noqa: BLE001 - recv timeout etc.
                    raise IpcNoAck(f"no CDP reply within {self.call_timeout_s:g}s: {err}") from err
                if not raw:
                    raise IpcNoAck("CDP websocket closed before replying")
                msg = json.loads(raw)
                if msg.get("id") == 1:
                    break
        finally:
            try:
                ws.close()
            except Exception:  # noqa: BLE001
                pass

        if "error" in msg:
            raise IpcError(f"CDP protocol error: {msg['error']}")
        result = msg.get("result", {})
        if "exceptionDetails" in result:
            exc = result["exceptionDetails"]
            desc = exc.get("exception", {}).get("description") or exc.get("text")
            raise IpcError(f"JS exception in overlay: {desc}")
        return result.get("result", {}).get("value")

    def _target_has_cefquery(self, ws_url: str) -> bool:
        try:
            return self._evaluate(ws_url, "typeof window.cefQuery", await_promise=False) == "function"
        except IpcError:
            return False

    def find_cefquery_target(self, force: bool = False) -> str:
        """Return the websocket URL of an overlay page exposing window.cefQuery.

        Prefers the overlay ('osc') page, then any page. Caches the result."""
        if self._cached_ws_url and not force:
            return self._cached_ws_url

        targets = self.list_targets()
        # Any target with a debugger URL is fair game — NVIDIA's target `type`
        # values aren't reliable, so we probe rather than filter by type.
        pages = [t for t in targets if t.get("webSocketDebuggerUrl")]
        # Overlay ('osc') page first — that's where the record controls live.
        pages.sort(key=lambda t: 0 if "osc" in (t.get("url", "").lower()) else 1)
        for t in pages:
            ws_url = t["webSocketDebuggerUrl"]
            if self._target_has_cefquery(ws_url):
                self._cached_ws_url = ws_url
                return ws_url
        raise IpcUnavailable(
            "connected to the CEF debug port but found no page exposing "
            "window.cefQuery (is the NVIDIA overlay running?)"
        )

    # ── the actual ShadowPlay command ───────────────────────────────────────

    def send_share_command(
        self, method: str, payload: dict, system: str, module: str
    ) -> dict:
        """Send one ShareServer command and return the overlay's reply dict.

        Raises IpcUnavailable if the overlay can't be reached, IpcNoAck if it was
        sent but not acked, IpcError if the overlay rejected it (ok:false)."""
        request_str = _share_request_str(system, module, method, payload)
        expr = _cefquery_expression(request_str)

        ws_url = self.find_cefquery_target()
        try:
            value = self._evaluate(ws_url, expr, await_promise=True)
        except IpcUnavailable:
            # Cached target may be stale (overlay reloaded); rediscover once.
            ws_url = self.find_cefquery_target(force=True)
            value = self._evaluate(ws_url, expr, await_promise=True)

        if value is None:
            raise IpcNoAck(f"{method}: overlay returned no result")
        if isinstance(value, dict) and value.get("ok") is False:
            raise IpcError(
                f"{method} rejected by overlay: code={value.get('code')} "
                f"msg={value.get('message')}"
            )
        return value if isinstance(value, dict) else {"ok": True, "response": value}

    # ── diagnostics (used by the CLI; safe to run anytime) ───────────────────

    def status(self) -> dict:
        """Best-effort snapshot for --ipc-status: lists every CEF target and
        whether each exposes window.cefQuery, so a missing overlay page is easy
        to spot."""
        info: dict[str, Any] = {"endpoint": f"{self._base}/json", "reachable": False}
        try:
            targets = self.list_targets()
        except IpcUnavailable as err:
            info["error"] = str(err)
            return info
        info["reachable"] = True
        info["target_count"] = len(targets)

        rows: list[dict] = []
        chosen: str | None = None
        for t in targets:
            ws = t.get("webSocketDebuggerUrl")
            cq: Any = None
            err_text: str | None = None
            if ws:
                try:
                    cq = (
                        self._evaluate(ws, "typeof window.cefQuery", await_promise=False)
                        == "function"
                    )
                except IpcError as err:
                    cq = "err"
                    err_text = f"{type(err).__name__}: {err}"
                if cq is True and chosen is None:
                    chosen = ws
            rows.append(
                {
                    "type": t.get("type"),
                    "title": (t.get("title") or "")[:40],
                    "url": t.get("url"),
                    "has_ws": bool(ws),
                    "cefquery": cq,
                    "error": err_text,
                }
            )
        info["targets"] = rows
        info["ready"] = chosen is not None
        if chosen:
            info["cefquery_ws"] = chosen
            self._cached_ws_url = chosen
        return info

    def install_query_logger(self) -> str:
        """Wrap window.cefQuery so subsequent calls are recorded (for discovery)."""
        ws_url = self.find_cefquery_target(force=True)
        js = (
            "(function(){if(window.__nv_ipc_log)return 'already-installed';"
            "window.__nv_ipc_log=[];var orig=window.cefQuery;"
            "window.cefQuery=function(arg){try{window.__nv_ipc_log.push("
            "arg&&arg.request!==undefined?arg.request:JSON.stringify(arg));}catch(e){}"
            "return orig.apply(this,arguments);};return 'installed';})()"
        )
        return str(self._evaluate(ws_url, js, await_promise=False))

    def read_query_log(self) -> list[str]:
        ws_url = self.find_cefquery_target()
        val = self._evaluate(
            ws_url, "JSON.stringify(window.__nv_ipc_log||[])", await_promise=False
        )
        try:
            return list(json.loads(val)) if val else []
        except (ValueError, TypeError):
            return []
