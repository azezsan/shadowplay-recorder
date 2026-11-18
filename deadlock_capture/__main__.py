"""Entry point: watch deadlock.exe, tail its console.log, and record matches.

Single-threaded loop. Every 3s we check whether the game is running; while it
is, we poll the console.log once a second and feed parsed events to the state
machine. Recording start/stop is driven by ShadowPlay (see recorder.py).
"""

from __future__ import annotations

import ctypes
import os
import sys
import threading
import time
from typing import Callable

import psutil

from . import cleanup, focus, log, nvipc
from .config import Config, load
from .nvipc import NvOverlayIpc
from .recorder import Recorder
from .reminder import Reminder, play_sound
from .tail import MapLoaded, MatchEnded, MatchStarted, Tailer, parse_line

PROCESS_INTERVAL_S = 3.0
LOG_INTERVAL_S = 1.0
MISSING_LOG_WARN_AFTER_S = 30.0
RUNNING_MISS_TICKS_TO_EXIT = 2
HEARTBEAT_S = 300.0  # log an "alive" line every 5 min so the log shows it's up
# After stealing focus back to the game, wait this long before sending the record
# command so ShadowPlay sees the game as the active app.
FOCUS_SETTLE_S = 1.5

# Holds the single-instance mutex handle for the life of the process.
_mutex_handle: int | None = None


def acquire_single_instance() -> bool:
    """Return True if we got the lock, False if another instance holds it.
    Uses a named Windows mutex that the OS releases automatically when this
    process dies — so a crash never leaves a stale lock behind."""
    global _mutex_handle
    ERROR_ALREADY_EXISTS = 183
    try:
        kernel32 = ctypes.windll.kernel32  # type: ignore[attr-defined]
        handle = kernel32.CreateMutexW(None, False, "Local\\DeadlockCaptureDaemon")
        if not handle:
            return True  # couldn't create the mutex; don't block startup
        if kernel32.GetLastError() == ERROR_ALREADY_EXISTS:
            return False
        _mutex_handle = handle
        return True
    except Exception:  # noqa: BLE001 — non-Windows or API issue: don't block
        return True


def is_process_running(name: str) -> bool:
    name = name.lower()
    for p in psutil.process_iter(["name"]):
        try:
            if (p.info["name"] or "").lower() == name:
                return True
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue
    return False


class StateMachine:
    """Match lifecycle distilled from the original state-machine.ts.

    Recording is gated on the map: we only start once a *recordable* map loads,
    so bot/sandbox/custom matches never trigger a recording.
    """

    def __init__(
        self,
        cfg: Config,
        recorder: Recorder,
        on_game_exit: Callable[[], None] | None = None,
        focus_game: Callable[[], bool] | None = None,
        is_game_foreground: Callable[[], bool] | None = None,
    ) -> None:
        self.cfg = cfg
        self.recorder = recorder
        self.on_game_exit = on_game_exit
        # Overridable so tests don't touch the Win32 foreground API.
        self.focus_game = focus_game or (lambda: focus.focus_process(cfg.process))
        self.is_game_foreground = is_game_foreground or (lambda: focus.is_foreground(cfg.process))

        self.running = False
        self.in_match = False
        self.entered = False  # have we fired recording-start for this match?
        self.recording = False  # confirmed: a recording file exists for this match
        self.match_id: str | None = None
        self.started_ts: float | None = None
        self._miss_ticks = 0

        # Non-blocking start-retry state (driven by tick()).
        self._starting = False
        self._start_deadline = 0.0  # monotonic; give up after this
        self._next_press_at = 0.0  # monotonic; earliest time for the next send
        self._press_count = 0
        self._focus_settling = False  # deferred this send once to let focus settle

        # Delayed-stop state (driven by tick()): after a match ends we keep
        # recording for stop_delay_s before sending the stop command.
        self._stop_pending = False
        self._stop_at = 0.0  # monotonic; fire the stop at/after this
        self._pending_match_id: str | None = None
        self._pending_since: float | None = None

    # ── process running transition ──────────────────────────────────────────

    def on_running(self, running: bool) -> None:
        if running:
            self._miss_ticks = 0
            if not self.running:
                self.running = True
                log.info("running=true")
            return

        if not self.running:
            return

        self._miss_ticks += 1
        if self._miss_ticks < RUNNING_MISS_TICKS_TO_EXIT:
            return

        self.running = False
        self._miss_ticks = 0
        log.info("running=false")
        self._game_exit()

    # ── log event ingestion ─────────────────────────────────────────────────

    def on_event(self, ev: object, now: float) -> None:
        if isinstance(ev, MatchStarted):
            if self.in_match:
                return
            self.in_match = True
            self.match_id = ev.match_id
            self.started_ts = time.time()
            log.info(f"matchStarted matchId={ev.match_id}")
            # A match was just found — you're probably alt-tabbed, so pull the
            # game up. Best-effort: focus_game() never raises.
            if self.cfg.focus_on_match_start:
                self.focus_game()

        elif isinstance(ev, MapLoaded):
            if not self.in_match:
                return
            if ev.map not in self.cfg.recordable_maps:
                return
            if self.entered:
                return
            log.info(f"map={ev.map} (recordable) — will start recording")
            self.entered = True
            self._begin_start(now)

        elif isinstance(ev, MatchEnded):
            if not self.in_match:
                return
            log.info("matchEnded")
            self.in_match = False
            self._cancel_start()  # stop any in-progress start retry immediately
            if self.entered:
                self._begin_stop(now)  # keep recording for stop_delay_s, then stop
                self.entered = False
            else:
                log.info("match was not recorded (non-recordable map); skipping")
            self.match_id = None
            self.started_ts = None

    # ── non-blocking recording start ─────────────────────────────────────────

    def _begin_start(self, now: float) -> None:
        """Kick off the start-retry. The actual sending happens in tick()."""
        # If a previous match's delayed stop is still pending, finalize it first
        # so we never start a new recording on top of the old one.
        if self._stop_pending:
            self._flush_stop()
        self._starting = True
        self._start_deadline = now + self.cfg.start_window_s
        self._next_press_at = now + self.cfg.start_delay_s  # settle before first send
        self._press_count = 0
        self._focus_settling = False
        log.info(
            f"start armed: will send the record command, retrying up to "
            f"{self.cfg.start_window_s:.0f}s"
        )

    def _cancel_start(self) -> None:
        if self._starting:
            self._starting = False
            log.info("start retry stopped")

    def tick(self, now: float) -> None:
        """Advance the start-retry and the delayed stop. Called every main-loop
        iteration. Sends the record command on an interval until a recording file
        appears or the window expires (a resend covers the overlay being
        momentarily busy)."""
        # Delayed stop: fire once its timer elapses.
        if self._stop_pending and now >= self._stop_at:
            self._flush_stop()

        if not self._starting or self.started_ts is None:
            return

        # Confirmed? A file newer than match start is our proof.
        if self.recorder.has_recording_since(self.started_ts):
            self.recording = True
            self._starting = False
            log.info(f"recording confirmed (after {self._press_count} send(s))")
            return

        if now >= self._start_deadline:
            self._starting = False
            log.warn(
                f"recording did not start within {self.cfg.start_window_s:.0f}s after "
                f"{self._press_count} send(s); giving up (this match is not recorded)"
            )
            return

        if now < self._next_press_at:
            return

        # ShadowPlay only records the game it considers active, so every attempt
        # starts by pulling Deadlock back to the front — you may have alt-tabbed
        # away since the last one. We defer the send by FOCUS_SETTLE_S once so
        # the overlay notices the switch, then send regardless of whether focus
        # actually took (a refused foreground must not stall the retry).
        if self.cfg.focus_on_match_start and not self._focus_settling:
            if not self.is_game_foreground():
                log.info("game not in the foreground — refocusing before the record command")
                self.focus_game()
                self._focus_settling = True
                self._next_press_at = now + FOCUS_SETTLE_S
                return

        self._focus_settling = False
        try:
            self.recorder.press()
        except Exception as err:  # noqa: BLE001 — never let a send kill the loop
            log.error(f"record command failed: {err}")
            return
        self._press_count += 1
        self._next_press_at = now + self.cfg.start_attempt_interval_s
        log.info(f"record command sent (#{self._press_count}); awaiting confirmation")

    # ── actions ─────────────────────────────────────────────────────────────

    def _begin_stop(self, now: float) -> None:
        """Arm the delayed stop: tick() fires it stop_delay_s later, so the
        recording keeps running until then (captures the post-match screen). A
        zero delay stops immediately."""
        self._pending_match_id = self.match_id
        self._pending_since = self.started_ts
        self._stop_pending = True
        self._stop_at = now + self.cfg.stop_delay_s
        if self.cfg.stop_delay_s > 0:
            log.info(f"match ended — stopping recording in {self.cfg.stop_delay_s:.0f}s")
        else:
            self._flush_stop()

    def _flush_stop(self) -> None:
        """Perform a pending stop now: send the stop command and rename the file."""
        if not self._stop_pending:
            return
        self._stop_pending = False
        match_id, since = self._pending_match_id, self._pending_since
        self._pending_match_id = None
        self._pending_since = None
        self._stop_and_rename(match_id, since)

    def _stop_and_rename(self, match_id: str | None, since: float | None) -> None:
        since = since if since is not None else time.time()
        try:
            stopped = self.recorder.stop(self.recording, since)
        except Exception as err:  # noqa: BLE001
            log.error(f"stopRecording failed: {err}")
            stopped = False
        self.recording = False

        # If nothing was recording, stop() didn't send anything — so there's
        # nothing to rename (and we avoid a pointless 30s wait-for-file).
        if not stopped or match_id is None:
            return
        try:
            renamed = self.recorder.rename(match_id, since)
            if renamed:
                log.info(f"renamed -> {renamed.name}")
        except Exception as err:  # noqa: BLE001
            log.error(f"renameRecording failed: {err}")

    def _game_exit(self) -> None:
        # Stop any in-progress start retry first so we never send into a closed
        # game.
        self._cancel_start()
        # If a delayed stop was pending, or we were mid-recording, stop + save
        # now — the game is gone, so don't wait out the rest of the stop delay.
        if self._stop_pending:
            self._flush_stop()
        elif self.entered:
            self._stop_and_rename(self.match_id, self.started_ts)
        self.entered = False
        self.in_match = False
        self.match_id = None
        self.started_ts = None
        if self.on_game_exit is not None:
            self.on_game_exit()


def build_recorder(cfg: Config) -> Recorder:
    """Construct the recorder that drives ShadowPlay through the NVIDIA overlay IPC."""
    ipc = NvOverlayIpc(
        cfg.ipc_host,
        cfg.ipc_debug_port,
        cfg.ipc_connect_timeout_s,
        cfg.ipc_call_timeout_s,
    )
    log.info(
        f"recorder: NVIDIA IPC (overlay {cfg.ipc_host}:{cfg.ipc_debug_port}, "
        f"start={cfg.ipc_start_method}, stop={cfg.ipc_stop_method})"
    )
    return Recorder(
        cfg.recordings_folder,
        cfg.rename_template,
        ipc,
        system=cfg.ipc_system,
        module=cfg.ipc_module,
        start_method=cfg.ipc_start_method,
        start_payload=cfg.ipc_start_payload,
        stop_method=cfg.ipc_stop_method,
        stop_payload=cfg.ipc_stop_payload,
    )


def run(cfg: Config) -> None:
    recorder = build_recorder(cfg)

    def schedule_cleanup() -> None:
        def fire() -> None:
            try:
                cleanup.run(cfg.recordings_folder, cfg.max_gb)
                log.info(f"cleanup ran (cap {cfg.max_gb:g} GB)")
            except Exception as err:  # noqa: BLE001
                log.error(f"cleanup failed: {err}")

        t = threading.Timer(cfg.exit_delay_s, fire)
        t.daemon = True
        t.start()

    sm = StateMachine(cfg, recorder, on_game_exit=schedule_cleanup)
    tailer = Tailer(cfg.console_log)
    log.info(f"tailing {cfg.console_log}")

    reminder = Reminder(
        cfg.reminders_enabled,
        cfg.reminder_interval_s,
        cfg.reminder_defer_delay_s,
        cfg.reminder_sound_file or None,
        cfg.reminder_tones,
    )
    reminder.start(time.monotonic())

    next_proc = 0.0
    next_heartbeat = time.monotonic() + HEARTBEAT_S
    missing_log_since: float | None = None
    missing_log_warned = False

    while True:
        now = time.monotonic()
        if now >= next_heartbeat:
            if sm._starting:
                state = "starting recording"
            elif sm.in_match:
                state = "in match"
            elif sm.running:
                state = "game up"
            else:
                state = "idle"
            log.info(f"heartbeat: {state} (recording={sm.recording})")
            next_heartbeat = now + HEARTBEAT_S
        if now >= next_proc:
            try:
                sm.on_running(is_process_running(cfg.process))
            except Exception as err:  # noqa: BLE001
                log.warn(f"process poll skipped: {err}")
            next_proc = now + PROCESS_INTERVAL_S

        if sm.running:
            res = tailer.poll()
            if res is None:
                if missing_log_since is None:
                    missing_log_since = now
                if not missing_log_warned and now - missing_log_since >= MISSING_LOG_WARN_AFTER_S:
                    log.warn(
                        f"console.log not found at {tailer.path} after 30s of running "
                        "— did you add -condebug to Steam launch options?"
                    )
                    missing_log_warned = True
            else:
                missing_log_since = None
                missing_log_warned = False
                if res.truncated:
                    log.info("log truncation detected, replaying from start")
                for line in res.lines:
                    ev = parse_line(line)
                    if ev is not None:
                        sm.on_event(ev, now)

        # Drive the non-blocking start-retry every iteration.
        sm.tick(time.monotonic())

        # Wellness reminders — suppressed while the game is running (which
        # covers both in-match and recording).
        reminder.tick(time.monotonic(), suppressed=sm.running)

        time.sleep(LOG_INTERVAL_S)


def test_sound() -> None:
    """Play the reminder sound once and exit — a quick way to confirm it's
    audible and to try out a custom sound_file. Doesn't touch the single-
    instance lock, so it works even while the daemon is running."""
    try:
        cfg = load()
        sound, tones = cfg.reminder_sound_file or None, cfg.reminder_tones
    except Exception:  # noqa: BLE001 — fall back to the default chime if config won't load
        sound, tones = None, ()
    print(f"playing reminder sound ({sound if sound else 'chime'})...")
    play_sound(sound, tones)
    time.sleep(2.5)  # give the fire-and-forget sound time to finish before we exit


# ── IPC diagnostics (safe to run while the daemon is running) ─────────────────


def _make_ipc(cfg: Config) -> NvOverlayIpc:
    return NvOverlayIpc(
        cfg.ipc_host, cfg.ipc_debug_port, cfg.ipc_connect_timeout_s, cfg.ipc_call_timeout_s
    )


def ipc_status() -> None:
    """--ipc-status: can we reach the overlay and drive ShadowPlay via IPC?"""
    cfg = load()
    info = _make_ipc(cfg).status()
    print(f"NVIDIA overlay IPC — {info['endpoint']}")
    if not info.get("reachable"):
        print(f"  reachable: NO — {info.get('error')}")
        print(f"  -> add  nv-remote-debugging-port={cfg.ipc_debug_port}  to the \"switches\" list in")
        print(r"     C:\Program Files\NVIDIA Corporation\NVIDIA App\CEF\Resources\NVIDIA Overlay.json")
        print("     then fully quit and restart the NVIDIA App. See capture/README.md.")
        return
    print(f"  reachable: yes ({info.get('target_count')} CEF targets)")
    for t in info.get("targets", []):
        cq = t["cefquery"]
        mark = "cefQuery=YES" if cq is True else ("cefQuery=err" if cq == "err" else "cefQuery=no")
        print(f"    - [{t.get('type')}] {mark}  title={t.get('title')!r}")
        print(f"        url={t.get('url')}")
        if t.get("error"):
            print(f"        error={t.get('error')}")
    if info.get("ready"):
        print(f"  -> ready: cefQuery found — the daemon can drive ShadowPlay via IPC")
    else:
        print("  -> no target exposes window.cefQuery yet. Try: open the overlay with")
        print("     Alt+Z (or launch Deadlock) so its page loads, then re-run. If it's")
        print("     still empty, the record UI may live in the main app window — see README.")


def test_record() -> None:
    """--test-record: fire the configured start command, wait, then stop — a
    quick end-to-end check that IPC actually records. Doesn't touch the daemon's
    single-instance lock, so it's safe to run alongside it."""
    cfg = load()
    ipc = _make_ipc(cfg)
    hold_s = 8.0

    def fire(label: str, method: str, payload: dict) -> bool:
        try:
            res = ipc.send_share_command(method, payload, cfg.ipc_system, cfg.ipc_module)
            print(f"  {label} ({method}): ok -> {res}")
            return True
        except nvipc.IpcNoAck as err:
            print(f"  {label} ({method}): sent, no ack ({err}) — likely worked")
            return True
        except nvipc.IpcError as err:
            print(f"  {label} ({method}): FAILED — {err}")
            return False

    print(f"IPC test-record via {cfg.ipc_host}:{cfg.ipc_debug_port}")
    if not fire("start", cfg.ipc_start_method, cfg.ipc_start_payload):
        return
    print(f"  recording for {hold_s:g}s ...")
    time.sleep(hold_s)
    fire("stop", cfg.ipc_stop_method, cfg.ipc_stop_payload)
    print(f"done — check {cfg.recordings_folder} for a new ~{hold_s:g}s clip.")


def discover_ipc() -> None:
    """--discover-ipc: hook the overlay's cefQuery and print the exact commands
    it emits, so you can confirm the record start/stop method names for [ipc]."""
    cfg = load()
    ipc = _make_ipc(cfg)
    try:
        state = ipc.install_query_logger()
    except nvipc.IpcError as err:
        print(f"cannot reach the overlay: {err}")
        print(f"-> run  --ipc-status  first to check the debug port ({cfg.ipc_debug_port}).")
        return
    print(f"cefQuery logger {state}.")
    print("Now, in the NVIDIA overlay (Alt+Z), click Record to START, wait a few")
    print("seconds, then STOP. Watching for 60s — capture-related calls are flagged:\n")
    seen = 0
    deadline = time.monotonic() + 60.0
    keys = ("Record", "Capture", "Replay", "Share", "Highlight")
    while time.monotonic() < deadline:
        try:
            entries = ipc.read_query_log()
        except nvipc.IpcError:
            entries = []
        for e in entries[seen:]:
            flag = ">>" if any(k in str(e) for k in keys) else "  "
            print(f"  {flag} {e}")
        seen = max(seen, len(entries))
        time.sleep(1.0)
    print("\ndone. Copy the method (and payload) from a flagged line into config.toml [ipc].")


def main() -> None:
    if "--test-sound" in sys.argv:
        test_sound()
        return
    if "--ipc-status" in sys.argv:
        ipc_status()
        return
    if "--test-record" in sys.argv:
        test_record()
        return
    if "--discover-ipc" in sys.argv:
        discover_ipc()
        return

    log.info(f"======== deadlock-capture starting (pid {os.getpid()}) ========")
    log.info(f"log file: {log.LOG_PATH}")

    if not acquire_single_instance():
        log.warn("another deadlock-capture instance is already running; exiting")
        return

    try:
        cfg = load()
    except Exception:  # noqa: BLE001
        log.exception("failed to load config.toml")
        return

    log.info(
        f"config: recordings={cfg.recordings_folder} | recordable_maps={list(cfg.recordable_maps)} "
        f"| cap={cfg.max_gb:g}GB"
    )

    try:
        run(cfg)
    except KeyboardInterrupt:
        log.info("interrupted; exiting")
    except Exception:  # noqa: BLE001 — last resort: record the crash before dying
        log.exception("daemon crashed")


if __name__ == "__main__":
    main()
