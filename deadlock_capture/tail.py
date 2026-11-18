"""Tail the Source 2 console.log and parse match events from it.

Port of the original log-tail.ts (byte-offset tailer) and log-parser.ts
(regexes). The tailer seeks to EOF on first open (no historical replay),
detects truncation on game restart (size shrank) and replays from byte 0,
and buffers partial trailing lines between polls.
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass

_READ_CHUNK = 64 * 1024


# ── Events ────────────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class MatchStarted:
    match_id: str


@dataclass(frozen=True)
class MatchEnded:
    pass


@dataclass(frozen=True)
class MapLoaded:
    map: str


Event = MatchStarted | MatchEnded | MapLoaded


# ── Parser (regexes kept verbatim from log-parser.ts) ─────────────────────────

_RX_MATCH_STARTED = re.compile(r"Lobby\s+\d+\s+for\s+Match\s+(\d+)\s+created")
_RX_LOBBY_DESTROYED = re.compile(r"Lobby\s+\d+\s+for\s+Match\s+\d+\s+destroyed")
_RX_MAP_LOADED = re.compile(r'\[Client\]\s+Map:\s+"([^"]+)"')


def parse_line(line: str) -> Event | None:
    m = _RX_MATCH_STARTED.search(line)
    if m:
        return MatchStarted(match_id=m.group(1))
    if _RX_LOBBY_DESTROYED.search(line):
        return MatchEnded()
    m = _RX_MAP_LOADED.search(line)
    if m:
        return MapLoaded(map=m.group(1))
    return None


# ── Tailer ────────────────────────────────────────────────────────────────────


@dataclass
class TailResult:
    lines: list[str]
    truncated: bool


class Tailer:
    """Returns whatever has appended since the last poll, one entry per
    ``\\n``-terminated line. ``poll()`` returns None while the file does not
    exist yet (caller should retry later)."""

    def __init__(self, path: str) -> None:
        self.path = path
        # -1 => never successfully opened. 0+ => read up to byte N of the
        # current file generation.
        self._last_size = -1
        self._pending = ""

    def _read_at(self, offset: int, end: int) -> str:
        out: list[bytes] = []
        with open(self.path, "rb") as f:
            f.seek(offset)
            pos = offset
            while pos < end:
                want = min(_READ_CHUNK, end - pos)
                got = f.read(want)
                if not got:
                    break
                out.append(got)
                pos += len(got)
        return b"".join(out).decode("utf-8", errors="replace")

    def _split_lines(self, chunk: str) -> list[str]:
        combined = self._pending + chunk
        parts = re.split(r"\r?\n", combined)
        self._pending = parts.pop() if parts else ""
        return parts

    def poll(self) -> TailResult | None:
        try:
            size = os.stat(self.path).st_size
        except OSError:
            return None

        # First successful stat — seek to current end; only handle events from
        # now on. (Truncation on next launch replays the full new session.)
        if self._last_size < 0:
            self._last_size = size
            return TailResult(lines=[], truncated=False)

        # Truncation: file rotated. Replay from the start of the new file.
        if size < self._last_size:
            self._pending = ""
            self._last_size = 0

        if size == self._last_size:
            return TailResult(lines=[], truncated=False)

        truncated = self._last_size == 0
        chunk = self._read_at(self._last_size, size)
        self._last_size = size
        return TailResult(lines=self._split_lines(chunk), truncated=truncated)
