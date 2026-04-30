from __future__ import annotations

import re
from dataclasses import dataclass


@dataclass(frozen=True)
class CommandTransformResult:
    final_text: str
    supported: bool
    command: str | None = None
    message: str = ""


_SPACES_RE = re.compile(r"\s+")
_CONCISE_FILLERS_RE = re.compile(
    r"\b(?:very|really|just|basically|actually|literally|kind of|sort of|in order to|due to the fact that)\b",
    re.IGNORECASE,
)


def _normalize_instruction(instruction: str) -> str:
    return _SPACES_RE.sub(" ", instruction).strip().lower()


def concise_text(text: str) -> str:
    """Small deterministic concision pass; intentionally conservative and local."""
    lines = []
    for line in text.splitlines() or [text]:
        shortened = _CONCISE_FILLERS_RE.sub("", line)
        shortened = re.sub(r"\s+([,.;:!?])", r"\1", shortened)
        shortened = _SPACES_RE.sub(" ", shortened).strip()
        lines.append(shortened)
    return "\n".join(lines).strip()


def route_command_transform(instruction: str, selected_text: str) -> CommandTransformResult:
    """Route a spoken command to a pure deterministic selected-text transform.

    This deliberately supports only safe, local rewrites. Unsupported commands
    return the original selection with a message instead of guessing.
    """
    command = _normalize_instruction(instruction)
    if not command:
        return CommandTransformResult(selected_text, False, None, "No command instruction was transcribed.")

    if any(phrase in command for phrase in ("uppercase", "upper case", "all caps", "capital letters")):
        return CommandTransformResult(selected_text.upper(), True, "uppercase")

    if any(phrase in command for phrase in ("lowercase", "lower case", "make it lower", "small letters")):
        return CommandTransformResult(selected_text.lower(), True, "lowercase")

    if any(phrase in command for phrase in ("concise", "shorter", "tighten", "trim this", "make this brief")):
        return CommandTransformResult(concise_text(selected_text), True, "concise")

    return CommandTransformResult(
        selected_text,
        False,
        None,
        "Unsupported command. Try: uppercase, lowercase, or make concise.",
    )
