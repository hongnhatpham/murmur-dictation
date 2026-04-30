from __future__ import annotations

import re
from collections.abc import Iterable, Mapping
from dataclasses import dataclass, field

from .personal import apply_dictionary_terms, apply_snippets

PRESS_ENTER_AFTER_INSERT = "press_enter_after_insert"


@dataclass(frozen=True)
class TransformAction:
    name: str

    @property
    def type(self) -> str:
        return self.name


@dataclass(frozen=True)
class TransformResult:
    final_text: str
    actions: list[TransformAction] = field(default_factory=list)

    def action_names(self) -> list[str]:
        return [action.name for action in self.actions]


_PRESS_ENTER_RE = re.compile(r"(?:[\s,.;:!?—-]+|^)(press\s+enter)[\s.?!]*$", re.IGNORECASE)
_FILLER_RE = re.compile(r"\b(um+|uh+|erm|ah)\b[ ,]*", re.IGNORECASE)
_SPACES_RE = re.compile(r"\s+")


def detect_press_enter(text: str) -> tuple[str, bool]:
    """Remove a trailing Wispr-style 'press enter' command if present."""
    stripped = text.strip()
    match = _PRESS_ENTER_RE.search(stripped)
    if not match:
        return text, False
    before = stripped[: match.start()].rstrip(" ,.;:!?—-")
    return before, True


def clean_text(text: str, mode: str = "clean") -> str:
    normalized = _SPACES_RE.sub(" ", text).strip()
    if mode == "raw" or not normalized:
        return normalized
    cleaned = _FILLER_RE.sub("", normalized)
    cleaned = _SPACES_RE.sub(" ", cleaned).strip()
    if not cleaned:
        return ""
    cleaned = cleaned[0].upper() + cleaned[1:]
    if cleaned[-1] not in ".!?…:;)]}\"'":
        cleaned += "."
    return cleaned


def transform_transcript(
    transcript: str,
    mode: str = "clean",
    press_enter_phrase: bool = True,
    dictionary_terms: Iterable[str] | None = None,
    snippets: Mapping[str, str] | None = None,
) -> TransformResult:
    actions: list[TransformAction] = []
    working = transcript.strip()

    if press_enter_phrase and mode != "raw":
        working, should_press_enter = detect_press_enter(working)
        if should_press_enter:
            actions.append(TransformAction(PRESS_ENTER_AFTER_INSERT))

    final_text = clean_text(transcript if mode == "raw" else working, mode=mode)
    if snippets:
        final_text = apply_snippets(final_text, snippets)
    if dictionary_terms:
        final_text = apply_dictionary_terms(final_text, dictionary_terms)
    return TransformResult(final_text=final_text, actions=actions)
