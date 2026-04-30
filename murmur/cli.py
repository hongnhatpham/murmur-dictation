from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path

from .audio import record_audio
from .config import ensure_local_dirs, load_config, sample_config
from .doctor import format_checks, has_required_failures, run_checks
from .errors import MurmurError
from .history import HistoryStore, format_entries
from .insertion import InsertionError, copy_to_clipboard, insert_text
from .notify import notify
from .stt import SttError, transcribe
from .transform import transform_transcript


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        return args.func(args)
    except KeyboardInterrupt:
        print("Canceled", file=sys.stderr)
        return 130


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="murmur", description="Murmur dictation prototype")
    sub = parser.add_subparsers(required=True)

    p = sub.add_parser("init", help="write default local config")
    p.add_argument("--overwrite", action="store_true")
    p.set_defaults(func=cmd_init)

    p = sub.add_parser("doctor", help="check local dependencies and config")
    p.set_defaults(func=cmd_doctor)

    p = sub.add_parser("dictate", help="record, transcribe, clean, and copy/paste")
    p.add_argument("--duration", type=float, default=5.0, help="fixed recording duration in seconds")
    p.add_argument("--mode", choices=["clean", "raw"], default=None)
    p.add_argument("--paste", action="store_true", help="paste into focused app after copying")
    p.add_argument("--keep-audio", action="store_true", help="keep temporary audio for debugging")
    p.add_argument("--audio", type=Path, help="use an existing audio file instead of recording")
    p.set_defaults(func=cmd_dictate)

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

    p = sub.add_parser("recopy-last", help="copy the last final text back to clipboard")
    p.set_defaults(func=cmd_recopy_last)

    p = sub.add_parser("last-transcript", help="print the last raw transcript")
    p.set_defaults(func=cmd_last_transcript)

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


def cmd_dictate(args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    store = HistoryStore(cfg.paths.history_path)
    mode = args.mode or cfg.cleanup.default_mode
    audio_path: Path | None = None
    duration_ms: int | None = None
    transcript_text = ""
    final_text = ""
    actions = []
    provider = cfg.stt.provider
    start = time.perf_counter()

    try:
        if args.audio:
            audio_path = args.audio.expanduser()
            if not audio_path.exists():
                raise MurmurError(f"Audio file not found: {audio_path}")
        else:
            cfg.paths.debug_audio_dir.mkdir(parents=True, exist_ok=True)
            audio_path = cfg.paths.debug_audio_dir / f"murmur-{int(time.time() * 1000)}.wav"
            notify("Recording")
            rec_start = time.perf_counter()
            record_audio(audio_path, args.duration)
            duration_ms = int((time.perf_counter() - rec_start) * 1000)

        notify("Processing")
        tx = transcribe(audio_path, cfg.stt)
        provider = tx.provider
        transcript_text = tx.text
        transformed = transform_transcript(transcript_text, mode=mode, press_enter_phrase=cfg.insertion.press_enter_phrase)
        final_text = transformed.final_text
        actions = transformed.actions
        result = insert_text(final_text, actions, cfg.insertion, paste=args.paste)
        latency_ms = int((time.perf_counter() - start) * 1000)
        if cfg.privacy.history:
            store.add(
                mode=mode,
                provider=provider,
                transcript=transcript_text,
                final_text=final_text,
                actions=actions,
                insertion_status=result.status,
                audio_duration_ms=duration_ms,
                latency_ms=latency_ms,
            )
        notify("Inserted" if result.status == "pasted" else "Copied", result.message)
        print(final_text)
        print(f"[{result.status}] {result.message} latency_ms={latency_ms}", file=sys.stderr)
        return 0
    except (MurmurError, SttError, InsertionError) as exc:
        if cfg.privacy.history:
            store.add(
                mode=mode,
                provider=provider,
                transcript=transcript_text or None,
                final_text=final_text or None,
                insertion_status="failed",
                error_message=str(exc),
            )
        notify("Failed", str(exc))
        doctor = exc.doctor() if isinstance(exc, MurmurError) else str(exc)
        print(f"murmur: {doctor}", file=sys.stderr)
        return 1
    finally:
        keep_audio = args.keep_audio or cfg.privacy.keep_debug_audio
        if audio_path and not args.audio and not keep_audio:
            audio_path.unlink(missing_ok=True)


def _cmd_insert_text(args: argparse.Namespace, *, paste: bool) -> int:
    cfg = load_config()
    mode = args.mode or cfg.cleanup.default_mode
    transcript_text = " ".join(args.text)
    transformed = transform_transcript(transcript_text, mode=mode, press_enter_phrase=cfg.insertion.press_enter_phrase)
    result = insert_text(transformed.final_text, transformed.actions, cfg.insertion, paste=paste)
    if cfg.privacy.history and not args.private:
        add_entry(
            mode=mode,
            provider="manual",
            transcript=transcript_text,
            final_text=transformed.final_text,
            actions=transformed.actions,
            insertion_status=result.status,
        )
    print(transformed.final_text)
    print(f"[{result.status}] {result.message}", file=sys.stderr)
    return 0


def cmd_copy(args: argparse.Namespace) -> int:
    return _cmd_insert_text(args, paste=False)


def cmd_paste(args: argparse.Namespace) -> int:
    return _cmd_insert_text(args, paste=True)


def cmd_recover(args: argparse.Namespace) -> int:
    entry = last()
    if not entry:
        print("No history entries", file=sys.stderr)
        return 1
    text = (entry.transcript if args.transcript else entry.final_text) or ""
    if args.print_only:
        print(text)
        return 0
    cfg = load_config()
    copy_to_clipboard(text, cfg.insertion.clipboard_tool)
    print(text)
    return 0


def cmd_history(args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    print(format_entries(HistoryStore(cfg.paths.history_path).recent(args.limit)))
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


if __name__ == "__main__":
    raise SystemExit(main())
