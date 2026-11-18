"""Periodic break reminders, signalled by a short notification *sound* — a nudge
to stretch, look away, drink water, whatever.

The capture daemon already knows when you're in the game / recording, so the
reminder is suppressed during those times — it never plays mid-match. A reminder
that comes due while you're playing is *queued*: once the game has been closed
for ``defer_delay_s`` it plays once (no matter how many were due during the
session — they collapse into a single nudge).

Driven by tick(now, suppressed) from the main loop, same as the recorder's
start-retry.
"""

from __future__ import annotations

import math
import os
import struct
import subprocess
import tempfile
import wave
from pathlib import Path
from typing import Callable

from . import log

# Hide the PowerShell window so nothing flashes.
_NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)

# A short, soft descending chime — two warm notes (a falling fifth) that resolve
# downward, so it's pleasant and unmistakably ours, not a Windows system sound.
# It's rendered as real sine tones with a gentle decay (see synthesize_chime),
# NOT square-wave beeps, so it sounds like a soft glockenspiel rather than 8-bit.
# Each pair is (frequency_hz, duration_ms), played in order. Tweak via
# [reminders].tones.
DEFAULT_TONES: tuple[tuple[int, int], ...] = (
    (1047, 150),  # C6
    (784, 250),   # G5 — soft resolve
)

_SAMPLE_RATE = 44100
_AMPLITUDE = 28000  # 16-bit peak headroom (max 32767)


def _ps_quote(s: str) -> str:
    """Quote a string as a PowerShell single-quoted literal."""
    return "'" + s.replace("'", "''") + "'"


def synthesize_chime(tones: tuple[tuple[int, int], ...], path: Path) -> None:
    """Render ``tones`` to a 16-bit mono WAV at ``path``. Each note is a sine
    fundamental plus two softer harmonics (a warm, bell-like timbre) shaped by a
    quick attack and exponential decay, with a short release fade so notes don't
    click into each other. The result is smooth and easy on the ear."""
    frames = bytearray()
    for freq, ms in tones:
        dur = ms / 1000.0
        n = int(_SAMPLE_RATE * dur)
        for i in range(n):
            t = i / _SAMPLE_RATE
            attack = min(1.0, t / 0.005)            # 5 ms fade-in (no click)
            release = min(1.0, (dur - t) / 0.012)   # 12 ms fade-out at the tail
            env = attack * max(0.0, release) * math.exp(-t * 6.0)
            sample = (
                math.sin(2 * math.pi * freq * t)
                + 0.5 * math.sin(2 * math.pi * 2 * freq * t)
                + 0.25 * math.sin(2 * math.pi * 3 * freq * t)
            ) / 1.75  # keep the harmonic sum within [-1, 1]
            val = max(-1.0, min(1.0, sample * env))
            frames += struct.pack("<h", int(val * _AMPLITUDE))

    tmp = path.with_suffix(".tmp")
    with wave.open(str(tmp), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(_SAMPLE_RATE)
        w.writeframes(bytes(frames))
    os.replace(tmp, path)  # atomic: never leave a half-written wav behind


def _chime_wav(tones: tuple[tuple[int, int], ...]) -> Path:
    """Path to the rendered chime WAV, synthesizing (and caching) it on first
    use. The filename encodes the tones, so changing them re-renders."""
    sig = "_".join(f"{int(f)}-{int(ms)}" for f, ms in tones)
    path = Path(tempfile.gettempdir()) / f"deadlock_chime_{sig}.wav"
    if not path.exists():
        synthesize_chime(tones, path)
    return path


def play_sound(sound_file: str | None, tones: tuple[tuple[int, int], ...] = ()) -> None:
    """Play the reminder sound, fire-and-forget. With ``sound_file`` set we play
    that .wav; otherwise we play the synthesized chime for ``tones`` (or
    DEFAULT_TONES). Either way it's a real audio clip via SoundPlayer.PlaySync —
    a distinct, smooth cue, not a system notification. No window, no focus
    stealing — just audio."""
    target = sound_file if sound_file else str(_chime_wav(tones or DEFAULT_TONES))
    # PlaySync blocks until the clip finishes, so the process lives long enough
    # for the whole sound to come out.
    script = f"(New-Object System.Media.SoundPlayer {_ps_quote(target)}).PlaySync()"
    subprocess.Popen(
        ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", script],
        creationflags=_NO_WINDOW,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


class Reminder:
    """Plays a reminder sound every ``interval_s``, but only when not suppressed
    (in-game / recording). A reminder due while suppressed is queued and plays
    once the game has been gone for ``defer_delay_s``."""

    def __init__(
        self,
        enabled: bool,
        interval_s: float,
        defer_delay_s: float,
        sound_file: str | None,
        tones: tuple[tuple[int, int], ...] = (),
        play: Callable[[], None] | None = None,
    ) -> None:
        self.enabled = enabled
        self.interval_s = interval_s
        self.defer_delay_s = defer_delay_s
        self.sound_file = sound_file
        self.tones = tones
        self._play = play or (lambda: play_sound(self.sound_file, self.tones))
        self._next_at = 0.0  # monotonic; next scheduled reminder
        self._pending = False  # a reminder came due during a session
        self._pending_at: float | None = None  # monotonic; when the queued one may play

    def start(self, now: float) -> None:
        """Arm the first reminder one interval from now."""
        self._next_at = now + self.interval_s
        if self.enabled:
            log.info(
                f"reminders on: a break sound every {self.interval_s / 60:.0f} min when "
                f"not in-game (queued reminders play {self.defer_delay_s:.0f}s after you finish)"
            )

    def _advance(self, now: float) -> None:
        """Move to the next slot; realign if we fell behind (e.g. machine slept)
        so we never play a backlog all at once."""
        self._next_at += self.interval_s
        if self._next_at <= now:
            self._next_at = now + self.interval_s

    def _fire(self) -> None:
        try:
            self._play()
            log.info("reminder: break sound played")
        except Exception as err:  # noqa: BLE001 — never let a reminder kill the loop
            log.error(f"reminder sound failed: {err}")

    def tick(self, now: float, suppressed: bool) -> None:
        """Called every main-loop iteration."""
        if not self.enabled:
            return

        # Regular schedule. When due mid-game we queue rather than play.
        if now >= self._next_at:
            self._advance(now)
            if suppressed:
                if not self._pending:
                    self._pending = True
                    self._pending_at = None
                    log.info("reminder due mid-game — queued until you're done")
            else:
                self._fire()
                self._pending = False
                self._pending_at = None
                return

        # A queued reminder waits until the game's been closed for defer_delay_s.
        if self._pending:
            if suppressed:
                self._pending_at = None  # game still/again active — reset the wait
            elif self._pending_at is None:
                self._pending_at = now + self.defer_delay_s
                log.info(f"queued reminder will play in {self.defer_delay_s:.0f}s")
            elif now >= self._pending_at:
                self._fire()
                self._pending = False
                self._pending_at = None
