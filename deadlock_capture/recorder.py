"""Drive NVIDIA ShadowPlay via the NVIDIA App's IPC, and name the resulting file.

Recording is started/stopped by sending ShadowPlay's own control command through
the NVIDIA overlay (see nvipc.py) — not a simulated keypress. We still confirm a
recording by watching for a new video file to appear in the recordings folder,
then rename that file (ShadowPlay's default name) to the match id.

A command that's sent but not acked is treated as "sent" (the file-watch is the
real confirmation). A command that outright fails is logged; the caller's
start-retry loop will try again on its next interval.
"""

from __future__ import annotations

import os
import re
import time
from pathlib import Path

from . import log
from .nvipc import IpcError, IpcNoAck, NvOverlayIpc

_VIDEO_EXTS = {".mp4", ".mkv", ".mov"}

# rename stable-file wait
_STABLE_S = 1.0
_RENAME_POLL_S = 0.5
_RENAME_MAX_WAIT_S = 30.0


def find_newest_video_newer_than(folder: str, since_ts: float) -> Path | None:
    """Newest video file in ``folder`` whose mtime is >= ``since_ts``."""
    best: Path | None = None
    best_mtime = -1.0
    try:
        entries = os.scandir(folder)
    except OSError:
        return None
    with entries:
        for e in entries:
            if e.name[e.name.rfind(".") :].lower() not in _VIDEO_EXTS:
                continue
            try:
                st = e.stat()
            except OSError:
                continue
            if not e.is_file() or st.st_mtime < since_ts:
                continue
            if st.st_mtime > best_mtime:
                best, best_mtime = Path(e.path), st.st_mtime
    return best


def _sanitize(s: str) -> str:
    s = re.sub(r"[^A-Za-z0-9 _\-.]", "_", s)
    s = re.sub(r"\s{2,}", " ", s)
    return s.strip()


class Recorder:
    """Starts/stops a ShadowPlay recording over the overlay IPC and renames the
    resulting file to the match id."""

    def __init__(
        self,
        folder: str,
        rename_template: str,
        ipc: NvOverlayIpc,
        *,
        system: str,
        module: str,
        start_method: str,
        start_payload: dict,
        stop_method: str,
        stop_payload: dict,
    ) -> None:
        self.folder = folder
        self.rename_template = rename_template
        self.ipc = ipc
        self.system = system
        self.module = module
        self.start_method = start_method
        self.start_payload = start_payload
        self.stop_method = stop_method
        self.stop_payload = stop_payload

    # Overridable so tests can swap in a no-op that just records the call.
    def _send(self, method: str, payload: dict, what: str) -> bool:
        """Send one ShareServer command. Returns True if it was accepted (or
        sent-without-ack), False if it outright failed."""
        try:
            self.ipc.send_share_command(method, payload, self.system, self.module)
            log.info(f"IPC {what} ok ({method})")
            return True
        except IpcNoAck as err:
            # Sent, just not acked — assume it took effect; the file-watch confirms.
            log.info(f"IPC {what} sent, no ack ({method}): {err}")
            return True
        except IpcError as err:
            log.warn(f"IPC {what} failed ({method}): {err}")
            return False

    def press(self) -> None:
        """Send the record-start command once (non-blocking). The caller decides
        when to send and confirms the result via has_recording_since()."""
        self._send(self.start_method, self.start_payload, "start")

    def has_recording_since(self, since_ts: float) -> bool:
        """True if a recording file newer than ``since_ts`` exists — our proof
        that ShadowPlay is actually recording."""
        return find_newest_video_newer_than(self.folder, since_ts) is not None

    def stop(self, recording: bool, since_ts: float) -> bool:
        """Send the stop-and-save command. ``recording`` is our belief about
        whether a recording is in progress; if False we re-check the folder in
        case one was started manually, so we don't wait pointlessly for a file
        that will never appear. Returns True if we sent the stop command."""
        if not recording:
            f = find_newest_video_newer_than(self.folder, since_ts)
            if f:
                recording = True
                log.info(f"recording detected from manual start ({f})")
        if not recording:
            log.info("no recording in progress; nothing to stop")
            return False
        self._send(self.stop_method, self.stop_payload, "stop")
        return True

    def _wait_for_stable(self, since_ts: float) -> Path | None:
        deadline = time.monotonic() + _RENAME_MAX_WAIT_S
        stable_since: float | None = None
        last_path: Path | None = None
        last_size = -1
        while time.monotonic() < deadline:
            c = find_newest_video_newer_than(self.folder, since_ts)
            if c is not None:
                try:
                    size = c.stat().st_size
                except OSError:
                    size = -1
                if c != last_path or size != last_size:
                    last_path, last_size, stable_since = c, size, time.monotonic()
                elif stable_since is not None and time.monotonic() - stable_since >= _STABLE_S:
                    return c
            time.sleep(_RENAME_POLL_S)
        return None

    def rename(self, match_id: str, since_ts: float) -> Path | None:
        """Wait for the new recording to stop growing, then rename it from
        ShadowPlay's default name to the match id."""
        candidate = self._wait_for_stable(since_ts)
        if candidate is None:
            log.warn(f"no new recording found in {self.folder} within {_RENAME_MAX_WAIT_S:.0f}s")
            return None

        base = _sanitize(self.rename_template.replace("{matchId}", match_id))
        if not base:
            log.warn("rename template resolved to empty string; leaving file unchanged")
            return None

        target = candidate.with_name(f"{base}{candidate.suffix}")
        if target == candidate:
            return target
        try:
            candidate.rename(target)
            return target
        except OSError as err:
            log.warn(f"rename failed: {err}")
            return None
