from __future__ import annotations

from dataclasses import dataclass

from .config import ContextConfig
from .insertion import focused_app_id


@dataclass(frozen=True)
class AppContext:
    focused_app_id: str | None = None
    category: str = "other"


def categorize_app(app_id: str | None, config: ContextConfig) -> str:
    if not app_id:
        return "other"
    if app_id in config.app_categories:
        return config.app_categories[app_id]
    lowered = app_id.lower()
    if any(token in lowered for token in ("term", "alacritty", "kitty", "foot", "wezterm", "ghostty", "konsole")):
        return "terminal"
    if any(token in lowered for token in ("code", "codium", "zed", "sublime", "emacs", "vim")):
        return "code"
    if any(token in lowered for token in ("slack", "teams")):
        return "work_message"
    if any(token in lowered for token in ("discord", "signal", "telegram", "whatsapp")):
        return "personal_message"
    if any(token in lowered for token in ("mail", "thunderbird", "outlook")):
        return "email"
    return "other"


def current_app_context(config: ContextConfig) -> AppContext:
    app_id = focused_app_id()
    return AppContext(focused_app_id=app_id, category=categorize_app(app_id, config))
