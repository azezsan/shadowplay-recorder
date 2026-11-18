"""Self-contained checks (no pytest). Run: uv run python tests/run_tests.py"""

from __future__ import annotations

import json
import os
import struct
import sys
import tempfile
import time
from pathlib import Path

# Send test logs to a throwaway file so we never pollute the real capture.log.
# Must be set before importing the package (log.py reads it at import time).
os.environ["DEADLOCK_CAPTURE_LOG"] = os.path.join(tempfile.gettempdir(), "deadlock_capture_test.log")

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from deadlock_capture import cleanup  # noqa: E402
from deadlock_capture import nvipc  # noqa: E402
from deadlock_capture.config import Config  # noqa: E402
from deadlock_capture.recorder import Recorder  # noqa: E402
from deadlock_capture.tail import (  # noqa: E402
    MapLoaded,
    MatchEnded,
    MatchStarted,
    Tailer,
    parse_line,
)
from deadlock_capture import __main__ as app  # noqa: E402

_passed = 0


def check(cond: bool, label: str) -> None:
    global _passed
    if not cond:
        raise AssertionError(f"FAILED: {label}")
    _passed += 1
    print(f"  ok: {label}")


def make_cfg(
    folder: str,
    console_log: str,
    start_delay_s: float = 0.0,
    start_window_s: float = 120.0,
    start_attempt_interval_s: float = 5.0,
    stop_delay_s: float = 0.0,
    focus_on_match_start: bool = True,
) -> Config:
    return Config(
        process="deadlock.exe",
        console_log=console_log,
        recordings_folder=folder,
        max_gb=100,
        exit_delay_s=30,
        recordable_maps=("start",),
        rename_template="{matchId}",
        start_delay_s=start_delay_s,
        start_window_s=start_window_s,
        start_attempt_interval_s=start_attempt_interval_s,
        stop_delay_s=stop_delay_s,
        focus_on_match_start=focus_on_match_start,
    )


class FakeIpc:
    """Stands in for NvOverlayIpc — no CDP, no network. Records every command;
    ``mode`` selects the outcome and ``fail_first`` makes the first N *start*
    commands fail (to exercise the resend loop). A successful start 'creates' a
    recording file, like ShadowPlay would, so the state machine's file-watch can
    confirm. Start vs stop is told apart by the {enable: true/false} payload."""

    def __init__(self, folder=None, mode="ok", fail_first=0):
        self.calls = []  # list of (method, payload)
        self.folder = folder
        self.mode = mode  # "ok" | "fail" | "noack"
        self.fail_first = fail_first

    def send_share_command(self, method, payload, system, module):
        self.calls.append((method, payload))
        is_start = payload.get("enable") is True
        starts = sum(1 for _m, p in self.calls if p.get("enable") is True)
        if is_start and starts <= self.fail_first:
            raise nvipc.IpcUnavailable("overlay busy (test)")
        if self.mode == "fail":
            raise nvipc.IpcUnavailable("overlay unreachable (test)")
        if self.mode == "noack":
            self._maybe_create(is_start)  # it worked, we just didn't hear back
            raise nvipc.IpcNoAck("no ack (test)")
        self._maybe_create(is_start)
        return {"ok": True}

    def _maybe_create(self, is_start):
        if self.folder and is_start:
            p = Path(self.folder) / "DefaultShadowPlay.mp4"
            if not p.exists():
                p.write_bytes(b"x" * 1024)
                future = time.time() + 5
                os.utime(p, (future, future))


def make_recorder(folder, ipc):
    return Recorder(
        folder,
        "{matchId}",
        ipc,
        system="CrimsonNative",
        module="ShareServer",
        start_method="EnableRecord",
        start_payload={"enable": True},
        stop_method="EnableRecord",
        stop_payload={"enable": False},
    )


def make_sm(cfg, rec, **kw):
    # Default to a no-op focus so tests never touch the Win32 foreground API.
    kw.setdefault("focus_game", lambda: True)
    kw.setdefault("is_game_foreground", lambda: True)
    return app.StateMachine(cfg, rec, **kw)


def _stops(ipc):
    return [p for _m, p in ipc.calls if p.get("enable") is False]


def test_parser() -> None:
    print("test_parser")
    ms = parse_line("Lobby 5 for Match 81038409 created")
    check(isinstance(ms, MatchStarted) and ms.match_id == "81038409", "matchStarted parsed")
    check(isinstance(parse_line("Lobby 5 for Match 81038409 destroyed"), MatchEnded), "matchEnded parsed")
    ml = parse_line('[Client] Map: "start"')
    check(isinstance(ml, MapLoaded) and ml.map == "start", "mapLoaded parsed")
    check(parse_line("some unrelated line") is None, "unrelated line ignored")
    check(parse_line("[Client] Players: 12 (0 bots) / 12 humans") is None, "playerInfo dropped")


def test_state_machine_records_real_match() -> None:
    print("test_state_machine_records_real_match")
    with tempfile.TemporaryDirectory() as d:
        ipc = FakeIpc(folder=d, mode="ok")
        sm = make_sm(make_cfg(d, "nonexistent.log"), make_recorder(d, ipc))
        t = 100.0
        sm.on_running(True)
        sm.on_event(MatchStarted("81038409"), t)
        check(sm.in_match and not sm.entered, "in match, not yet recording (waiting for map)")
        sm.on_event(MapLoaded("start"), t)
        check(sm.entered and sm._starting and not sm.recording, "armed, but nothing sent until tick")
        sm.tick(t)  # settle delay is 0 -> first send now
        check(len(ipc.calls) == 1 and ipc.calls[0][1] == {"enable": True}, "start command sent on first tick")
        sm.tick(t + 1)  # file now present -> confirmed
        check(sm.recording and not sm._starting, "recording confirmed after file appears")
        sm.on_event(MatchEnded(), t + 2)
        check(ipc.calls[-1][1] == {"enable": False}, "stop command sent")
        check(not sm.recording and not sm.entered and not sm.in_match, "state reset after match")
        check((Path(d) / "81038409.mp4").exists(), "recording renamed to <matchId>.mp4")


def test_state_machine_retries_until_success() -> None:
    print("test_state_machine_retries_until_success")
    with tempfile.TemporaryDirectory() as d:
        ipc = FakeIpc(folder=d, mode="ok", fail_first=2)  # first two start sends fail
        sm = make_sm(make_cfg(d, "nonexistent.log", start_attempt_interval_s=5.0), make_recorder(d, ipc))
        t = 100.0
        sm.on_running(True)
        sm.on_event(MatchStarted("4242"), t)
        sm.on_event(MapLoaded("start"), t)
        sm.tick(t)  # send #1 (fails)
        check(len(ipc.calls) == 1 and not sm.recording, "send 1 failed, still trying")
        sm.tick(t + 1)  # too soon for the next send
        check(len(ipc.calls) == 1, "no send before the interval elapses")
        sm.tick(t + 5)  # send #2 (fails)
        check(len(ipc.calls) == 2 and not sm.recording, "send 2 failed, still trying")
        sm.tick(t + 10)  # send #3 (succeeds -> file created)
        check(len(ipc.calls) == 3, "send 3 after the interval")
        sm.tick(t + 11)  # file present -> confirmed
        check(sm.recording, "recording confirmed after persistent retries")
        sm.on_event(MatchEnded(), t + 12)
        check((Path(d) / "4242.mp4").exists(), "recording saved")


def test_no_stray_send_when_nothing_recording() -> None:
    print("test_no_stray_send_when_nothing_recording")
    with tempfile.TemporaryDirectory() as d:
        ipc = FakeIpc(folder=None, mode="fail")  # every send fails; no file ever
        sm = make_sm(
            make_cfg(d, "nonexistent.log", start_window_s=10.0, start_attempt_interval_s=5.0),
            make_recorder(d, ipc),
        )
        t = 100.0
        sm.on_running(True)
        sm.on_event(MatchStarted("8"), t)
        sm.on_event(MapLoaded("start"), t)
        sm.tick(t)  # send #1 (fails)
        sm.tick(t + 5)  # send #2 (fails)
        sm.tick(t + 10)  # deadline reached -> give up
        check(not sm._starting and not sm.recording, "gave up after the window with no recording")
        sm.on_event(MatchEnded(), t + 11)
        check(_stops(ipc) == [], "stop never sent when nothing was recording")
        vids = [p for p in Path(d).iterdir() if p.suffix == ".mp4"]
        check(vids == [], "no stray recording created at match end")


def test_state_machine_skips_non_recordable_map() -> None:
    print("test_state_machine_skips_non_recordable_map")
    with tempfile.TemporaryDirectory() as d:
        ipc = FakeIpc(folder=d, mode="ok")
        sm = make_sm(make_cfg(d, "nonexistent.log"), make_recorder(d, ipc))
        t = 100.0
        sm.on_running(True)
        sm.on_event(MatchStarted("999"), t)
        sm.on_event(MapLoaded("sandbox"), t)  # not in recordable_maps
        sm.tick(t)
        check(not sm.entered and not sm._starting and not sm.recording, "no recording on non-recordable map")
        sm.on_event(MatchEnded(), t + 1)
        check(ipc.calls == [], "never sent a command for a bot/sandbox match")


def test_game_exit_mid_recording() -> None:
    print("test_game_exit_mid_recording")
    with tempfile.TemporaryDirectory() as d:
        ipc = FakeIpc(folder=d, mode="ok")
        exits: list[int] = []
        sm = make_sm(
            make_cfg(d, "nonexistent.log"), make_recorder(d, ipc), on_game_exit=lambda: exits.append(1)
        )
        t = 100.0
        sm.on_running(True)
        sm.on_event(MatchStarted("555"), t)
        sm.on_event(MapLoaded("start"), t)
        sm.tick(t)  # send start
        sm.tick(t + 1)  # confirm
        check(sm.recording, "recording in progress")
        # Game disappears: needs 2 consecutive misses to count as exit.
        sm.on_running(False)
        check(sm.running, "still running after 1 miss")
        sm.on_running(False)
        check(not sm.running, "exited after 2 misses")
        check(_stops(ipc) == [{"enable": False}], "stop command sent on game exit")
        check((Path(d) / "555.mp4").exists(), "recording saved on game exit")
        check(exits == [1], "cleanup scheduled once on exit")


def test_stop_is_delayed_after_match_end() -> None:
    print("test_stop_is_delayed_after_match_end")
    with tempfile.TemporaryDirectory() as d:
        ipc = FakeIpc(folder=d, mode="ok")
        sm = make_sm(make_cfg(d, "nonexistent.log", stop_delay_s=10.0), make_recorder(d, ipc))
        t = 100.0
        sm.on_running(True)
        sm.on_event(MatchStarted("77"), t)
        sm.on_event(MapLoaded("start"), t)
        sm.tick(t)  # start
        sm.tick(t + 1)  # confirm
        check(sm.recording, "recording in progress")
        sm.on_event(MatchEnded(), t + 2)
        check(_stops(ipc) == [] and sm._stop_pending, "stop deferred, not sent at match end")
        sm.tick(t + 2 + 9)  # 9s into the 10s delay
        check(_stops(ipc) == [], "still recording during the delay")
        sm.tick(t + 2 + 10)  # delay elapsed -> stop fires
        check(_stops(ipc) == [{"enable": False}], "stop sent after the delay")
        check((Path(d) / "77.mp4").exists(), "recording renamed after the delayed stop")


def test_game_exit_flushes_pending_stop() -> None:
    print("test_game_exit_flushes_pending_stop")
    with tempfile.TemporaryDirectory() as d:
        ipc = FakeIpc(folder=d, mode="ok")
        sm = make_sm(make_cfg(d, "nonexistent.log", stop_delay_s=10.0), make_recorder(d, ipc))
        t = 100.0
        sm.on_running(True)
        sm.on_event(MatchStarted("88"), t)
        sm.on_event(MapLoaded("start"), t)
        sm.tick(t)
        sm.tick(t + 1)
        sm.on_event(MatchEnded(), t + 2)
        check(sm._stop_pending, "stop pending after match end")
        # Game process closes before the delay elapses -> flush immediately.
        sm.on_running(False)
        sm.on_running(False)
        check(not sm._stop_pending, "pending stop flushed on game exit")
        check(_stops(ipc) == [{"enable": False}], "stop sent on game exit")
        check((Path(d) / "88.mp4").exists(), "recording saved")


def test_recorder_no_ack_is_treated_as_sent() -> None:
    print("test_recorder_no_ack_is_treated_as_sent")
    with tempfile.TemporaryDirectory() as d:
        ipc = FakeIpc(folder=d, mode="noack")
        sm = make_sm(make_cfg(d, "nonexistent.log"), make_recorder(d, ipc))
        t = 100.0
        sm.on_running(True)
        sm.on_event(MatchStarted("9"), t)
        sm.on_event(MapLoaded("start"), t)
        sm.tick(t)  # no-ack, but the file is still created
        check(len(ipc.calls) == 1, "start command sent once")
        sm.tick(t + 1)
        check(sm.recording, "recording confirmed via file despite the missing ack")


def test_focus_on_match_start() -> None:
    print("test_focus_on_match_start")
    with tempfile.TemporaryDirectory() as d:
        focuses: list[int] = []
        sm = make_sm(
            make_cfg(d, "nonexistent.log"),
            make_recorder(d, FakeIpc(folder=d)),
            focus_game=lambda: focuses.append(1) or True,
        )
        t = 100.0
        sm.on_running(True)
        sm.on_event(MatchStarted("31337"), t)
        check(len(focuses) == 1, "game focused when a match starts")
        sm.on_event(MatchStarted("31337"), t + 1)  # duplicate line in the log
        check(len(focuses) == 1, "not re-focused while already in the match")
        sm.on_event(MapLoaded("start"), t + 2)
        sm.on_event(MatchEnded(), t + 3)
        check(len(focuses) == 1, "no extra focus while the game is already in front")

        # Disabled via config -> never touches focus.
        off: list[int] = []
        sm2 = make_sm(
            make_cfg(d, "nonexistent.log", focus_on_match_start=False),
            make_recorder(d, FakeIpc(folder=None)),
            focus_game=lambda: off.append(1) or True,
        )
        sm2.on_running(True)
        sm2.on_event(MatchStarted("1"), t)
        check(off == [], "focus_on_match_start=false disables focusing")


def test_refocuses_before_each_start_attempt() -> None:
    print("test_refocuses_before_each_start_attempt")
    with tempfile.TemporaryDirectory() as d:
        # No file ever appears, so the start keeps retrying; the game is never in
        # the foreground (you alt-tabbed away again after each pull).
        ipc = FakeIpc(folder=None, mode="fail")
        focuses: list[float] = []
        sm = make_sm(
            make_cfg(d, "nonexistent.log", start_window_s=60.0, start_attempt_interval_s=15.0),
            make_recorder(d, ipc),
            focus_game=lambda: focuses.append(1.0) or True,
            is_game_foreground=lambda: False,
        )
        t = 100.0
        sm.on_running(True)
        sm.on_event(MatchStarted("6161"), t)
        check(len(focuses) == 1, "focused at match start")
        sm.on_event(MapLoaded("start"), t)
        sm.tick(t)  # not in front -> refocus, defer the send
        check(len(focuses) == 2 and ipc.calls == [], "refocused instead of sending blind")
        sm.tick(t + app.FOCUS_SETTLE_S)  # settle elapsed -> send anyway
        check(len(ipc.calls) == 1 and len(focuses) == 2, "sent after the focus settles")
        sm.tick(t + app.FOCUS_SETTLE_S + 15)  # next attempt -> focus again first
        check(len(focuses) == 3 and len(ipc.calls) == 1, "each retry re-focuses the game")
        sm.tick(t + app.FOCUS_SETTLE_S * 2 + 15)
        check(len(ipc.calls) == 2, "second send lands after its own refocus")

        # A refused foreground must not stall the retry: focus_game returns False
        # and we still send.
        ipc2 = FakeIpc(folder=None, mode="fail")
        sm2 = make_sm(
            make_cfg(d, "nonexistent.log", start_window_s=60.0),
            make_recorder(d, ipc2),
            focus_game=lambda: False,
            is_game_foreground=lambda: False,
        )
        sm2.on_running(True)
        sm2.on_event(MatchStarted("6162"), t)
        sm2.on_event(MapLoaded("start"), t)
        sm2.tick(t)
        sm2.tick(t + app.FOCUS_SETTLE_S)
        check(len(ipc2.calls) == 1, "record command still sent when focus is refused")


def test_build_recorder() -> None:
    print("test_build_recorder")
    with tempfile.TemporaryDirectory() as d:
        rec = app.build_recorder(make_cfg(d, "x.log"))
        check(isinstance(rec, Recorder), "build_recorder -> Recorder")
        check(rec.start_method == "EnableRecord" and rec.start_payload == {"enable": True}, "start wired from config")
        check(rec.stop_method == "EnableRecord" and rec.stop_payload == {"enable": False}, "stop wired from config")


def test_cefquery_expression_builds_valid_ipc_json() -> None:
    print("test_cefquery_expression_builds_valid_ipc_json")
    req = nvipc._share_request_str("CrimsonNative", "ShareServer", "EnableRecord", {"enable": True})
    obj = json.loads(req)
    check(obj["command"] == "QUERY_IPC_EXTENSION_MESSAGE", "wrapper command set")
    check(obj["system"] == "CrimsonNative" and obj["module"] == "ShareServer", "routing set")
    check(obj["method"] == "EnableRecord" and obj["payload"] == {"enable": True}, "method/payload set")
    expr = nvipc._cefquery_expression(req)
    check("window.cefQuery" in expr, "expression calls window.cefQuery")
    check("onSuccess" in expr and "onFailure" in expr, "expression wires both callbacks")
    check(json.dumps(req) in expr, "request embedded as a valid JS string literal")


def test_tailer_truncation() -> None:
    print("test_tailer_truncation")
    with tempfile.TemporaryDirectory() as d:
        logp = os.path.join(d, "console.log")
        Path(logp).write_text("preexisting line\n", encoding="utf-8")
        t = Tailer(logp)
        first = t.poll()
        check(first is not None and first.lines == [], "first poll seeks to EOF (no replay)")

        with open(logp, "a", encoding="utf-8") as f:
            f.write("Lobby 1 for Match 7 created\n[Client] Map: \"start\"\n")
        r = t.poll()
        check(r is not None and len(r.lines) == 2 and not r.truncated, "appended lines read")

        # Truncate (rotate) the file, then write fresh content.
        Path(logp).write_text("Lobby 2 for Match 8 created\n", encoding="utf-8")
        r2 = t.poll()
        check(r2 is not None and r2.truncated and r2.lines == ["Lobby 2 for Match 8 created"], "truncation replays from start")


def test_tailer_partial_line_buffering() -> None:
    print("test_tailer_partial_line_buffering")
    with tempfile.TemporaryDirectory() as d:
        logp = os.path.join(d, "console.log")
        Path(logp).write_text("", encoding="utf-8")
        t = Tailer(logp)
        t.poll()  # seek to EOF
        with open(logp, "a", encoding="utf-8") as f:
            f.write("partial without newline")
        r = t.poll()
        check(r is not None and r.lines == [], "partial line buffered, not emitted")
        with open(logp, "a", encoding="utf-8") as f:
            f.write(" continued\n")
        r2 = t.poll()
        check(r2 is not None and r2.lines == ["partial without newline continued"], "buffered partial completed on newline")


def test_cleanup() -> None:
    print("test_cleanup")
    with tempfile.TemporaryDirectory() as d:
        # 3 files of 1 MB each; cap so only ~2 MB fits -> oldest deleted.
        names = ["old.mp4", "mid.mp4", "new.mp4"]
        for i, n in enumerate(names):
            p = Path(d) / n
            p.write_bytes(b"x" * (1024 * 1024))
            os.utime(p, (1000 + i, 1000 + i))  # ascending mtime: old < mid < new
        non_video = Path(d) / "notes.txt"
        non_video.write_bytes(b"y" * (1024 * 1024))

        cleanup.run(d, max_gb=2 / 1024)  # cap = 2 MB
        check(not (Path(d) / "old.mp4").exists(), "oldest video deleted")
        check((Path(d) / "mid.mp4").exists() and (Path(d) / "new.mp4").exists(), "newer videos kept")
        check(non_video.exists(), "non-video files untouched")


def test_reminder_plays_when_not_in_game() -> None:
    print("test_reminder_plays_when_not_in_game")
    from deadlock_capture.reminder import Reminder

    plays: list[int] = []
    r = Reminder(True, 3600.0, 60.0, None, play=lambda: plays.append(1))
    t = 1000.0
    r.start(t)  # next reminder at t + 3600
    r.tick(t + 10, suppressed=False)
    check(len(plays) == 0, "nothing before the interval elapses")
    r.tick(t + 3600, suppressed=False)  # due, not in-game -> plays now
    check(len(plays) == 1, "plays the sound when due and not in-game")


def test_reminder_queues_during_game_and_plays_after_delay() -> None:
    print("test_reminder_queues_during_game_and_plays_after_delay")
    from deadlock_capture.reminder import Reminder

    plays: list[int] = []
    r = Reminder(True, 3600.0, 60.0, None, play=lambda: plays.append(1))
    t = 1000.0
    r.start(t)  # next reminder at t + 3600
    r.tick(t + 3600, suppressed=True)  # due mid-game -> queued, no sound
    check(len(plays) == 0, "queued (silent) while in-game")
    r.tick(t + 5400, suppressed=True)  # another due mid-game -> still just one queued
    check(len(plays) == 0, "stays queued through a long session")
    # Game ends at t+7000 (before the next hourly mark at t+7200), so the queued
    # reminder takes the deferred path rather than a fresh regular due.
    r.tick(t + 7000, suppressed=False)  # game just ended -> start the defer countdown
    check(len(plays) == 0, "not played immediately on game close (waits defer delay)")
    r.tick(t + 7000 + 59, suppressed=False)
    check(len(plays) == 0, "still waiting out the defer delay")
    r.tick(t + 7000 + 60, suppressed=False)  # delay elapsed -> plays once
    check(len(plays) == 1, "queued reminder plays once after the defer delay")
    r.tick(t + 7000 + 120, suppressed=False)  # not due again yet, nothing pending
    check(len(plays) == 1, "does not replay a queued reminder")


def test_chime_synthesis_produces_valid_wav() -> None:
    print("test_chime_synthesis_produces_valid_wav")
    import wave as wavemod

    from deadlock_capture.reminder import synthesize_chime

    with tempfile.TemporaryDirectory() as d:
        tones = ((1047, 150), (784, 250))  # 400 ms total
        out = Path(d) / "chime.wav"
        synthesize_chime(tones, out)
        check(out.exists() and not out.with_suffix(".tmp").exists(), "wav written, temp cleaned up")
        with wavemod.open(str(out), "rb") as w:
            check(w.getnchannels() == 1 and w.getsampwidth() == 2, "16-bit mono PCM")
            check(w.getframerate() == 44100, "44.1 kHz sample rate")
            expected = int(44100 * 0.4)
            check(abs(w.getnframes() - expected) <= 2, "frame count matches total duration")
            # Endpoints are silent (attack fade-in / release fade-out -> no clicks).
            first = struct.unpack("<h", w.readframes(1))[0]
            check(abs(first) < 200, "starts near silence (soft attack, no click)")


def test_reminder_disabled_never_plays() -> None:
    print("test_reminder_disabled_never_plays")
    from deadlock_capture.reminder import Reminder

    plays: list[int] = []
    r = Reminder(False, 1.0, 1.0, None, play=lambda: plays.append(1))
    r.start(0.0)
    r.tick(1000.0, suppressed=False)
    check(plays == [], "disabled reminder never plays")


def test_find_newest_helper() -> None:
    print("test_find_newest_helper")
    with tempfile.TemporaryDirectory() as d:
        from deadlock_capture.recorder import find_newest_video_newer_than

        before = time.time() - 1  # clearly in the past vs. the file we create
        p = Path(d) / "rec.mp4"
        p.write_bytes(b"x")
        found = find_newest_video_newer_than(d, before)
        check(found is not None and found.name == "rec.mp4", "finds video newer than threshold")
        check(find_newest_video_newer_than(d, time.time() + 100) is None, "ignores videos older than threshold")


def main() -> int:
    tests = [
        test_parser,
        test_state_machine_records_real_match,
        test_state_machine_retries_until_success,
        test_no_stray_send_when_nothing_recording,
        test_state_machine_skips_non_recordable_map,
        test_game_exit_mid_recording,
        test_stop_is_delayed_after_match_end,
        test_game_exit_flushes_pending_stop,
        test_recorder_no_ack_is_treated_as_sent,
        test_focus_on_match_start,
        test_refocuses_before_each_start_attempt,
        test_build_recorder,
        test_cefquery_expression_builds_valid_ipc_json,
        test_tailer_truncation,
        test_tailer_partial_line_buffering,
        test_cleanup,
        test_chime_synthesis_produces_valid_wav,
        test_reminder_plays_when_not_in_game,
        test_reminder_queues_during_game_and_plays_after_delay,
        test_reminder_disabled_never_plays,
        test_find_newest_helper,
    ]
    for t in tests:
        t()
    print(f"\nAll checks passed ({_passed} assertions).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
