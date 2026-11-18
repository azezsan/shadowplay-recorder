# NVIDIA overlay record command

How the app starts and stops a ShadowPlay recording. NVIDIA does not document
any of this. It was read out of the NVIDIA App's own JavaScript and confirmed
by hooking the overlay's `cefQuery` calls while clicking its Record button
(the Python version of this project did that with a `--discover-ipc` tool).
The code is `crates/core/src/overlay.rs` and `overlay_setup.rs`.

## What the project relies on

- **The overlay is a CEF (Chromium) page.** Its switches are read at launch from
  `%ProgramFiles%\NVIDIA Corporation\NVIDIA App\CEF\Resources\NVIDIA Overlay.json`,
  a JSON object with a `"switches"` list of strings.
- **`nv-remote-debugging-port=<port>` in that list** opens the Chrome DevTools
  Protocol on `127.0.0.1:<port>`. The app uses port `49222`. The NVIDIA App must
  be fully quit from its tray icon and restarted for the switch to take effect.
  An NVIDIA App update can reset the file.
- **`GET http://127.0.0.1:49222/json`** lists the debuggable targets. Each has a
  `url` and usually a `webSocketDebuggerUrl`. The target `type` values are not
  reliable. The overlay page's `url` contains `osc`.
- **The WebSocket upgrade must carry no `Origin` header.** Chromium 111 and
  later answers 403 to an Origin that is not allow-listed, and accepts a missing
  one.
- **A usable page exposes `window.cefQuery`.** `Runtime.evaluate` of
  `typeof window.cefQuery` returns `"function"` on it.
- **The record command** is a `cefQuery` whose `request` is this JSON, as a
  string:

  ```json
  {
    "command": "QUERY_IPC_EXTENSION_MESSAGE",
    "system": "CrimsonNative",
    "module": "ShareServer",
    "method": "EnableRecord",
    "payload": { "enable": true }
  }
  ```

  `enable: true` starts a manual recording and `enable: false` stops and saves
  it. It is the same call the overlay's Record button makes. `ShareServer` also
  has `StartDiskCapture` and `StopAndSaveDiskCapture`, which need a fuller
  payload and are not used.

- **The reply is not proof.** The `onSuccess` callback sometimes never fires even
  though the recording started. The app therefore confirms a start by watching
  for a new video file, and treats a missing reply as "sent".
- **ShadowPlay records the game it considers active**, so the game window is
  brought to the foreground before each start command.

## Limits and risks

- The debug port is unauthenticated. It is bound to loopback, so other machines
  cannot reach it, but any program on the same PC can run code in the overlay
  page through it.
- A future NVIDIA App can rename the command, module or switch. The names are
  constants in `overlay.rs`; fixing a rename means shipping an update.
- Whether `EnableRecord` with `enable: true` is ignored while a recording is
  already running is not known. The start interval is kept longer than
  ShadowPlay's file-creation latency so a retry never lands on a recording that
  just started.

## Sources

- Chrome DevTools Protocol, `Runtime.evaluate`: https://chromedevtools.github.io/devtools-protocol/tot/Runtime/#method-evaluate
- CEF message router (`window.cefQuery`): https://bitbucket.org/chromiumembedded/cef/src/master/include/wrapper/cef_message_router.h
- Chromium remote-debugging origin check: https://chromium.googlesource.com/chromium/src/+/main/content/browser/devtools/devtools_http_handler.cc
