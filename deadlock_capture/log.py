"""Logging to both a rotating file (capture.log) and stderr.

Because the daemon normally runs windowless under pythonw, stderr goes nowhere
— the file is the only way to see what happened. capture.log sits next to
config.toml.
"""

from __future__ import annotations

import logging
import os
import sys
from logging.handlers import RotatingFileHandler
from pathlib import Path

# Override the log location with DEADLOCK_CAPTURE_LOG (used by tests so they
# never write into the production capture.log).
_env_path = os.environ.get("DEADLOCK_CAPTURE_LOG")
LOG_PATH = Path(_env_path) if _env_path else Path(__file__).resolve().parent.parent / "capture.log"

_logger = logging.getLogger("deadlock_capture")


def _setup() -> None:
    if _logger.handlers:
        return
    _logger.setLevel(logging.INFO)
    fmt = logging.Formatter(
        "%(asctime)s %(levelname)-5s %(message)s", datefmt="%Y-%m-%d %H:%M:%S"
    )
    try:
        fh = RotatingFileHandler(
            LOG_PATH, maxBytes=2_000_000, backupCount=3, encoding="utf-8"
        )
        fh.setFormatter(fmt)
        _logger.addHandler(fh)
    except OSError:
        pass  # if the file can't be opened, still log to stderr
    sh = logging.StreamHandler(sys.stderr)
    sh.setFormatter(fmt)
    _logger.addHandler(sh)


_setup()


def _join(parts: tuple[object, ...]) -> str:
    return " ".join(str(p) for p in parts)


def info(*parts: object) -> None:
    _logger.info(_join(parts))


def warn(*parts: object) -> None:
    _logger.warning(_join(parts))


def error(*parts: object) -> None:
    _logger.error(_join(parts))


def exception(msg: str) -> None:
    """Log msg with the current exception's traceback."""
    _logger.exception(msg)
