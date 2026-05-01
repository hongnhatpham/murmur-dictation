from __future__ import annotations

import json
import time
from pathlib import Path


def status_path(state_dir: Path) -> Path:
    return state_dir / "status.json"


def write_status(state_dir: Path, state: str, message: str = "", *, ttl_seconds: float = 2.0) -> None:
    try:
        state_dir.mkdir(parents=True, exist_ok=True)
        payload = {
            "state": state,
            "message": message,
            "updated_at": time.time(),
            "expires_at": time.time() + ttl_seconds if ttl_seconds > 0 else 0,
        }
        status_path(state_dir).write_text(json.dumps(payload), encoding="utf-8")
    except Exception:
        pass


def clear_status(state_dir: Path) -> None:
    try:
        status_path(state_dir).unlink(missing_ok=True)
    except Exception:
        pass
