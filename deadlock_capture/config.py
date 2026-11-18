"""Load and validate config.toml."""

from __future__ import annotations

import tomllib
from dataclasses import dataclass, field
from pathlib import Path

from .reminder import DEFAULT_TONES


@dataclass(frozen=True)
class Config:
    process: str
    console_log: str
    recordings_folder: str
    max_gb: float
    exit_delay_s: float
    recordable_maps: tuple[str, ...]
    rename_template: str
    # Recording-start retry. After a recordable map loads we wait start_delay_s
    # (so the fullscreen load transition is over), then press the record hotkey
    # and watch for a video file. If none appears we keep re-pressing every
    # start_attempt_interval_s for up to start_window_s total, then give up.
    # Every attempt first brings the game back to the foreground (when
    # focus_on_match_start is set), since ShadowPlay only records the active game.
    start_delay_s: float = 4.0
    start_window_s: float = 120.0
    start_attempt_interval_s: float = 15.0
    # After a match ends, keep recording this long before sending the stop
    # command — captures the post-match screen. 0 = stop immediately.
    stop_delay_s: float = 10.0
    # When a match starts, pull the game window to the foreground (you're
    # probably alt-tabbed while queueing) — and again before each record-start
    # attempt, until the recording is confirmed.
    focus_on_match_start: bool = True
    # ShadowPlay is driven through the NVIDIA App's CEF IPC (see nvipc.py),
    # configured under the [ipc] table in config.toml.
    ipc_host: str = "127.0.0.1"
    ipc_debug_port: int = 49222
    ipc_connect_timeout_s: float = 2.0
    ipc_call_timeout_s: float = 5.0
    ipc_system: str = "CrimsonNative"
    ipc_module: str = "ShareServer"
    ipc_start_method: str = "EnableRecord"
    ipc_stop_method: str = "EnableRecord"
    ipc_start_payload: dict = field(default_factory=lambda: {"enable": True})
    ipc_stop_payload: dict = field(default_factory=lambda: {"enable": False})
    # Periodic break reminders, signalled by a notification sound. Suppressed
    # whenever the game is running; a reminder due mid-game is queued and plays
    # reminder_defer_delay_s after the game closes. Configured under the
    # [reminders] table in config.toml.
    reminders_enabled: bool = True
    reminder_interval_s: float = 3600.0
    reminder_defer_delay_s: float = 60.0
    # By default a distinct synthesized chime (DEFAULT_TONES) plays. Set
    # reminder_sound_file to a .wav to use that instead; it takes precedence.
    reminder_sound_file: str = ""
    reminder_tones: tuple[tuple[int, int], ...] = DEFAULT_TONES


_DEFAULT_PATH = Path(__file__).resolve().parent.parent / "config.toml"


def load(path: Path | None = None) -> Config:
    p = path or _DEFAULT_PATH
    with p.open("rb") as f:
        raw = tomllib.load(f)

    required = (
        "process",
        "console_log",
        "recordings_folder",
        "max_gb",
        "exit_delay_s",
        "recordable_maps",
        "rename_template",
    )
    missing = [k for k in required if k not in raw]
    if missing:
        raise ValueError(f"{p}: missing required key(s): {', '.join(missing)}")

    rem = raw.get("reminders", {})
    ipc = raw.get("ipc", {})
    return Config(
        process=str(raw["process"]),
        console_log=str(raw["console_log"]),
        recordings_folder=str(raw["recordings_folder"]),
        max_gb=float(raw["max_gb"]),
        exit_delay_s=float(raw["exit_delay_s"]),
        recordable_maps=tuple(str(m) for m in raw["recordable_maps"]),
        rename_template=str(raw["rename_template"]),
        start_delay_s=float(raw.get("start_delay_s", 4.0)),
        start_window_s=float(raw.get("start_window_s", 120.0)),
        start_attempt_interval_s=float(raw.get("start_attempt_interval_s", 15.0)),
        stop_delay_s=float(raw.get("stop_delay_s", 10.0)),
        focus_on_match_start=bool(raw.get("focus_on_match_start", True)),
        ipc_host=str(ipc.get("host", "127.0.0.1")),
        ipc_debug_port=int(ipc.get("debug_port", 49222)),
        ipc_connect_timeout_s=float(ipc.get("connect_timeout_s", 2.0)),
        ipc_call_timeout_s=float(ipc.get("call_timeout_s", 5.0)),
        ipc_system=str(ipc.get("system", "CrimsonNative")),
        ipc_module=str(ipc.get("module", "ShareServer")),
        ipc_start_method=str(ipc.get("start_method", "EnableRecord")),
        ipc_stop_method=str(ipc.get("stop_method", "EnableRecord")),
        ipc_start_payload=dict(ipc.get("start_payload", {"enable": True})),
        ipc_stop_payload=dict(ipc.get("stop_payload", {"enable": False})),
        reminders_enabled=bool(rem.get("enabled", True)),
        reminder_interval_s=float(rem.get("interval_s", 3600.0)),
        reminder_defer_delay_s=float(rem.get("defer_delay_s", 60.0)),
        reminder_sound_file=str(rem.get("sound_file", "")),
        reminder_tones=(
            tuple((int(f), int(ms)) for f, ms in rem["tones"])
            if "tones" in rem
            else DEFAULT_TONES
        ),
    )
