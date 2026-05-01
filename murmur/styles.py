from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from .config import StyleConfig


@dataclass(frozen=True)
class AppliedStyle:
    name: str
    settings: dict[str, Any]


def style_for_category(category: str, config: StyleConfig) -> AppliedStyle:
    settings = config.presets.get(category) or config.presets.get("other") or {}
    return AppliedStyle(name=category if category in config.presets else "other", settings=dict(settings))


def apply_style(text: str, style: AppliedStyle) -> str:
    result = text
    trailing_period = bool(style.settings.get("trailing_period", True))
    if not trailing_period and result.endswith("."):
        result = result[:-1]
    return result
