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
    personal_db: Path | None = None
    model_dir: Path = field(default_factory=lambda: cache_dir() / "models")
    debug_audio_dir: Path = field(default_factory=lambda: cache_dir() / "audio")

    @property
    def history_path(self) -> Path:
        return self.history_db or (self.state_dir / "history.sqlite3")

    @property
    def personal_path(self) -> Path:
        return self.personal_db or (self.state_dir / "personal.sqlite3")


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
    auto_leading_space: bool = True
    terminal_app_ids: tuple[str, ...] = (
        "Alacritty",
        "alacritty",
        "kitty",
        "foot",
        "footclient",
        "org.wezfurlong.wezterm",
        "WezTerm",
        "com.mitchellh.ghostty",
        "ghostty",
        "org.gnome.Terminal",
        "konsole",
    )


@dataclass(frozen=True)
class StyleConfig:
    presets: dict[str, dict[str, Any]] = field(default_factory=lambda: {
        "terminal": {"trailing_period": False, "rewrite_aggressiveness": "literal", "formality": "literal"},
        "code": {"trailing_period": False, "rewrite_aggressiveness": "literal", "formality": "literal"},
        "personal_message": {"trailing_period": False, "rewrite_aggressiveness": "light", "formality": "casual"},
        "work_message": {"trailing_period": False, "rewrite_aggressiveness": "light", "formality": "casual-professional"},
        "email": {"trailing_period": True, "rewrite_aggressiveness": "medium", "formality": "formal"},
        "docs": {"trailing_period": True, "rewrite_aggressiveness": "medium", "formality": "clear"},
        "other": {"trailing_period": True, "rewrite_aggressiveness": "light", "formality": "neutral"},
    })


@dataclass(frozen=True)
class CorrectionConfig:
    enabled: bool = False
    provider: str = "ollama"
    model: str = "llama3.2:3b"
    endpoint: str = "http://127.0.0.1:11434/api/generate"
    timeout_seconds: float = 2.5
    raw_mode: bool = False
    skip_categories: tuple[str, ...] = ()
    skip_below_duration_ms: int = 0


@dataclass(frozen=True)
class CommandConfig:
    max_selection_chars: int = 12000


@dataclass(frozen=True)
class ContextConfig:
    app_categories: dict[str, str] = field(default_factory=lambda: {
        "Alacritty": "terminal",
        "alacritty": "terminal",
        "kitty": "terminal",
        "foot": "terminal",
        "footclient": "terminal",
        "org.wezfurlong.wezterm": "terminal",
        "WezTerm": "terminal",
        "com.mitchellh.ghostty": "terminal",
        "ghostty": "terminal",
        "org.gnome.Terminal": "terminal",
        "konsole": "terminal",
        "Code": "code",
        "code": "code",
        "codium": "code",
        "discord": "personal_message",
        "Slack": "work_message",
        "signal": "personal_message",
        "thunderbird": "email",
    })


@dataclass(frozen=True)
class PrivacyConfig:
    history: bool = True
    keep_debug_audio: bool = False
    keep_audio_days: int = 1


@dataclass(frozen=True)
class MurmurConfig:
    config_path: Path
    paths: PathsConfig = field(default_factory=PathsConfig)
    stt: SttConfig = field(default_factory=SttConfig)
    cleanup: CleanupConfig = field(default_factory=CleanupConfig)
    styles: StyleConfig = field(default_factory=StyleConfig)
    correction: CorrectionConfig = field(default_factory=CorrectionConfig)
    command: CommandConfig = field(default_factory=CommandConfig)
    insertion: InsertionConfig = field(default_factory=InsertionConfig)
    context: ContextConfig = field(default_factory=ContextConfig)
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
        personal_db=_path(data.get("personal_db")),
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


def _merge_styles(data: dict[str, Any]) -> StyleConfig:
    defaults = StyleConfig()
    raw = data.get("presets", defaults.presets)
    if not isinstance(raw, dict):
        raw = defaults.presets
    presets: dict[str, dict[str, Any]] = {}
    for category, style in raw.items():
        if isinstance(style, dict):
            presets[str(category)] = {str(key): value for key, value in style.items()}
    return StyleConfig(presets=presets or defaults.presets)


def _merge_correction(data: dict[str, Any]) -> CorrectionConfig:
    defaults = CorrectionConfig()
    raw_skip_categories = data.get("skip_categories", defaults.skip_categories)
    skip_categories = tuple(str(item) for item in raw_skip_categories) if isinstance(raw_skip_categories, list | tuple) else defaults.skip_categories
    return CorrectionConfig(
        enabled=bool(data.get("enabled", defaults.enabled)),
        provider=str(data.get("provider", defaults.provider)),
        model=str(data.get("model", defaults.model)),
        endpoint=str(data.get("endpoint", defaults.endpoint)),
        timeout_seconds=float(data.get("timeout_seconds", defaults.timeout_seconds)),
        raw_mode=bool(data.get("raw_mode", defaults.raw_mode)),
        skip_categories=skip_categories,
        skip_below_duration_ms=int(data.get("skip_below_duration_ms", defaults.skip_below_duration_ms)),
    )


def _merge_command(data: dict[str, Any]) -> CommandConfig:
    defaults = CommandConfig()
    return CommandConfig(max_selection_chars=int(data.get("max_selection_chars", defaults.max_selection_chars)))


def _merge_insertion(data: dict[str, Any]) -> InsertionConfig:
    defaults = InsertionConfig()
    paste_tool = str(data.get("paste_tool", data.get("paste_simulator", defaults.paste_tool)))
    terminal_app_ids_raw = data.get("terminal_app_ids", defaults.terminal_app_ids)
    terminal_app_ids = tuple(str(item) for item in terminal_app_ids_raw) if isinstance(terminal_app_ids_raw, list | tuple) else defaults.terminal_app_ids
    return InsertionConfig(
        clipboard_tool=str(data.get("clipboard_tool", defaults.clipboard_tool)),
        paste_simulator=str(data.get("paste_simulator", paste_tool)),
        fallback_paste_simulator=str(data.get("fallback_paste_simulator", defaults.fallback_paste_simulator)),
        paste_tool=paste_tool,
        press_enter_phrase=bool(data.get("press_enter_phrase", defaults.press_enter_phrase)),
        auto_leading_space=bool(data.get("auto_leading_space", defaults.auto_leading_space)),
        terminal_app_ids=terminal_app_ids,
    )


def _merge_context(data: dict[str, Any]) -> ContextConfig:
    defaults = ContextConfig()
    raw = data.get("app_categories", defaults.app_categories)
    if not isinstance(raw, dict):
        raw = defaults.app_categories
    return ContextConfig(app_categories={str(key): str(value) for key, value in raw.items()})


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
        styles=_merge_styles(raw.get("styles", {})),
        correction=_merge_correction(raw.get("correction", {})),
        command=_merge_command(raw.get("command", {})),
        insertion=_merge_insertion(raw.get("insertion", {})),
        context=_merge_context(raw.get("context", {})),
        privacy=_merge_privacy(raw.get("privacy", {})),
    )


def ensure_local_dirs(config: MurmurConfig) -> None:
    config.paths.state_dir.mkdir(parents=True, exist_ok=True)
    config.paths.history_path.parent.mkdir(parents=True, exist_ok=True)
    config.paths.personal_path.parent.mkdir(parents=True, exist_ok=True)
    config.paths.model_dir.mkdir(parents=True, exist_ok=True)
    if config.privacy.keep_debug_audio:
        config.paths.debug_audio_dir.mkdir(parents=True, exist_ok=True)


def sample_config() -> str:
    defaults = load_config(Path("/nonexistent/murmur-defaults.toml"))
    return f'''# Murmur local configuration. Store secrets only in your shell environment, not here.

[paths]
# state_dir = "{defaults.paths.state_dir}"
# history_db = "{defaults.paths.history_path}"
# personal_db = "{defaults.paths.personal_path}"
# model_dir = "{defaults.paths.model_dir}"
# debug_audio_dir = "{defaults.paths.debug_audio_dir}"

[stt]
# Near-instant profiles target only local STT and Groq:
#   murmur provider-profile local
#   murmur provider-profile groq
# Supported STT values: "faster-whisper", "whisper-cpp", "groq", or legacy "elevenlabs".
# Groq requires GROQ_API_KEY/MURMUR_GROQ_API_KEY or ~/.config/murmur/groq_api_key.
# ElevenLabs requires ELEVENLABS_API_KEY/MURMUR_ELEVENLABS_API_KEY or ~/.config/murmur/elevenlabs_api_key.
provider = "{defaults.stt.provider}"
model = "{defaults.stt.model}"
language = "{defaults.stt.language}"
# whisper_cpp_binary = "{defaults.stt.whisper_cpp_binary}"
# whisper_cpp_model = "{defaults.paths.model_dir / 'ggml-base.en.bin'}"
# Groq example:
# provider = "groq"
# model = "whisper-large-v3-turbo"
# ElevenLabs example:
# provider = "elevenlabs"
# model = "scribe_v2"

[cleanup]
default_mode = "{defaults.cleanup.default_mode}" # "clean" or "raw"

[styles]
# Optional per-category style overrides.
# [styles.presets.personal_message]
# trailing_period = false
# rewrite_aggressiveness = "light"
# formality = "casual"

[correction]
# Optional local LLM cleanup. Disabled by default until latency/quality is proven.
enabled = false
provider = "{defaults.correction.provider}"
model = "{defaults.correction.model}"
endpoint = "{defaults.correction.endpoint}"
timeout_seconds = {defaults.correction.timeout_seconds}
raw_mode = false
# Optional latency policy: skip AI correction where STT is usually enough.
skip_categories = []
skip_below_duration_ms = 0

[command]
max_selection_chars = {defaults.command.max_selection_chars}

[insertion]
clipboard_tool = "{defaults.insertion.clipboard_tool}"
paste_tool = "{defaults.insertion.paste_tool}"
paste_simulator = "{defaults.insertion.paste_simulator}"
fallback_paste_simulator = "{defaults.insertion.fallback_paste_simulator}"
press_enter_phrase = true
auto_leading_space = true
terminal_app_ids = {list(defaults.insertion.terminal_app_ids)!r}

[context]
# Maps focused app IDs to correction/style categories.
# app_categories = {{"Alacritty" = "terminal", "Code" = "code", "Slack" = "work_message"}}

[privacy]
history = true
keep_debug_audio = false
keep_audio_days = 1
'''


def write_default_config(path: str | Path | None = None, *, overwrite: bool = False) -> Path:
    cfg_path = Path(path).expanduser() if path else default_config_path()
    if cfg_path.exists() and not overwrite:
        return cfg_path
    cfg_path.parent.mkdir(parents=True, exist_ok=True)
    cfg_path.write_text(sample_config(), encoding="utf-8")
    return cfg_path
