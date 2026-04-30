from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
import os
import tomllib
from typing import Any

APP_NAME = "murmur"


def _xdg_dir(env_name: str, fallback: Path) -> Path:
    raw = os.environ.get(env_name)
    if raw:
        return Path(raw).expanduser()
    return fallback


def config_dir() -> Path:
    return _xdg_dir("XDG_CONFIG_HOME", Path.home() / ".config") / APP_NAME


def state_dir() -> Path:
    return _xdg_dir("XDG_STATE_HOME", Path.home() / ".local" / "state") / APP_NAME


def cache_dir() -> Path:
    return _xdg_dir("XDG_CACHE_HOME", Path.home() / ".cache") / APP_NAME


def default_config_path() -> Path:
    return config_dir() / "config.toml"


@dataclass(frozen=True)
class PathsConfig:
    state_dir: Path = field(default_factory=state_dir)
    history_db: Path | None = None
    model_dir: Path = field(default_factory=lambda: cache_dir() / "models")
    debug_audio_dir: Path = field(default_factory=lambda: cache_dir() / "audio")

    @property
    def history_path(self) -> Path:
        return self.history_db or (self.state_dir / "history.sqlite3")


@dataclass(frozen=True)
class SttConfig:
    provider: str = "faster-whisper"
    model: str = "base.en"
    language: str = "auto"
    whisper_cpp_binary: str = "whisper-cli"
    whisper_cpp_model: Path | None = None


@dataclass(frozen=True)
class CleanupConfig:
    default_mode: str = "clean"


@dataclass(frozen=True)
class InsertionConfig:
    clipboard_tool: str = "wl-copy"
    paste_simulator: str = "wtype"
    fallback_paste_simulator: str = "ydotool"
    paste_tool: str = "wtype"
    press_enter_phrase: bool = True


@dataclass(frozen=True)
class PrivacyConfig:
    history: bool = True
    keep_debug_audio: bool = False
    keep_audio_days: int = 0


@dataclass(frozen=True)
class MurmurConfig:
    config_path: Path
    paths: PathsConfig = field(default_factory=PathsConfig)
    stt: SttConfig = field(default_factory=SttConfig)
    cleanup: CleanupConfig = field(default_factory=CleanupConfig)
    insertion: InsertionConfig = field(default_factory=InsertionConfig)
    privacy: PrivacyConfig = field(default_factory=PrivacyConfig)


def _path(value: Any) -> Path | None:
    if value in (None, ""):
        return None
    return Path(str(value)).expanduser()


def _merge_paths(data: dict[str, Any]) -> PathsConfig:
    defaults = PathsConfig()
    return PathsConfig(
        state_dir=_path(data.get("state_dir")) or defaults.state_dir,
        history_db=_path(data.get("history_db")),
        model_dir=_path(data.get("model_dir")) or defaults.model_dir,
        debug_audio_dir=_path(data.get("debug_audio_dir")) or defaults.debug_audio_dir,
    )


def _merge_stt(data: dict[str, Any]) -> SttConfig:
    defaults = SttConfig()
    return SttConfig(
        provider=str(data.get("provider", defaults.provider)),
        model=str(data.get("model", defaults.model)),
        language=str(data.get("language", defaults.language)),
        whisper_cpp_binary=str(data.get("whisper_cpp_binary", defaults.whisper_cpp_binary)),
        whisper_cpp_model=_path(data.get("whisper_cpp_model")),
    )


def _merge_cleanup(data: dict[str, Any]) -> CleanupConfig:
    defaults = CleanupConfig()
    return CleanupConfig(default_mode=str(data.get("default_mode", defaults.default_mode)))


def _merge_insertion(data: dict[str, Any]) -> InsertionConfig:
    defaults = InsertionConfig()
    paste_tool = str(data.get("paste_tool", data.get("paste_simulator", defaults.paste_tool)))
    return InsertionConfig(
        clipboard_tool=str(data.get("clipboard_tool", defaults.clipboard_tool)),
        paste_simulator=str(data.get("paste_simulator", paste_tool)),
        fallback_paste_simulator=str(data.get("fallback_paste_simulator", defaults.fallback_paste_simulator)),
        paste_tool=paste_tool,
        press_enter_phrase=bool(data.get("press_enter_phrase", defaults.press_enter_phrase)),
    )


def _merge_privacy(data: dict[str, Any]) -> PrivacyConfig:
    defaults = PrivacyConfig()
    return PrivacyConfig(
        history=bool(data.get("history", defaults.history)),
        keep_debug_audio=bool(data.get("keep_debug_audio", defaults.keep_debug_audio)),
        keep_audio_days=int(data.get("keep_audio_days", defaults.keep_audio_days)),
    )


def load_config(path: str | Path | None = None) -> MurmurConfig:
    cfg_path = Path(path).expanduser() if path else default_config_path()
    raw: dict[str, Any] = {}
    if cfg_path.exists():
        with cfg_path.open("rb") as f:
            raw = tomllib.load(f)
    return MurmurConfig(
        config_path=cfg_path,
        paths=_merge_paths(raw.get("paths", {})),
        stt=_merge_stt(raw.get("stt", {})),
        cleanup=_merge_cleanup(raw.get("cleanup", {})),
        insertion=_merge_insertion(raw.get("insertion", {})),
        privacy=_merge_privacy(raw.get("privacy", {})),
    )


def ensure_local_dirs(config: MurmurConfig) -> None:
    config.paths.state_dir.mkdir(parents=True, exist_ok=True)
    config.paths.history_path.parent.mkdir(parents=True, exist_ok=True)
    config.paths.model_dir.mkdir(parents=True, exist_ok=True)
    if config.privacy.keep_debug_audio:
        config.paths.debug_audio_dir.mkdir(parents=True, exist_ok=True)


def sample_config() -> str:
    defaults = load_config(Path("/nonexistent/murmur-defaults.toml"))
    return f'''# Murmur local configuration. Store secrets only in your shell environment, not here.

[paths]
# state_dir = "{defaults.paths.state_dir}"
# history_db = "{defaults.paths.history_path}"
# model_dir = "{defaults.paths.model_dir}"
# debug_audio_dir = "{defaults.paths.debug_audio_dir}"

[stt]
# Local/free first. Supported MVP values: "faster-whisper" or "whisper-cpp".
provider = "{defaults.stt.provider}"
model = "{defaults.stt.model}"
language = "{defaults.stt.language}"
# whisper_cpp_binary = "{defaults.stt.whisper_cpp_binary}"
# whisper_cpp_model = "{defaults.paths.model_dir / 'ggml-base.en.bin'}"

[cleanup]
default_mode = "{defaults.cleanup.default_mode}" # "clean" or "raw"

[insertion]
clipboard_tool = "{defaults.insertion.clipboard_tool}"
paste_tool = "{defaults.insertion.paste_tool}"
paste_simulator = "{defaults.insertion.paste_simulator}"
fallback_paste_simulator = "{defaults.insertion.fallback_paste_simulator}"
press_enter_phrase = true

[privacy]
history = true
keep_debug_audio = false
keep_audio_days = 0
'''


def write_default_config(path: str | Path | None = None, *, overwrite: bool = False) -> Path:
    cfg_path = Path(path).expanduser() if path else default_config_path()
    if cfg_path.exists() and not overwrite:
        return cfg_path
    cfg_path.parent.mkdir(parents=True, exist_ok=True)
    cfg_path.write_text(sample_config(), encoding="utf-8")
    return cfg_path
