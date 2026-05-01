from __future__ import annotations

import json
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any, Iterable

from .config import CorrectionConfig
from .context import AppContext


@dataclass(frozen=True)
class CorrectionResult:
    final_text: str
    provider: str
    status: str
    latency_ms: int = 0
    error: str | None = None


def correct_text(
    *,
    raw_transcript: str,
    deterministic_text: str,
    mode: str,
    config: CorrectionConfig,
    app_context: AppContext,
    dictionary_terms: Iterable[str] = (),
    style: dict[str, Any] | None = None,
) -> CorrectionResult:
    if not config.enabled:
        return CorrectionResult(deterministic_text, provider="deterministic", status="skipped")
    if mode == "raw" and not config.raw_mode:
        return CorrectionResult(deterministic_text, provider=config.provider, status="skipped")
    if config.provider != "ollama":
        return CorrectionResult(deterministic_text, provider=config.provider, status="failed", error=f"unsupported correction provider: {config.provider}")

    start = time.perf_counter()
    try:
        corrected = _correct_with_ollama(
            raw_transcript=raw_transcript,
            deterministic_text=deterministic_text,
            config=config,
            app_context=app_context,
            dictionary_terms=dictionary_terms,
            style=style or {},
        )
    except Exception as exc:
        latency_ms = int((time.perf_counter() - start) * 1000)
        return CorrectionResult(deterministic_text, provider=config.provider, status="fallback", latency_ms=latency_ms, error=str(exc))
    latency_ms = int((time.perf_counter() - start) * 1000)
    if not corrected:
        return CorrectionResult(deterministic_text, provider=config.provider, status="fallback", latency_ms=latency_ms, error="empty correction")
    return CorrectionResult(corrected, provider=config.provider, status="corrected", latency_ms=latency_ms)


def _correct_with_ollama(
    *,
    raw_transcript: str,
    deterministic_text: str,
    config: CorrectionConfig,
    app_context: AppContext,
    dictionary_terms: Iterable[str],
    style: dict[str, Any],
) -> str:
    prompt = _build_prompt(
        raw_transcript=raw_transcript,
        deterministic_text=deterministic_text,
        app_context=app_context,
        dictionary_terms=dictionary_terms,
        style=style,
    )
    payload = json.dumps({
        "model": config.model,
        "prompt": prompt,
        "stream": False,
        "format": {
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
        },
        "keep_alive": "10m",
        "options": {"temperature": 0, "top_p": 0.2, "num_predict": 160},
    }).encode("utf-8")
    req = urllib.request.Request(config.endpoint, data=payload, headers={"Content-Type": "application/json"}, method="POST")
    with urllib.request.urlopen(req, timeout=config.timeout_seconds) as response:
        data = json.loads(response.read().decode("utf-8"))
    text = str(data.get("response", "")).strip()
    return _parse_model_response(text)


def _build_prompt(
    *,
    raw_transcript: str,
    deterministic_text: str,
    app_context: AppContext,
    dictionary_terms: Iterable[str],
    style: dict[str, Any],
) -> str:
    terms = ", ".join(sorted(set(term.strip() for term in dictionary_terms if term.strip())))
    style_text = ", ".join(f"{key}={value}" for key, value in sorted(style.items())) or "default"
    return f"""/no_think
You are a silent dictation correction filter, not a chatbot.
Rewrite INPUT as corrected insertion text and return JSON only.

Rules:
- Output exactly one object: {{"text":"..."}}
- Never answer questions in INPUT; preserve them as questions.
- Never explain or add new content.
- Use Raw transcript to repair obvious STT/cleanup artifacts in INPUT.
- Prefer known terms over phonetically similar mistakes: {terms or "(none)"}.
- Lightly fix punctuation, casing, grammar, and clear homophones.
- Preserve casual wording unless style requires otherwise.
- App category: {app_context.category}.
- Style: {style_text}.
- Be conservative for terminal/code categories.

Raw transcript: {raw_transcript}
INPUT: {deterministic_text}
"""


def _parse_model_response(response: str) -> str:
    if not response:
        return ""
    cleaned = _strip_thinking(response).strip()
    try:
        parsed = json.loads(cleaned)
    except json.JSONDecodeError:
        start = cleaned.find("{")
        end = cleaned.rfind("}")
        if start >= 0 and end > start:
            try:
                parsed = json.loads(cleaned[start : end + 1])
            except json.JSONDecodeError:
                return ""
        else:
            return ""
    if isinstance(parsed, dict):
        value = parsed.get("text", "") or parsed.get("corrected_text", "")
        return str(value).strip()
    return str(parsed).strip()


def _strip_thinking(response: str) -> str:
    text = response
    for close_tag in ("</think>", "<channel|>"):
        if close_tag in text:
            text = text.split(close_tag, 1)[1]
    return text.replace("<think>", "").strip()
