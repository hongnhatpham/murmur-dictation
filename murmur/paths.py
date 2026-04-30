from __future__ import annotations

import os
from pathlib import Path

APP_NAME = "murmur"


def _xdg(env_name: str, fallback: Path) -> Path:
    raw = os.environ.get(env_name)
    return Path(raw).expanduser() if raw else fallback


def config_dir() -> Path:
    return _xdg("XDG_CONFIG_HOME", Path.home() / ".config") / APP_NAME


def config_file() -> Path:
    return config_dir() / "config.toml"


def data_dir() -> Path:
    return _xdg("XDG_DATA_HOME", Path.home() / ".local" / "share") / APP_NAME


def state_dir() -> Path:
    return _xdg("XDG_STATE_HOME", Path.home() / ".local" / "state") / APP_NAME


def cache_dir() -> Path:
    return _xdg("XDG_CACHE_HOME", Path.home() / ".cache") / APP_NAME


def audio_cache_dir() -> Path:
    return cache_dir() / "audio"


def history_db() -> Path:
    return state_dir() / "history.sqlite3"


def ensure_runtime_dirs() -> None:
    for path in (config_dir(), data_dir(), state_dir(), audio_cache_dir()):
        path.mkdir(parents=True, exist_ok=True)
