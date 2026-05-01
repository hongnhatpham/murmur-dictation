from __future__ import annotations

import json
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Iterable

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
) -> str:
    prompt = _build_prompt(
        raw_transcript=raw_transcript,
        deterministic_text=deterministic_text,
        app_context=app_context,
        dictionary_terms=dictionary_terms,
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
) -> str:
    terms = ", ".join(sorted(set(term.strip() for term in dictionary_terms if term.strip())))
    return f"""You are the correction layer for a dictation input method.
Return only JSON: {{"text":"..."}}

Rules:
- Preserve the user's meaning.
- Do not add facts.
- Remove obvious filler words and speech artifacts.
- Fix punctuation, casing, and grammar lightly.
- Respect app category: {app_context.category}.
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
    try:
        parsed = json.loads(response)
    except json.JSONDecodeError:
        return response.strip().strip('"')
    if isinstance(parsed, dict):
        value = parsed.get("text", "")
        return str(value).strip()
    return str(parsed).strip()
