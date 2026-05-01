from __future__ import annotations

import json
import os
import time
import urllib.error
import urllib.request
from pathlib import Path
from dataclasses import dataclass
from typing import Any, Iterable

CORRECTION_KEEP_ALIVE = "10m"

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
    if config.provider not in ("ollama", "groq"):
        return CorrectionResult(deterministic_text, provider=config.provider, status="failed", error=f"unsupported correction provider: {config.provider}")

    start = time.perf_counter()
    try:
        corrector = _correct_with_groq if config.provider == "groq" else _correct_with_ollama
        corrected = corrector(
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
        "keep_alive": CORRECTION_KEEP_ALIVE,
        "options": {"temperature": 0, "top_p": 0.2, "num_predict": 160},
    }).encode("utf-8")
    req = urllib.request.Request(config.endpoint, data=payload, headers={"Content-Type": "application/json"}, method="POST")
    with urllib.request.urlopen(req, timeout=config.timeout_seconds) as response:
        data = json.loads(response.read().decode("utf-8"))
    text = str(data.get("response", "")).strip()
    return _parse_model_response(text)


def _correct_with_groq(
    *,
    raw_transcript: str,
    deterministic_text: str,
    config: CorrectionConfig,
    app_context: AppContext,
    dictionary_terms: Iterable[str],
    style: dict[str, Any],
) -> str:
    api_key = _groq_api_key()
    if not api_key:
        raise RuntimeError("missing Groq API key; set GROQ_API_KEY/MURMUR_GROQ_API_KEY or write ~/.config/murmur/groq_api_key")
    prompt = _build_prompt(
        raw_transcript=raw_transcript,
        deterministic_text=deterministic_text,
        app_context=app_context,
        dictionary_terms=dictionary_terms,
        style=style,
    )
    endpoint = config.endpoint
    if "11434" in endpoint or endpoint.endswith("/api/generate"):
        endpoint = "https://api.groq.com/openai/v1/chat/completions"
    payload = json.dumps({
        "model": config.model,
        "messages": [
            {"role": "system", "content": "You are a strict JSON dictation correction filter."},
            {"role": "user", "content": prompt},
        ],
        "temperature": 0,
        "top_p": 0.2,
        "max_completion_tokens": 180,
        "response_format": {"type": "json_object"},
    }).encode("utf-8")
    req = urllib.request.Request(
        endpoint,
        data=payload,
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {api_key}", "User-Agent": "murmur-dictation/0.1"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=config.timeout_seconds) as response:
            data = json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"HTTP {exc.code}: {detail}") from exc
    choices = data.get("choices") or []
    if not choices:
        return ""
    message = choices[0].get("message", {}) if isinstance(choices[0], dict) else {}
    return _parse_model_response(str(message.get("content", "")))


def warm_correction_model(config: CorrectionConfig) -> CorrectionResult:
    if not config.enabled:
        return CorrectionResult("", provider="deterministic", status="skipped")
    if config.provider != "ollama":
        return CorrectionResult("", provider=config.provider, status="failed", error=f"unsupported correction provider: {config.provider}")
    start = time.perf_counter()
    payload = json.dumps({
        "model": config.model,
        "prompt": '/no_think Return JSON only: {"text":"ready"}',
        "stream": False,
        "format": {
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
        },
        "keep_alive": CORRECTION_KEEP_ALIVE,
        "options": {"temperature": 0, "num_predict": 16},
    }).encode("utf-8")
    req = urllib.request.Request(config.endpoint, data=payload, headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=config.timeout_seconds) as response:
            data = json.loads(response.read().decode("utf-8"))
        text = _parse_model_response(str(data.get("response", "")))
    except Exception as exc:
        latency_ms = int((time.perf_counter() - start) * 1000)
        return CorrectionResult("", provider=config.provider, status="fallback", latency_ms=latency_ms, error=str(exc))
    latency_ms = int((time.perf_counter() - start) * 1000)
    return CorrectionResult(text, provider=config.provider, status="warmed" if text else "fallback", latency_ms=latency_ms, error=None if text else "empty correction")


def _groq_api_key() -> str | None:
    key = os.environ.get("MURMUR_GROQ_API_KEY") or os.environ.get("GROQ_API_KEY")
    if key:
        return key.strip()
    key_file = Path(os.environ.get("MURMUR_GROQ_API_KEY_FILE", Path.home() / ".config" / "murmur" / "groq_api_key")).expanduser()
    try:
        if key_file.exists():
            return key_file.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    return None


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
