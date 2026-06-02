from __future__ import annotations

import importlib.util
import os
import shutil
from dataclasses import dataclass
from pathlib import Path

from . import paths
from .config import MurmurConfig, config_dir


@dataclass(frozen=True)
class Check:
    name: str
    ok: bool
    detail: str
    required: bool = True


def run_checks(config: MurmurConfig) -> list[Check]:
    checks: list[Check] = []
    paths.ensure_runtime_dirs()

    checks.append(Check("config", True, str(paths.config_file())))
    checks.append(_tool("pw-record", "PipeWire audio recorder"))
    checks.append(_tool(config.insertion.clipboard_tool, "Wayland clipboard"))

    paste_tools = [config.insertion.paste_tool]
    if config.insertion.fallback_paste_simulator not in paste_tools:
        paste_tools.append(config.insertion.fallback_paste_simulator)
    paste_checks: list[Check] = []
    for paste_tool in paste_tools:
        if paste_tool in ("wtype", "ydotool"):
            paste_checks.append(_tool(paste_tool, "Wayland paste/Enter simulation", required=False))
        else:
            paste_checks.append(Check("paste tool", False, f"Unknown paste tool `{paste_tool}`", required=False))
    checks.extend(paste_checks)
    checks.append(
        Check(
            "paste simulator",
            any(check.ok for check in paste_checks),
            "at least one of wtype or ydotool is available; otherwise Murmur falls back to clipboard-only",
            required=False,
        )
    )

    model_dir = config.paths.model_dir.expanduser()
    checks.append(Check("model dir", model_dir.parent.exists(), str(model_dir), required=False))

    if config.stt.provider == "faster-whisper":
        checks.append(
            Check(
                "faster-whisper",
                importlib.util.find_spec("faster_whisper") is not None,
                "Python package for free/local Whisper models. Install with `python -m pip install faster-whisper`.",
            )
        )
    elif config.stt.provider in ("whisper-cpp", "whispercpp", "whisper.cpp"):
        checks.append(_tool(config.stt.whisper_cpp_binary, "whisper.cpp binary"))
        model = config.stt.whisper_cpp_model
        checks.append(Check("whisper.cpp model", bool(model and Path(model).expanduser().exists()), str(model or "not configured")))
    elif config.stt.provider in ("elevenlabs", "eleven-labs", "scribe"):
        key_file = Path(os.environ.get("MURMUR_ELEVENLABS_API_KEY_FILE", config_dir() / "elevenlabs_api_key")).expanduser()
        has_key = bool(os.environ.get("MURMUR_ELEVENLABS_API_KEY") or os.environ.get("ELEVENLABS_API_KEY") or key_file.exists())
        detail = "key available; value hidden" if has_key else f"Set ELEVENLABS_API_KEY, MURMUR_ELEVENLABS_API_KEY, or write {key_file}"
        checks.append(Check("elevenlabs api key", has_key, detail, required=True))
    elif config.stt.provider == "groq":
        key_file = Path(os.environ.get("MURMUR_GROQ_API_KEY_FILE", config_dir() / "groq_api_key")).expanduser()
        has_key = bool(os.environ.get("MURMUR_GROQ_API_KEY") or os.environ.get("GROQ_API_KEY") or key_file.exists())
        detail = "key available; value hidden" if has_key else f"Set GROQ_API_KEY, MURMUR_GROQ_API_KEY, or write {key_file}"
        checks.append(Check("groq api key", has_key, detail, required=True))
    else:
        checks.append(Check("stt provider", False, f"Unsupported provider `{config.stt.provider}`"))

    checks.append(Check("history db", True, str(config.paths.history_path)))
    checks.append(Check("audio cache", True, str(paths.audio_cache_dir())))
    return checks


def _tool(name: str, purpose: str, required: bool = True) -> Check:
    found = shutil.which(name)
    return Check(name, found is not None, f"{purpose}: {found or 'missing'}", required=required)


def has_required_failures(checks: list[Check]) -> bool:
    return any(check.required and not check.ok for check in checks)


def format_checks(checks: list[Check]) -> str:
    lines = []
    for check in checks:
        if check.ok:
            label = "OK"
        elif check.required:
            label = "FAIL"
        else:
            label = "WARN"
        lines.append(f"[{label}] {check.name}: {check.detail}")
    return "\n".join(lines)
