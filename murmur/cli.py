from __future__ import annotations

import argparse
import json
import re
import sqlite3
import sys
import time
import traceback
from dataclasses import replace
from datetime import datetime, timezone
from types import SimpleNamespace
from pathlib import Path

from .audio import record_audio
from .audio_cleanup import cleanup_successful_audio, prune_audio_dir
from .command import route_command_transform
from .config import ensure_local_dirs, load_config, sample_config
from .context import current_app_context
from .correction import correct_text, warm_correction_model
from .doctor import format_checks, has_required_failures, run_checks
from .errors import MurmurError
from .history import HistoryStore, format_entries, format_latency_metrics
from .insertion import InsertionError, copy_selection_to_clipboard, copy_to_clipboard, insert_text, read_clipboard
from .notify import notify
from .personal import PersonalStore, format_snippets, format_terms, format_vocabulary_misses, likely_vocabulary_misses
from .styles import apply_style, style_for_category
from .session import (
    acquire_lock,
    clear_session,
    load_active_session,
    lock_path,
    release_lock,
    start_recording_session,
    stop_recording_process,
)
from .status import write_status
from .stt import SttError, transcribe
from .transform import transform_transcript


def debug_log(message: str) -> None:
    try:
        log_path = Path.home() / ".local/state/murmur/murmur.log"
        log_path.parent.mkdir(parents=True, exist_ok=True)
        stamp = datetime.now(timezone.utc).isoformat(timespec="seconds")
        with log_path.open("a", encoding="utf-8") as fh:
            fh.write(f"[{stamp}] {message}\n")
    except Exception:
        pass


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    debug_log("argv=" + " ".join(sys.argv[1:] if argv is None else argv))
    try:
        rc = args.func(args)
        debug_log(f"exit rc={rc}")
        return rc
    except KeyboardInterrupt:
        debug_log("keyboard interrupt")
        print("Canceled", file=sys.stderr)
        return 130
    except Exception:
        debug_log("unhandled exception:\n" + traceback.format_exc())
        print("murmur: unexpected failure; see ~/.local/state/murmur/murmur.log", file=sys.stderr)
        return 1


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="murmur", description="Murmur dictation prototype")
    sub = parser.add_subparsers(required=True)

    p = sub.add_parser("init", help="write default local config")
    p.add_argument("--overwrite", action="store_true")
    p.set_defaults(func=cmd_init)

    p = sub.add_parser("doctor", help="check local dependencies and config")
    p.set_defaults(func=cmd_doctor)

    p = sub.add_parser("logs", help="print recent Murmur and hotkey debug logs")
    p.add_argument("--lines", type=int, default=80)
    p.set_defaults(func=cmd_logs)

    p = sub.add_parser("cleanup-audio", help="remove cached Murmur audio files")
    p.add_argument("--keep-days", type=int, default=None, help="override configured audio retention window")
    p.set_defaults(func=cmd_cleanup_audio)

    p = sub.add_parser("provider-profile", help="switch between local and Groq provider presets")
    p.add_argument("profile", choices=["local", "groq", "cloud"], help="cloud is a compatibility alias for groq")
    p.set_defaults(func=cmd_provider_profile)

    p = sub.add_parser("benchmark", help="benchmark local and Groq STT against existing audio files")
    p.add_argument("audio", nargs="+", type=Path, help="audio files to transcribe; benchmark never pastes")
    p.add_argument("--local-model", action="append", dest="local_models", help="local faster-whisper model to test; repeatable")
    p.add_argument("--groq-model", help="Groq STT model to test; defaults to configured Groq model or whisper-large-v3-turbo")
    p.add_argument("--skip-groq", action="store_true", help="only run local models")
    p.add_argument("--language", help="override STT language hint for benchmark runs")
    p.add_argument("--output", type=Path, help="write JSONL benchmark results locally")
    p.set_defaults(func=cmd_benchmark)

    p = sub.add_parser("usage", help="show approximate provider usage and cost")
    p.add_argument("--days", type=int, default=1)
    p.set_defaults(func=cmd_usage)

    p = sub.add_parser("warm-correction", help="preload the local correction model")
    p.set_defaults(func=cmd_warm_correction)

    p = sub.add_parser("dictate", help="record, transcribe, clean, and copy/paste")
    p.add_argument("--duration", type=float, default=5.0, help="fixed recording duration in seconds")
    p.add_argument("--mode", choices=["clean", "raw"], default=None)
    p.add_argument("--paste", action="store_true", help="paste into focused app after copying")
    p.add_argument("--keep-audio", action="store_true", help="keep temporary audio for debugging")
    p.add_argument("--audio", type=Path, help="use an existing audio file instead of recording")
    p.set_defaults(func=cmd_dictate)

    p = sub.add_parser("start-recording", help="start a held-key recording session")
    p.add_argument("--mode", choices=["clean", "raw"], default=None)
    p.add_argument("--paste", action="store_true", help="paste on stop-recording after copying")
    p.add_argument("--keep-audio", action="store_true", help="keep captured audio after processing")
    p.set_defaults(func=cmd_start_recording)

    p = sub.add_parser("stop-recording", help="stop a held-key recording session and process it")
    p.add_argument("--mode", choices=["clean", "raw"], default=None, help="override mode saved by start-recording")
    p.add_argument("--paste", action="store_true", help="paste into focused app after copying")
    p.add_argument("--no-paste", action="store_true", help="override a start-recording --paste session")
    p.add_argument("--keep-audio", action="store_true", help="keep captured audio after processing")
    p.set_defaults(func=cmd_stop_recording)

    p = sub.add_parser("toggle-recording", help="start recording, or stop and process if already recording")
    p.add_argument("--mode", choices=["clean", "raw"], default=None)
    p.add_argument("--paste", action="store_true", help="paste on stop after copying")
    p.add_argument("--keep-audio", action="store_true", help="keep captured audio after processing")
    p.set_defaults(func=cmd_toggle_recording)

    p = sub.add_parser("cancel-recording", help="cancel a held-key recording session without insertion")
    p.set_defaults(func=cmd_cancel_recording)

    p = sub.add_parser("command", help="record a command and transform selected/clipboard text")
    source = p.add_mutually_exclusive_group()
    source.add_argument("--selection", action="store_true", help="copy the current selection with Ctrl+C before reading clipboard")
    source.add_argument("--clipboard", action="store_true", help="transform existing clipboard text without sending Ctrl+C")
    p.add_argument("--duration", type=float, default=3.0, help="fixed command recording duration in seconds")
    p.add_argument("--audio", type=Path, help="use an existing command audio file instead of recording")
    p.add_argument("--instruction", nargs="+", help="use provided instruction text instead of recording")
    p.add_argument("--paste", action="store_true", help="paste transformed text; --selection implies this")
    p.add_argument("--keep-audio", action="store_true", help="keep temporary audio for debugging")
    p.set_defaults(func=cmd_command)

    p = sub.add_parser("copy", help="transform provided text and copy it to the clipboard")
    p.add_argument("text", nargs="+")
    p.add_argument("--mode", choices=["clean", "raw"], default=None)
    p.add_argument("--private", action="store_true", help="do not write history")
    p.set_defaults(func=cmd_copy)

    p = sub.add_parser("paste", help="transform provided text, copy it, and paste into the focused app")
    p.add_argument("text", nargs="+")
    p.add_argument("--mode", choices=["clean", "raw"], default=None)
    p.add_argument("--private", action="store_true", help="do not write history")
    p.set_defaults(func=cmd_paste)

    p = sub.add_parser("recover", help="copy or print the last final text/raw transcript")
    p.add_argument("--transcript", action="store_true", help="recover raw transcript instead of final text")
    p.add_argument("--print", dest="print_only", action="store_true", help="print instead of copying")
    p.set_defaults(func=cmd_recover)

    p = sub.add_parser("history", help="show recent dictations")
    p.add_argument("--limit", type=int, default=10)
    p.set_defaults(func=cmd_history)

    p = sub.add_parser("metrics", help="show recent latency summaries without transcript text")
    p.add_argument("--limit", type=int, default=20)
    p.set_defaults(func=cmd_metrics)

    p = sub.add_parser("recopy-last", help="copy the last final text back to clipboard")
    p.set_defaults(func=cmd_recopy_last)

    p = sub.add_parser("last-transcript", help="print the last raw transcript")
    p.set_defaults(func=cmd_last_transcript)

    p = sub.add_parser("dictionary", aliases=["dict"], help="manage local personal vocabulary")
    dict_sub = p.add_subparsers(required=True)
    dp = dict_sub.add_parser("add", help="add or update a dictionary term")
    dp.add_argument("term")
    dp.add_argument("--note", help="optional private note")
    dp.add_argument("--replacement", help="replacement text to use when this term is recognized")
    dp.add_argument("--category", help="optional app/category where this term is most relevant")
    dp.set_defaults(func=cmd_dictionary_add)
    dp = dict_sub.add_parser("list", help="list dictionary terms")
    dp.set_defaults(func=cmd_dictionary_list)
    dp = dict_sub.add_parser("remove", aliases=["rm"], help="remove a dictionary term by id or exact term")
    dp.add_argument("key")
    dp.set_defaults(func=cmd_dictionary_remove)
    dp = dict_sub.add_parser("misses", help="show likely dictionary misses from recent history")
    dp.add_argument("--limit", type=int, default=50)
    dp.set_defaults(func=cmd_dictionary_misses)

    p = sub.add_parser("snippets", aliases=["snippet"], help="manage local text snippets")
    snip_sub = p.add_subparsers(required=True)
    sp = snip_sub.add_parser("add", help="add or update a snippet")
    sp.add_argument("trigger")
    sp.add_argument("expansion", nargs="+")
    sp.set_defaults(func=cmd_snippet_add)
    sp = snip_sub.add_parser("list", help="list snippets")
    sp.set_defaults(func=cmd_snippet_list)
    sp = snip_sub.add_parser("remove", aliases=["rm"], help="remove a snippet by id or trigger")
    sp.add_argument("key")
    sp.set_defaults(func=cmd_snippet_remove)
    sp = snip_sub.add_parser("expand", help="print the expansion for a trigger")
    sp.add_argument("trigger")
    sp.set_defaults(func=cmd_snippet_expand)

    return parser


def cmd_init(args: argparse.Namespace) -> int:
    cfg = load_config()
    cfg.config_path.parent.mkdir(parents=True, exist_ok=True)
    if cfg.config_path.exists() and not args.overwrite:
        print(cfg.config_path)
        return 0
    cfg.config_path.write_text(sample_config())
    cfg.config_path.chmod(0o600)
    print(cfg.config_path)
    return 0


def cmd_doctor(_args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    checks = run_checks(cfg)
    print(format_checks(checks))
    return 1 if has_required_failures(checks) else 0


def _tail(path: Path, lines: int) -> str:
    if not path.exists():
        return "(missing)"
    data = path.read_text(encoding="utf-8", errors="replace").splitlines()
    return "\n".join(data[-lines:]) if data else "(empty)"


def cmd_logs(args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    paths = [
        cfg.paths.state_dir / "murmur.log",
        cfg.paths.state_dir / "hotkey-evdev.log",
        cfg.paths.state_dir / "swhkd-actions.log",
        cfg.paths.state_dir / "recording-session.log",
        cfg.paths.state_dir / "recording-session.json",
        cfg.paths.state_dir / "recording-session.lock",
    ]
    for path in paths:
        print(f"\n== {path} ==")
        print(_tail(path, args.lines))
    return 0


def cmd_cleanup_audio(args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    keep_days = cfg.privacy.keep_audio_days if args.keep_days is None else args.keep_days
    result = prune_audio_dir(cfg.paths.debug_audio_dir, keep_days=keep_days)
    print(f"removed={result.removed} freed_bytes={result.freed_bytes}")
    return 0


def cmd_provider_profile(args: argparse.Namespace) -> int:
    cfg = load_config()
    cfg.config_path.parent.mkdir(parents=True, exist_ok=True)
    text = cfg.config_path.read_text(encoding="utf-8") if cfg.config_path.exists() else sample_config()
    profile = "groq" if args.profile == "cloud" else args.profile
    if profile == "groq":
        text = _set_toml_value(text, "stt", "provider", '"groq"')
        text = _set_toml_value(text, "stt", "model", '"whisper-large-v3-turbo"')
        text = _set_toml_value(text, "stt", "language", '"en"')
        text = _set_toml_value(text, "correction", "enabled", "false")
        text = _set_toml_value(text, "correction", "provider", '"groq"')
        text = _set_toml_value(text, "correction", "model", '"openai/gpt-oss-20b"')
        text = _set_toml_value(text, "correction", "endpoint", '"https://api.groq.com/openai/v1/chat/completions"')
    else:
        text = _set_toml_value(text, "stt", "provider", '"faster-whisper"')
        text = _set_toml_value(text, "stt", "model", '"small.en"')
        text = _set_toml_value(text, "stt", "language", '"en"')
        text = _set_toml_value(text, "correction", "enabled", "false")
        text = _set_toml_value(text, "correction", "provider", '"ollama"')
        text = _set_toml_value(text, "correction", "model", '"qwen3:1.7b"')
        text = _set_toml_value(text, "correction", "endpoint", '"http://127.0.0.1:11434/api/generate"')
    cfg.config_path.write_text(text, encoding="utf-8")
    alias = " (cloud alias)" if args.profile == "cloud" else ""
    print(f"{profile} profile{alias} written to {cfg.config_path}; correction.enabled=false")
    return 0


def cmd_benchmark(args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    audio_paths = [path.expanduser() for path in args.audio]
    missing = [str(path) for path in audio_paths if not path.exists()]
    if missing:
        print(f"murmur: audio file not found: {', '.join(missing)}", file=sys.stderr)
        return 1

    personal = PersonalStore(cfg.paths.personal_path)
    terms = [term.term for term in personal.list_terms()]
    rows: list[dict[str, object]] = []
    for audio_path in audio_paths:
        for provider, model in _benchmark_profiles(cfg, args):
            language = args.language or ("en" if provider == "groq" and cfg.stt.language == "auto" else cfg.stt.language)
            bench_stt = replace(cfg.stt, provider=provider, model=model, language=language)
            start = time.perf_counter()
            stt_start = time.perf_counter()
            transcript = ""
            error: str | None = None
            provider_name = provider
            status = "ok"
            try:
                tx = transcribe(audio_path, bench_stt, dictionary_terms=terms)
                provider_name = tx.provider
                transcript = tx.text
            except SttError as exc:
                status = "failed"
                error = str(exc)
            stt_latency_ms = _elapsed_ms(stt_start)
            total_latency_ms = _elapsed_ms(start)
            rows.append(
                {
                    "audio": str(audio_path),
                    "provider": provider_name,
                    "model": model,
                    "status": status,
                    "total_latency_ms": total_latency_ms,
                    "stt_latency_ms": stt_latency_ms,
                    "insertion_status": "skipped",
                    "transcript": transcript,
                    "error": error,
                }
            )

    print(_format_benchmark_rows(rows))
    if args.output:
        output_path = args.output.expanduser()
        output_path.parent.mkdir(parents=True, exist_ok=True)
        with output_path.open("w", encoding="utf-8") as fh:
            for row in rows:
                fh.write(json.dumps(row, ensure_ascii=False) + "\n")
        print(f"saved={output_path}")
    return 0 if any(row["status"] == "ok" for row in rows) else 1


def _benchmark_profiles(cfg, args: argparse.Namespace) -> list[tuple[str, str]]:
    local_models = args.local_models or ["tiny.en", "base.en"]
    profiles = [("faster-whisper", str(model)) for model in local_models]
    if not args.skip_groq:
        groq_model = args.groq_model or (cfg.stt.model if cfg.stt.provider == "groq" else "whisper-large-v3-turbo")
        profiles.append(("groq", str(groq_model)))
    return profiles


def _format_benchmark_rows(rows: list[dict[str, object]]) -> str:
    lines = []
    for row in rows:
        error = f" error={row['error']}" if row.get("error") else ""
        transcript = json.dumps(str(row.get("transcript") or ""), ensure_ascii=False)
        lines.append(
            f"audio={row['audio']} provider={row['provider']} model={row['model']} "
            f"status={row['status']} total_ms={row['total_latency_ms']} stt_ms={row['stt_latency_ms']} "
            f"insertion={row['insertion_status']}{error} transcript={transcript}"
        )
    return "\n".join(lines)


def _set_toml_value(text: str, section: str, key: str, value: str) -> str:
    pattern = re.compile(rf"(^\[{re.escape(section)}\]\n)(.*?)(?=^\[|\Z)", re.M | re.S)
    match = pattern.search(text)
    if not match:
        return text.rstrip() + f"\n\n[{section}]\n{key} = {value}\n"
    body = match.group(2)
    key_pattern = re.compile(rf"^(#\s*)?{re.escape(key)}\s*=.*$", re.M)
    if key_pattern.search(body):
        body = key_pattern.sub(f"{key} = {value}", body, count=1)
    else:
        body = body.rstrip() + f"\n{key} = {value}\n"
    return text[: match.start(2)] + body + text[match.end(2) :]


def cmd_usage(args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    if not cfg.paths.history_path.exists():
        print("No history yet.")
        return 0
    cutoff = f"-{max(args.days, 1)} days"
    with sqlite3.connect(cfg.paths.history_path) as conn:
        conn.row_factory = sqlite3.Row
        rows = conn.execute(
            """
            SELECT provider, COUNT(*) AS requests, COALESCE(SUM(audio_duration_ms), 0) AS audio_ms,
                   COALESCE(AVG(latency_ms), 0) AS avg_latency_ms
            FROM dictations
            WHERE created_at >= datetime('now', ?)
            GROUP BY provider
            ORDER BY requests DESC
            """,
            (cutoff,),
        ).fetchall()
    if not rows:
        print("No usage in this window.")
        return 0
    for row in rows:
        audio_seconds = int(row["audio_ms"] or 0) / 1000
        estimate = ""
        if row["provider"] == "groq":
            billable_seconds = max(audio_seconds, int(row["requests"] or 0) * 10)
            estimate = f" estimated_stt_cost=${billable_seconds / 3600 * 0.04:.4f}"
        print(f"provider={row['provider']} requests={row['requests']} audio_seconds={audio_seconds:.1f} avg_latency_ms={int(row['avg_latency_ms'])}{estimate}")
    return 0


def cmd_warm_correction(_args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    result = warm_correction_model(cfg.correction)
    if result.status == "warmed":
        print(f"warmed {cfg.correction.model} in {result.latency_ms}ms")
        return 0
    print(f"correction warmup {result.status}: {result.error or 'not enabled'}", file=sys.stderr)
    return 1 if result.status != "skipped" else 0


def _cleanup_processed_audio(cfg, audio_path: Path, *, keep_audio: bool) -> None:
    result = cleanup_successful_audio(
        audio_path,
        audio_dir=cfg.paths.debug_audio_dir,
        keep_audio=keep_audio,
        keep_audio_days=cfg.privacy.keep_audio_days,
    )
    if result.removed:
        debug_log(f"audio cleanup removed={result.removed} freed_bytes={result.freed_bytes}")


def _elapsed_ms(start: float) -> int:
    return int((time.perf_counter() - start) * 1000)


def _overhead_latency_ms(
    total_latency_ms: int | None,
    *stages: int | None,
) -> int | None:
    if total_latency_ms is None:
        return None
    known = sum(stage for stage in stages if stage is not None)
    return max(total_latency_ms - known, 0)


def _process_audio(
    *,
    cfg,
    audio_path: Path,
    mode: str,
    paste: bool,
    duration_ms: int | None = None,
    start: float | None = None,
    recorder_stop_latency_ms: int | None = None,
) -> int:
    store = HistoryStore(cfg.paths.history_path)
    transcript_text = ""
    deterministic_text = ""
    final_text = ""
    provider = cfg.stt.provider
    start = start or time.perf_counter()
    stage = "validate"
    stage_start = start
    stt_latency_ms: int | None = None
    transform_latency_ms: int | None = None
    clipboard_latency_ms: int | None = None
    paste_latency_ms: int | None = None
    correction_provider = "deterministic"
    correction_status = "skipped"
    correction_latency_ms: int | None = None
    correction_error: str | None = None
    try:
        if not audio_path.exists() or audio_path.stat().st_size == 0:
            raise MurmurError(
                "Audio recording produced no data.",
                "Check that the default microphone works and PipeWire can see it (try `wpctl status` and `pw-record test.wav`).",
            )
        notify("Processing")
        write_status(cfg.paths.state_dir, "processing", "Transcribing", ttl_seconds=30)
        personal = PersonalStore(cfg.paths.personal_path)
        term_entries = personal.list_terms()
        terms = [term.term for term in term_entries]
        snippets = personal.snippet_map()
        stage = "stt"
        stage_start = time.perf_counter()
        tx = transcribe(audio_path, cfg.stt, dictionary_terms=terms)
        stt_latency_ms = _elapsed_ms(stage_start)
        provider = tx.provider
        transcript_text = tx.text
        stage = "transform"
        stage_start = time.perf_counter()
        transformed = transform_transcript(
            transcript_text,
            mode=mode,
            press_enter_phrase=cfg.insertion.press_enter_phrase,
            dictionary_terms=term_entries,
            snippets=snippets,
        )
        app_context = current_app_context(cfg.context)
        style = style_for_category(app_context.category, cfg.styles)
        styled_text = apply_style(transformed.final_text, style)
        deterministic_text = styled_text
        skip_reason = _correction_skip_reason(cfg, app_context.category, duration_ms)
        transform_latency_ms = _elapsed_ms(stage_start)
        stage = "correction"
        stage_start = time.perf_counter()
        if skip_reason:
            correction = SimpleNamespace(
                final_text=styled_text,
                provider=cfg.correction.provider,
                status="skipped",
                latency_ms=0,
                error=skip_reason,
            )
        else:
            correction = correct_text(
                raw_transcript=transcript_text,
                deterministic_text=styled_text,
                mode=mode,
                config=cfg.correction,
                app_context=app_context,
                dictionary_terms=terms,
                style=style.settings,
            )
        correction_provider = correction.provider
        correction_status = correction.status
        correction_latency_ms = correction.latency_ms
        correction_error = correction.error
        final_text = correction.final_text
        stage = "insertion"
        stage_start = time.perf_counter()
        result = insert_text(final_text, transformed.actions, cfg.insertion, paste=paste)
        clipboard_latency_ms = result.clipboard_latency_ms
        paste_latency_ms = result.paste_latency_ms
        latency_ms = _elapsed_ms(start)
        overhead_latency_ms = _overhead_latency_ms(
            latency_ms,
            recorder_stop_latency_ms,
            stt_latency_ms,
            transform_latency_ms,
            correction_latency_ms,
            clipboard_latency_ms,
            paste_latency_ms,
        )
        if cfg.privacy.history:
            store.add(
                mode=mode,
                provider=provider,
                transcript=transcript_text,
                deterministic_text=deterministic_text,
                final_text=final_text,
                correction_provider=correction_provider,
                correction_status=correction_status,
                correction_latency_ms=correction_latency_ms,
                focused_app_id=app_context.focused_app_id,
                app_category=app_context.category,
                style_applied=style.name,
                correction_error=correction_error,
                actions=transformed.actions,
                insertion_status=result.status,
                audio_duration_ms=duration_ms,
                latency_ms=latency_ms,
                recorder_stop_latency_ms=recorder_stop_latency_ms,
                stt_latency_ms=stt_latency_ms,
                transform_latency_ms=transform_latency_ms,
                clipboard_latency_ms=clipboard_latency_ms,
                paste_latency_ms=paste_latency_ms,
                overhead_latency_ms=overhead_latency_ms,
            )
        notify("Inserted" if result.status == "pasted" else "Copied", result.message)
        write_status(cfg.paths.state_dir, "inserted" if result.status == "pasted" else "copied", result.message, ttl_seconds=2.5)
        print(final_text)
        print(f"[{result.status}] {result.message} latency_ms={latency_ms}", file=sys.stderr)
        return 0
    except (MurmurError, SttError, InsertionError) as exc:
        debug_log(f"process_audio failed: {exc!r}")
        elapsed_ms = _elapsed_ms(start)
        if stage == "stt" and stt_latency_ms is None:
            stt_latency_ms = _elapsed_ms(stage_start)
        elif stage == "transform" and transform_latency_ms is None:
            transform_latency_ms = _elapsed_ms(stage_start)
        elif stage == "insertion":
            paste_latency_ms = _elapsed_ms(stage_start)
        overhead_latency_ms = _overhead_latency_ms(
            elapsed_ms,
            recorder_stop_latency_ms,
            stt_latency_ms,
            transform_latency_ms,
            correction_latency_ms,
            clipboard_latency_ms,
            paste_latency_ms,
        )
        if cfg.privacy.history:
            store.add(
                mode=mode,
                provider=provider,
                transcript=transcript_text or None,
                deterministic_text=deterministic_text or final_text or None,
                final_text=final_text or None,
                correction_provider=correction_provider,
                correction_status="failed" if final_text else correction_status,
                correction_latency_ms=correction_latency_ms,
                correction_error=correction_error,
                insertion_status="failed",
                audio_duration_ms=duration_ms,
                latency_ms=elapsed_ms,
                recorder_stop_latency_ms=recorder_stop_latency_ms,
                stt_latency_ms=stt_latency_ms,
                transform_latency_ms=transform_latency_ms,
                clipboard_latency_ms=clipboard_latency_ms,
                paste_latency_ms=paste_latency_ms,
                overhead_latency_ms=overhead_latency_ms,
                failure_stage=stage,
                error_message=str(exc),
            )
        notify("Failed", str(exc))
        write_status(cfg.paths.state_dir, "failed", str(exc), ttl_seconds=4)
        doctor = exc.doctor() if isinstance(exc, MurmurError) else str(exc)
        print(f"murmur: {doctor}", file=sys.stderr)
        return 1


def _correction_skip_reason(cfg, app_category: str, duration_ms: int | None) -> str | None:
    if not cfg.correction.enabled:
        return None
    if app_category in cfg.correction.skip_categories:
        return f"policy: category {app_category}"
    if duration_ms is not None and cfg.correction.skip_below_duration_ms > 0 and duration_ms < cfg.correction.skip_below_duration_ms:
        return f"policy: duration {duration_ms}ms < {cfg.correction.skip_below_duration_ms}ms"
    return None


def cmd_dictate(args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    mode = args.mode or cfg.cleanup.default_mode
    audio_path: Path | None = None
    try:
        if args.audio:
            audio_path = args.audio.expanduser()
            if not audio_path.exists():
                raise MurmurError(f"Audio file not found: {audio_path}")
            return _process_audio(cfg=cfg, audio_path=audio_path, mode=mode, paste=args.paste, start=time.perf_counter())

        cfg.paths.debug_audio_dir.mkdir(parents=True, exist_ok=True)
        audio_path = cfg.paths.debug_audio_dir / f"murmur-{int(time.time() * 1000)}.wav"
        notify("Recording")
        write_status(cfg.paths.state_dir, "recording", "Recording", ttl_seconds=30)
        rec_start = time.perf_counter()
        record_audio(audio_path, args.duration, target=cfg.recording.target)
        duration_ms = int((time.perf_counter() - rec_start) * 1000)
        rc = _process_audio(cfg=cfg, audio_path=audio_path, mode=mode, paste=args.paste, duration_ms=duration_ms, start=time.perf_counter())
        if rc == 0:
            _cleanup_processed_audio(cfg, audio_path, keep_audio=(args.keep_audio or cfg.privacy.keep_debug_audio))
        return rc
    except MurmurError as exc:
        notify("Failed", str(exc))
        print(f"murmur: {exc.doctor()}", file=sys.stderr)
        return 1


def cmd_start_recording(args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    try:
        fd = acquire_lock(lock_path(cfg.paths.state_dir))
    except MurmurError as exc:
        notify("Failed", str(exc))
        print(f"murmur: {exc.doctor()}", file=sys.stderr)
        return 1
    try:
        session = start_recording_session(
            state_dir=cfg.paths.state_dir,
            audio_dir=cfg.paths.debug_audio_dir,
            target=cfg.recording.target,
            mode=args.mode,
            paste=args.paste,
            keep_audio=args.keep_audio,
        )
    except MurmurError as exc:
        debug_log(f"start-recording failed: {exc.doctor()}")
        if str(exc) == "Recording already in progress.":
            # swhkd can repeat the press binding while Super+Space is held.
            # Repeated starts are expected in hold-to-record mode; ignore them.
            return 0
        notify("Failed", str(exc))
        print(f"murmur: {exc.doctor()}", file=sys.stderr)
        return 1
    finally:
        release_lock(lock_path(cfg.paths.state_dir), fd)
    write_status(cfg.paths.state_dir, "recording", "Recording", ttl_seconds=0)
    notify("Recording")
    print(f"recording pid={session.pid} audio={session.audio_path}")
    return 0


def cmd_stop_recording(args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    try:
        fd = acquire_lock(lock_path(cfg.paths.state_dir))
    except MurmurError as exc:
        notify("Failed", str(exc))
        print(f"murmur: {exc.doctor()}", file=sys.stderr)
        return 1
    try:
        release_start = time.perf_counter()
        recorder_stop_latency_ms: int | None = None
        try:
            session = load_active_session(cfg.paths.state_dir)
            stop_start = time.perf_counter()
            stop_recording_process(session)
            recorder_stop_latency_ms = _elapsed_ms(stop_start)
            clear_session(cfg.paths.state_dir)
        except MurmurError as exc:
            debug_log(f"stop-recording failed before processing: {exc.doctor()}")
            if str(exc) in {"No recording in progress.", "Recording process is not running."}:
                # Release hotkeys can fire more than once after a successful stop.
                # Treat stale/no-session stops as no-ops to avoid false failure notifications.
                return 0
            notify("Failed", str(exc))
            print(f"murmur: {exc.doctor()}", file=sys.stderr)
            return 1

        mode = args.mode or session.mode or cfg.cleanup.default_mode
        paste = False if args.no_paste else (args.paste or session.paste)
        duration_ms = int((time.time() - session.started_at) * 1000)
        keep_audio = args.keep_audio or session.keep_audio or cfg.privacy.keep_debug_audio
        rc = _process_audio(
            cfg=cfg,
            audio_path=session.audio_path,
            mode=mode,
            paste=paste,
            duration_ms=duration_ms,
            start=release_start,
            recorder_stop_latency_ms=recorder_stop_latency_ms,
        )
        if rc == 0:
            _cleanup_processed_audio(cfg, session.audio_path, keep_audio=keep_audio)
        return rc
    finally:
        release_lock(lock_path(cfg.paths.state_dir), fd)


def load_active_session_or_none(state_dir: Path):
    try:
        return load_active_session(state_dir)
    except MurmurError:
        return None


def cmd_toggle_recording(args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    if load_active_session_or_none(cfg.paths.state_dir) is None:
        return cmd_start_recording(args)
    stop_args = argparse.Namespace(
        mode=args.mode,
        paste=args.paste,
        no_paste=False,
        keep_audio=args.keep_audio,
    )
    return cmd_stop_recording(stop_args)


def cmd_cancel_recording(_args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    try:
        fd = acquire_lock(lock_path(cfg.paths.state_dir))
    except MurmurError as exc:
        notify("Failed", str(exc))
        print(f"murmur: {exc.doctor()}", file=sys.stderr)
        return 1
    try:
        session = load_active_session(cfg.paths.state_dir)
        stop_recording_process(session, validate_audio=False)
        clear_session(cfg.paths.state_dir)
        session.audio_path.unlink(missing_ok=True)
    except MurmurError as exc:
        debug_log(f"cancel-recording failed: {exc.doctor()}")
        if str(exc) in {"No recording in progress.", "Recording process is not running."}:
            # Escape may repeat while the temporary swhkd mode is already cleared.
            return 0
        notify("Failed", str(exc))
        print(f"murmur: {exc.doctor()}", file=sys.stderr)
        return 1
    finally:
        release_lock(lock_path(cfg.paths.state_dir), fd)
    write_status(cfg.paths.state_dir, "canceled", "Canceled", ttl_seconds=2)
    notify("Canceled")
    print("canceled")
    return 0


def _cmd_insert_text(args: argparse.Namespace, *, paste: bool) -> int:
    cfg = load_config()
    mode = args.mode or cfg.cleanup.default_mode
    transcript_text = " ".join(args.text)
    start = time.perf_counter()
    personal = PersonalStore(cfg.paths.personal_path)
    transform_start = time.perf_counter()
    transformed = transform_transcript(
        transcript_text,
        mode=mode,
        press_enter_phrase=cfg.insertion.press_enter_phrase,
        dictionary_terms=personal.list_terms(),
        snippets=personal.snippet_map(),
    )
    app_context = current_app_context(cfg.context)
    style = style_for_category(app_context.category, cfg.styles)
    final_text = apply_style(transformed.final_text, style)
    transform_latency_ms = _elapsed_ms(transform_start)
    result = insert_text(final_text, transformed.actions, cfg.insertion, paste=paste)
    latency_ms = _elapsed_ms(start)
    overhead_latency_ms = _overhead_latency_ms(
        latency_ms,
        transform_latency_ms,
        result.clipboard_latency_ms,
        result.paste_latency_ms,
    )
    if cfg.privacy.history and not args.private:
        HistoryStore(cfg.paths.history_path).add(
            mode=mode,
            provider="manual",
            transcript=transcript_text,
            deterministic_text=final_text,
            final_text=final_text,
            correction_provider="deterministic",
            correction_status="skipped",
            focused_app_id=app_context.focused_app_id,
            app_category=app_context.category,
            style_applied=style.name,
            actions=transformed.actions,
            insertion_status=result.status,
            latency_ms=latency_ms,
            transform_latency_ms=transform_latency_ms,
            clipboard_latency_ms=result.clipboard_latency_ms,
            paste_latency_ms=result.paste_latency_ms,
            overhead_latency_ms=overhead_latency_ms,
        )
    print(final_text)
    print(f"[{result.status}] {result.message}", file=sys.stderr)
    return 0


def cmd_command(args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    audio_path: Path | None = None
    transcript_text = ""
    provider = cfg.stt.provider
    start = time.perf_counter()
    stt_latency_ms: int | None = None

    try:
        if args.selection:
            if not copy_selection_to_clipboard(cfg.insertion):
                print("murmur: could not copy selection; falling back to existing clipboard", file=sys.stderr)
        selected_text = read_clipboard()
        if not selected_text:
            print("murmur: no selected/clipboard text to transform", file=sys.stderr)
            return 1
        if len(selected_text) > cfg.command.max_selection_chars:
            print(
                f"murmur: selected text is too large ({len(selected_text)} chars; max {cfg.command.max_selection_chars})",
                file=sys.stderr,
            )
            return 1

        if args.instruction:
            transcript_text = " ".join(args.instruction)
            provider = "manual"
        else:
            if args.audio:
                audio_path = args.audio.expanduser()
                if not audio_path.exists():
                    raise MurmurError(f"Audio file not found: {audio_path}")
            else:
                cfg.paths.debug_audio_dir.mkdir(parents=True, exist_ok=True)
                audio_path = cfg.paths.debug_audio_dir / f"murmur-command-{int(time.time() * 1000)}.wav"
                notify("Recording command")
                write_status(cfg.paths.state_dir, "command", "Recording command", ttl_seconds=30)
                record_audio(audio_path, args.duration, target=cfg.recording.target)
            notify("Processing command")
            write_status(cfg.paths.state_dir, "processing", "Command", ttl_seconds=30)
            personal = PersonalStore(cfg.paths.personal_path)
            stt_start = time.perf_counter()
            tx = transcribe(audio_path, cfg.stt, dictionary_terms=[term.term for term in personal.list_terms()])
            stt_latency_ms = _elapsed_ms(stt_start)
            provider = tx.provider
            transcript_text = tx.text

        transform_start = time.perf_counter()
        routed = route_command_transform(transcript_text, selected_text)
        if not routed.supported:
            print(f"murmur: {routed.message}", file=sys.stderr)
            print(selected_text)
            return 2

        app_context = current_app_context(cfg.context)
        style = style_for_category(app_context.category, cfg.styles)
        final_text = apply_style(routed.final_text, style)
        transform_latency_ms = _elapsed_ms(transform_start)
        result = insert_text(final_text, [], cfg.insertion, paste=(args.paste or args.selection))
        latency_ms = _elapsed_ms(start)
        overhead_latency_ms = _overhead_latency_ms(
            latency_ms,
            stt_latency_ms,
            transform_latency_ms,
            result.clipboard_latency_ms,
            result.paste_latency_ms,
        )
        if cfg.privacy.history:
            HistoryStore(cfg.paths.history_path).add(
                mode=f"command:{routed.command}",
                provider=provider,
                transcript=transcript_text,
                deterministic_text=final_text,
                final_text=final_text,
                correction_provider="deterministic",
                correction_status="skipped",
                focused_app_id=app_context.focused_app_id,
                app_category=app_context.category,
                style_applied=style.name,
                actions=[],
                insertion_status=result.status,
                latency_ms=latency_ms,
                stt_latency_ms=stt_latency_ms,
                transform_latency_ms=transform_latency_ms,
                clipboard_latency_ms=result.clipboard_latency_ms,
                paste_latency_ms=result.paste_latency_ms,
                overhead_latency_ms=overhead_latency_ms,
            )
        notify("Command applied" if result.status == "pasted" else "Command copied", result.message)
        write_status(cfg.paths.state_dir, "inserted" if result.status == "pasted" else "copied", result.message, ttl_seconds=2.5)
        print(final_text)
        print(f"[{result.status}] {result.message}", file=sys.stderr)
        return 0
    except (MurmurError, SttError, InsertionError) as exc:
        notify("Command failed", str(exc))
        write_status(cfg.paths.state_dir, "failed", str(exc), ttl_seconds=4)
        doctor = exc.doctor() if isinstance(exc, MurmurError) else str(exc)
        print(f"murmur: {doctor}", file=sys.stderr)
        return 1
    finally:
        keep_audio = args.keep_audio or cfg.privacy.keep_debug_audio
        if audio_path and not args.audio and not keep_audio and transcript_text:
            _cleanup_processed_audio(cfg, audio_path, keep_audio=keep_audio)


def cmd_copy(args: argparse.Namespace) -> int:
    return _cmd_insert_text(args, paste=False)


def cmd_paste(args: argparse.Namespace) -> int:
    return _cmd_insert_text(args, paste=True)


def cmd_recover(args: argparse.Namespace) -> int:
    cfg = load_config()
    entry = HistoryStore(cfg.paths.history_path).last()
    if not entry:
        print("No history entries", file=sys.stderr)
        return 1
    text = (entry.transcript if args.transcript else entry.final_text) or ""
    if args.print_only:
        print(text)
        return 0
    copy_to_clipboard(text, cfg.insertion.clipboard_tool)
    print(text)
    return 0



def cmd_history(args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    print(format_entries(HistoryStore(cfg.paths.history_path).recent(args.limit)))
    return 0


def cmd_metrics(args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    print(format_latency_metrics(HistoryStore(cfg.paths.history_path).recent(args.limit)))
    return 0


def cmd_recopy_last(_args: argparse.Namespace) -> int:
    cfg = load_config()
    entry = HistoryStore(cfg.paths.history_path).last()
    if not entry or not entry.final_text:
        print("No final text in history", file=sys.stderr)
        return 1
    copy_to_clipboard(entry.final_text, cfg.insertion.clipboard_tool)
    print(entry.final_text)
    return 0


def cmd_last_transcript(_args: argparse.Namespace) -> int:
    cfg = load_config()
    entry = HistoryStore(cfg.paths.history_path).last()
    if not entry or not entry.transcript:
        print("No transcript in history", file=sys.stderr)
        return 1
    print(entry.transcript)
    return 0


def _personal_store() -> PersonalStore:
    cfg = load_config()
    ensure_local_dirs(cfg)
    return PersonalStore(cfg.paths.personal_path)


def cmd_dictionary_add(args: argparse.Namespace) -> int:
    try:
        term_id = _personal_store().add_term(args.term, note=args.note, replacement=args.replacement, category=args.category)
    except ValueError as exc:
        print(f"murmur: {exc}", file=sys.stderr)
        return 1
    print(f"Added dictionary term {term_id}: {args.term.strip()}")
    return 0


def cmd_dictionary_list(_args: argparse.Namespace) -> int:
    print(format_terms(_personal_store().list_terms()))
    return 0


def cmd_dictionary_remove(args: argparse.Namespace) -> int:
    removed = _personal_store().remove_term(args.key)
    if not removed:
        print(f"No dictionary term found: {args.key}", file=sys.stderr)
        return 1
    print(f"Removed dictionary term: {args.key}")
    return 0


def cmd_dictionary_misses(args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    personal = PersonalStore(cfg.paths.personal_path)
    entries = HistoryStore(cfg.paths.history_path).recent(args.limit)
    print(format_vocabulary_misses(likely_vocabulary_misses(entries, personal.list_terms())))
    return 0


def cmd_snippet_add(args: argparse.Namespace) -> int:
    try:
        snippet_id = _personal_store().add_snippet(args.trigger, " ".join(args.expansion))
    except ValueError as exc:
        print(f"murmur: {exc}", file=sys.stderr)
        return 1
    print(f"Added snippet {snippet_id}: {args.trigger}")
    return 0


def cmd_snippet_list(_args: argparse.Namespace) -> int:
    print(format_snippets(_personal_store().list_snippets()))
    return 0


def cmd_snippet_remove(args: argparse.Namespace) -> int:
    removed = _personal_store().remove_snippet(args.key)
    if not removed:
        print(f"No snippet found: {args.key}", file=sys.stderr)
        return 1
    print(f"Removed snippet: {args.key}")
    return 0


def cmd_snippet_expand(args: argparse.Namespace) -> int:
    for snippet in _personal_store().list_snippets():
        if snippet.trigger == args.trigger:
            print(snippet.expansion)
            return 0
    print(f"No snippet found: {args.trigger}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
