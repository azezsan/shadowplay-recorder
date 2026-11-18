"""Bring the game window to the foreground.

Why: matches are usually found while you're doing something else (browser,
Discord). When the log says a match has started we pull Deadlock to the front so
you don't miss the hero-pick timer — and, as a bonus, the recording starts with
the game actually on screen.

Windows only, and deliberately best-effort: the OS refuses SetForegroundWindow to
a process that has no recent input, so we do the usual dance (attach to the
foreground window's input queue, then set it) and treat failure as a no-op rather
than an error — a missed focus must never disturb the capture loop.
"""

from __future__ import annotations

import ctypes

import psutil

from . import log

SW_RESTORE = 9


def _pids_for(process_name: str) -> set[int]:
    name = process_name.lower()
    pids: set[int] = set()
    for p in psutil.process_iter(["name", "pid"]):
        try:
            if (p.info["name"] or "").lower() == name:
                pids.add(p.info["pid"])
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue
    return pids


def find_window(process_name: str) -> int | None:
    """Handle of the first visible, titled top-level window owned by
    ``process_name`` (the game's main window), or None."""
    pids = _pids_for(process_name)
    if not pids:
        return None

    # Imported here, not at module scope: ctypes.wintypes only exists on
    # Windows, and this module must stay importable elsewhere (tests).
    from ctypes import wintypes

    user32 = ctypes.windll.user32  # type: ignore[attr-defined]
    found: list[int] = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def on_window(hwnd, _lparam):  # noqa: ANN001
        if not user32.IsWindowVisible(hwnd):
            return True
        pid = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        if pid.value not in pids:
            return True
        if user32.GetWindowTextLengthW(hwnd) == 0:
            return True  # skip the invisible helper windows Source 2 creates
        found.append(hwnd)
        return False  # first match wins; stop enumerating

    user32.EnumWindows(on_window, 0)
    return found[0] if found else None


def is_foreground(process_name: str) -> bool:
    """True if ``process_name`` owns the current foreground window. Used to
    avoid stealing focus we already have. Any failure answers False — the worst
    case is one redundant (and harmless) focus attempt."""
    try:
        from ctypes import wintypes

        user32 = ctypes.windll.user32  # type: ignore[attr-defined]
        hwnd = user32.GetForegroundWindow()
        if not hwnd:
            return False
        pid = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        return pid.value in _pids_for(process_name)
    except Exception as err:  # noqa: BLE001 — focus is a nicety; never break the loop
        log.warn(f"foreground check failed: {err}")
        return False


def focus_window(hwnd: int) -> bool:
    """Restore (if minimized) and foreground ``hwnd``. Returns True if the
    window ended up in the foreground."""
    user32 = ctypes.windll.user32  # type: ignore[attr-defined]
    kernel32 = ctypes.windll.kernel32  # type: ignore[attr-defined]

    if user32.IsIconic(hwnd):
        user32.ShowWindow(hwnd, SW_RESTORE)

    # Windows only grants foreground to the process that owns the current
    # foreground window (or one with recent input). Attaching our input queue to
    # that window's thread makes us count as "the same" caller for the call.
    fg = user32.GetForegroundWindow()
    us = kernel32.GetCurrentThreadId()
    target = user32.GetWindowThreadProcessId(fg, None) if fg else 0
    attached = bool(target) and target != us and bool(user32.AttachThreadInput(us, target, True))
    try:
        user32.BringWindowToTop(hwnd)
        user32.SetForegroundWindow(hwnd)
    finally:
        if attached:
            user32.AttachThreadInput(us, target, False)

    return user32.GetForegroundWindow() == hwnd


def focus_process(process_name: str) -> bool:
    """Bring ``process_name``'s main window to the front. Never raises — logs
    and returns False if the window can't be found or focus is refused."""
    try:
        hwnd = find_window(process_name)
        if hwnd is None:
            log.warn(f"focus: no visible window found for {process_name}")
            return False
        if focus_window(hwnd):
            log.info(f"focus: {process_name} brought to the foreground")
            return True
        log.warn(f"focus: Windows refused to foreground {process_name}")
        return False
    except Exception as err:  # noqa: BLE001 — focus is a nicety; never break the loop
        log.warn(f"focus failed: {err}")
        return False
