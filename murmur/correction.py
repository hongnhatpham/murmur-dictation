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
    payload = json.dumps({"model": config.model, "prompt": prompt, "stream": False}).encode("utf-8")
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
    return f"""You are the correction layer for a dictation input method.
/no_think
Return only JSON: {{"text":"..."}}

Rules:
- Preserve the user's meaning.
- Do not add facts.
- Remove obvious filler words and speech artifacts.
- Fix punctuation, casing, and grammar lightly.
- Correct obvious speech-to-text homophones only when context makes the fix clear.
- Do not rewrite casual wording into formal prose unless the style asks for it.
- Respect app category: {app_context.category}.
- Respect style settings: {style_text}.
- Be conservative for terminal/code categories.
- Keep personal vocabulary spelling/casing when relevant: {terms or "(none)"}.

Raw transcript:
{raw_transcript}

Deterministic cleanup:
{deterministic_text}
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
                return cleaned.strip().strip('"')
        else:
            return cleaned.strip().strip('"')
    if isinstance(parsed, dict):
        value = parsed.get("text", "")
        return str(value).strip()
    return str(parsed).strip()


def _strip_thinking(response: str) -> str:
    text = response
    for close_tag in ("</think>", "<channel|>"):
        if close_tag in text:
            text = text.split(close_tag, 1)[1]
    return text.replace("<think>", "").strip()
