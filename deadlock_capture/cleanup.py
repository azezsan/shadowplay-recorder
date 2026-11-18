"""Keep the recordings folder under a size cap by deleting oldest videos.

Port of cleanup-recordings.ps1.
"""

from __future__ import annotations

import os
from pathlib import Path

from . import log

_VIDEO_EXTS = {".mp4", ".mkv", ".avi", ".mov", ".wmv"}


def run(folder: str, max_gb: float) -> None:
    cap_bytes = int(max_gb * 1024**3)
    files: list[tuple[Path, int, float]] = []  # (path, size, mtime)
    try:
        entries = list(os.scandir(folder))
    except OSError:
        return
    for e in entries:
        if e.name[e.name.rfind(".") :].lower() not in _VIDEO_EXTS:
            continue
        try:
            st = e.stat()
        except OSError:
            continue
        if e.is_file():
            files.append((Path(e.path), st.st_size, st.st_mtime))

    total = sum(size for _, size, _ in files)
    if total <= cap_bytes:
        return

    # Oldest first.
    files.sort(key=lambda t: t[2])
    for path, size, _ in files:
        if total <= cap_bytes:
            break
        try:
            path.unlink()
        except OSError as err:
            # Locked / in use — skip it (a future run will catch it).
            log.warn(f"cleanup: could not delete {path}: {err}")
            continue
        total -= size
        log.info(f"cleanup: deleted {path.name} ({size / 1024**3:.2f} GB)")
