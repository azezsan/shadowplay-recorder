# Deadlock game-capture

A tiny background daemon. Its main job: when you enter a real Deadlock match it
starts an NVIDIA ShadowPlay recording; when the match ends it stops and saves
the video, named after the match id (e.g. `81038409.mp4`). It also keeps the
recordings folder under a size cap by deleting the oldest videos.

Because it already knows when you're in a game, it also doubles as a **break
reminder**: a periodic notification *sound* (your cue to stretch, look away,
drink water) that is suppressed while the game is running. See
[Reminders](#reminders) below.

## How it works

- Watches for `deadlock.exe` and tails the game's Source 2 `console.log`.
- Detects match start / map / match end from log lines.
- Recording is gated on the map — only maps in `recordable_maps` trigger a
  recording, so bot/sandbox/custom matches are ignored.
- When a match starts, the game window is pulled to the foreground (you're
  usually alt-tabbed while queueing) — and again before *every* record-start
  attempt while the recording is still unconfirmed, since ShadowPlay only
  records the active game and you may have alt-tabbed away again. Turn it off
  with `focus_on_match_start = false`. Best-effort: if Windows refuses the
  foreground switch it's logged and the attempt is sent anyway.
- Recording is driven **programmatically** through the NVIDIA App's IPC — the
  daemon sends ShadowPlay's *own* start/stop command (see [Programmatic
  control](#programmatic-control)), never a simulated keypress — and confirms it
  by watching for a new video file to appear.

### Reliable start/stop

- **Confirmed by file, not by hope.** After a recordable map loads the daemon
  sends the record command, then re-sends every `start_attempt_interval_s` until
  a recording file actually appears, for up to `start_window_s` (default 2
  minutes) — so a momentarily-busy overlay just gets another try.
- **Never stops blindly.** The stop command is only sent if a recording file
  actually exists. If nothing is recording, it does nothing — so it can't
  accidentally *start* a recording into the post-match lobby.

## Programmatic control

The daemon drives ShadowPlay **programmatically** — it sends ShadowPlay's own
start/stop command rather than faking the `F10` keypress. It's the same control
channel the NVIDIA overlay's Record button uses, so it doesn't depend on the game
having focus and can't be "dropped" during the fullscreen load the way a keystroke
can. There is **no keystroke fallback** — if the overlay can't be reached the
match simply isn't recorded (see [If the overlay is unreachable](#if-the-overlay-is-unreachable)).

How it reaches ShadowPlay: the modern NVIDIA App is a Chromium (CEF) app whose
overlay issues `window.cefQuery(...)` calls that route to the native
`ShareServer` module over NVIDIA's message bus. We call that same channel from
outside by turning on CEF's remote-debugging port and driving it over the Chrome
DevTools Protocol. The start/stop command (`EnableRecord` with `{enable: true}` /
`{enable: false}` — the overlay's own manual-record toggle), the module
(`ShareServer`) and the message wrapper were taken from the NVIDIA App's own
JavaScript and live in the `[ipc]` table of [`config.toml`](config.toml) so they
can be retuned if a future App renames them.

(The DevTools client connects with no `Origin` header, so Chromium's remote-debug
origin guard doesn't 403 it — no `--remote-allow-origins` switch needed.)

### One-time setup

1. **Install the new dependency.** In `capture/`, run `uv sync` — it adds
   `websocket-client` (the DevTools-Protocol transport).

2. **Open the overlay's CEF debug port.** Edit (as admin; back it up first):

   ```
   C:\Program Files\NVIDIA Corporation\NVIDIA App\CEF\Resources\NVIDIA Overlay.json
   ```

   and add one switch to the `"switches"` list. The port must match
   `[ipc].debug_port` in `config.toml` (default `49222`):

   ```json
   {"switches":[
       "nv-combine-processes=false",
       "nv-osc=true",
       "...leave the existing entries...",
       "nv-remote-debugging-port=49222"
   ]}
   ```

   Mind the commas — every entry except the last needs a trailing comma. (If
   `nv-remote-debugging-port` has no effect on your build, try the plain CEF
   name `remote-debugging-port=49222`.)

3. **Fully restart the NVIDIA App** — quit it from the system-tray icon (not just
   closing the window) so the overlay relaunches with the new switch.

> **Security note.** This opens an *unauthenticated* debug port on `127.0.0.1`.
> It's loopback-only (not reachable from other machines), but any local program
> could use it to run code in the overlay's page — an acceptable trade-off on a
> personal single-user PC, but don't enable it on a shared or hostile machine.
> A NVIDIA App update can reset `NVIDIA Overlay.json`; if IPC stops working after
> an update, re-add the line.

### Verify it

Three helpers, all safe to run *while the daemon is running* (they don't touch
its single-instance lock):

```powershell
uv run python -m deadlock_capture --ipc-status    # is the overlay reachable?
uv run python -m deadlock_capture --test-record   # start, wait 8s, stop
uv run python -m deadlock_capture --discover-ipc  # print the overlay's own cefQuery calls
```

- **`--ipc-status`** confirms the debug port is open and an overlay page exposing
  `window.cefQuery` was found. If it can't connect it prints exactly what to fix.
- **`--test-record`** fires the configured start command, waits 8 s, then stops.
  Afterwards look in your recordings folder for a fresh ~8 s clip — this is the
  real end-to-end proof.
- **`--discover-ipc`** hooks the overlay's `cefQuery` and prints each call as it
  happens. Run it, then click **Record** / **Stop** in the overlay (`Alt+Z`); it
  flags the capture-related calls so you can copy the exact method/payload into
  the `[ipc]` table — handy if a future NVIDIA App renames `EnableRecord`.

### If the overlay is unreachable

There is **no keystroke fallback**. If the debug port isn't open (e.g. an NVIDIA
App update reset `NVIDIA Overlay.json`) or the App isn't running, the start
command fails, the daemon logs it, and that match isn't recorded. Re-add the port
line (step 2 above) and restart the App to restore it. A command that was *sent
but not acknowledged* is treated as "sent" (the file-watch confirms the
recording), so a working recording is never disturbed. If a match unexpectedly
didn't record, check `capture.log`.

## Reminders

Every `interval_s` (default hourly) the daemon plays a short notification
**sound** — your cue to stretch, look away, drink water, whatever. No popup, no
window, no focus stealing; just audio. By default it's a **soft synthesized
chime** — two warm sine notes (a gentle descending C6→G5) with a smooth decay,
like a glockenspiel rather than a square-wave beep, and deliberately *not* a
Windows system sound so you won't confuse it with any other app. Customize it
via `tones`, or point `sound_file` at a `.wav` to use that instead.

- **Suppressed while the game is running** — covers both being in a match and
  recording, so nothing plays mid-game.
- **Queued, not dropped.** A reminder that comes due mid-game is held and plays
  once `defer_delay_s` after the game closes. Multiple reminders due during one
  session collapse into a single nudge — no burst the moment you quit.

All of it lives under the `[reminders]` table in
[`config.toml`](config.toml) — set `enabled = false` to turn it off, change
`interval_s` / `defer_delay_s`, retune the `tones`, or set `sound_file` to a
custom `.wav` (which takes precedence over `tones`).

To hear the sound right now (e.g. after pointing `sound_file` at a `.wav`),
without waiting an hour:

```powershell
uv run python -m deadlock_capture --test-sound
```

It plays the configured sound once and exits — safe to run even while the
daemon is already running.

## Requirements

1. **Steam launch option `-condebug`** on Deadlock (right-click → Properties →
   Launch Options). Without it the game writes no `console.log` and nothing is
   detected.
2. **NVIDIA ShadowPlay** enabled (Instant Replay / recording turned on in the
   NVIDIA App overlay), so a manual recording can be made.
3. **The CEF debug-port setup** under [Programmatic
   control](#programmatic-control), plus `websocket-client` (installed by
   `uv sync`) — this is how the daemon drives ShadowPlay.

Everything tunable — paths, the size cap, the recordable map list, and the IPC
command — lives in [`config.toml`](config.toml).

## Run

```powershell
uv sync                       # one-time: create venv + install psutil, websocket-client
uv run python -m deadlock_capture
```

For a background run with no console window, launch via `pythonw.exe`:

```powershell
uv run pythonw -m deadlock_capture
```

### Autostart at logon

Point a Startup-folder shortcut (or a Task Scheduler "at logon" task) at the
`pythonw -m deadlock_capture` command above.

## Is it working? (it runs invisibly)

The daemon runs windowless, so there's no window to look at — it writes a log
file instead:

```
capture.log
```

Tail it to see live activity (startup banner, `running=true` when Deadlock is
up, `recording started`, `renamed -> <matchId>.mp4`, and an "alive" heartbeat
every 5 min). Any crash is written there with a full traceback.

```powershell
Get-Content capture.log -Tail 20 -Wait
```

Check whether it's running:

```powershell
Get-CimInstance Win32_Process -Filter "Name='python.exe' OR Name='pythonw.exe'" |
  Where-Object { $_.CommandLine -like '*deadlock_capture*' } |
  Select-Object ProcessId, Name
```

Only one copy can run at a time (a named mutex guards it); a second launch logs
"another instance is already running" and exits.

## Tests

```powershell
uv run python tests/run_tests.py
```
