from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path

from .audio import record_audio
from .command import route_command_transform
from .config import ensure_local_dirs, load_config, sample_config
from .doctor import format_checks, has_required_failures, run_checks
from .errors import MurmurError
from .history import HistoryStore, format_entries
from .insertion import InsertionError, copy_selection_to_clipboard, copy_to_clipboard, insert_text, read_clipboard
from .notify import notify
from .personal import PersonalStore, format_snippets, format_terms
from .session import (
    acquire_lock,
    clear_session,
    load_active_session,
    lock_path,
    release_lock,
    start_recording_session,
    stop_recording_process,
)
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

    p = sub.add_parser("recopy-last", help="copy the last final text back to clipboard")
    p.set_defaults(func=cmd_recopy_last)

    p = sub.add_parser("last-transcript", help="print the last raw transcript")
    p.set_defaults(func=cmd_last_transcript)

    p = sub.add_parser("dictionary", aliases=["dict"], help="manage local personal vocabulary")
    dict_sub = p.add_subparsers(required=True)
    dp = dict_sub.add_parser("add", help="add or update a dictionary term")
    dp.add_argument("term")
    dp.add_argument("--note", help="optional private note")
    dp.set_defaults(func=cmd_dictionary_add)
    dp = dict_sub.add_parser("list", help="list dictionary terms")
    dp.set_defaults(func=cmd_dictionary_list)
    dp = dict_sub.add_parser("remove", aliases=["rm"], help="remove a dictionary term by id or exact term")
    dp.add_argument("key")
    dp.set_defaults(func=cmd_dictionary_remove)

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


def _process_audio(
    *,
    cfg,
    audio_path: Path,
    mode: str,
    paste: bool,
    duration_ms: int | None = None,
    start: float | None = None,
) -> int:
    store = HistoryStore(cfg.paths.history_path)
    transcript_text = ""
    final_text = ""
    provider = cfg.stt.provider
    start = start or time.perf_counter()
    try:
        if not audio_path.exists() or audio_path.stat().st_size == 0:
            raise MurmurError(
                "Audio recording produced no data.",
                "Check that the default microphone works and PipeWire can see it (try `wpctl status` and `pw-record test.wav`).",
            )
        notify("Processing")
        personal = PersonalStore(cfg.paths.personal_path)
        terms = [term.term for term in personal.list_terms()]
        snippets = personal.snippet_map()
        tx = transcribe(audio_path, cfg.stt, dictionary_terms=terms)
        provider = tx.provider
        transcript_text = tx.text
        transformed = transform_transcript(
            transcript_text,
            mode=mode,
            press_enter_phrase=cfg.insertion.press_enter_phrase,
            dictionary_terms=terms,
            snippets=snippets,
        )
        final_text = transformed.final_text
        result = insert_text(final_text, transformed.actions, cfg.insertion, paste=paste)
        latency_ms = int((time.perf_counter() - start) * 1000)
        if cfg.privacy.history:
            store.add(
                mode=mode,
                provider=provider,
                transcript=transcript_text,
                final_text=final_text,
                actions=transformed.actions,
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


def cmd_dictate(args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    mode = args.mode or cfg.cleanup.default_mode
    audio_path: Path | None = None
    start = time.perf_counter()
    try:
        if args.audio:
            audio_path = args.audio.expanduser()
            if not audio_path.exists():
                raise MurmurError(f"Audio file not found: {audio_path}")
            return _process_audio(cfg=cfg, audio_path=audio_path, mode=mode, paste=args.paste, start=start)

        cfg.paths.debug_audio_dir.mkdir(parents=True, exist_ok=True)
        audio_path = cfg.paths.debug_audio_dir / f"murmur-{int(time.time() * 1000)}.wav"
        notify("Recording")
        rec_start = time.perf_counter()
        record_audio(audio_path, args.duration)
        duration_ms = int((time.perf_counter() - rec_start) * 1000)
        return _process_audio(cfg=cfg, audio_path=audio_path, mode=mode, paste=args.paste, duration_ms=duration_ms, start=start)
    except MurmurError as exc:
        notify("Failed", str(exc))
        print(f"murmur: {exc.doctor()}", file=sys.stderr)
        return 1
    finally:
        keep_audio = args.keep_audio or cfg.privacy.keep_debug_audio
        if audio_path and not args.audio and not keep_audio:
            audio_path.unlink(missing_ok=True)


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
            mode=args.mode,
            paste=args.paste,
            keep_audio=args.keep_audio,
        )
    except MurmurError as exc:
        notify("Failed", str(exc))
        print(f"murmur: {exc.doctor()}", file=sys.stderr)
        return 1
    finally:
        release_lock(lock_path(cfg.paths.state_dir), fd)
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
        try:
            session = load_active_session(cfg.paths.state_dir)
            stop_recording_process(session)
            clear_session(cfg.paths.state_dir)
        except MurmurError as exc:
            notify("Failed", str(exc))
            print(f"murmur: {exc.doctor()}", file=sys.stderr)
            return 1

        mode = args.mode or session.mode or cfg.cleanup.default_mode
        paste = False if args.no_paste else (args.paste or session.paste)
        duration_ms = int((time.time() - session.started_at) * 1000)
        keep_audio = args.keep_audio or session.keep_audio or cfg.privacy.keep_debug_audio
        try:
            return _process_audio(cfg=cfg, audio_path=session.audio_path, mode=mode, paste=paste, duration_ms=duration_ms)
        finally:
            if not keep_audio:
                session.audio_path.unlink(missing_ok=True)
    finally:
        release_lock(lock_path(cfg.paths.state_dir), fd)


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
        notify("Failed", str(exc))
        print(f"murmur: {exc.doctor()}", file=sys.stderr)
        return 1
    finally:
        release_lock(lock_path(cfg.paths.state_dir), fd)
    notify("Canceled")
    print("canceled")
    return 0


def _cmd_insert_text(args: argparse.Namespace, *, paste: bool) -> int:
    cfg = load_config()
    mode = args.mode or cfg.cleanup.default_mode
    transcript_text = " ".join(args.text)
    personal = PersonalStore(cfg.paths.personal_path)
    transformed = transform_transcript(
        transcript_text,
        mode=mode,
        press_enter_phrase=cfg.insertion.press_enter_phrase,
        dictionary_terms=[term.term for term in personal.list_terms()],
        snippets=personal.snippet_map(),
    )
    result = insert_text(transformed.final_text, transformed.actions, cfg.insertion, paste=paste)
    if cfg.privacy.history and not args.private:
        HistoryStore(cfg.paths.history_path).add(
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


def cmd_command(args: argparse.Namespace) -> int:
    cfg = load_config()
    ensure_local_dirs(cfg)
    audio_path: Path | None = None
    transcript_text = ""
    provider = cfg.stt.provider

    try:
        if args.selection:
            if not copy_selection_to_clipboard(cfg.insertion):
                print("murmur: could not copy selection; falling back to existing clipboard", file=sys.stderr)
        selected_text = read_clipboard()
        if not selected_text:
            print("murmur: no selected/clipboard text to transform", file=sys.stderr)
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
                record_audio(audio_path, args.duration)
            notify("Processing command")
            personal = PersonalStore(cfg.paths.personal_path)
            tx = transcribe(audio_path, cfg.stt, dictionary_terms=[term.term for term in personal.list_terms()])
            provider = tx.provider
            transcript_text = tx.text

        routed = route_command_transform(transcript_text, selected_text)
        if not routed.supported:
            print(f"murmur: {routed.message}", file=sys.stderr)
            print(selected_text)
            return 2

        result = insert_text(routed.final_text, [], cfg.insertion, paste=(args.paste or args.selection))
        if cfg.privacy.history:
            HistoryStore(cfg.paths.history_path).add(
                mode=f"command:{routed.command}",
                provider=provider,
                transcript=transcript_text,
                final_text=routed.final_text,
                actions=[],
                insertion_status=result.status,
            )
        notify("Command applied" if result.status == "pasted" else "Command copied", result.message)
        print(routed.final_text)
        print(f"[{result.status}] {result.message}", file=sys.stderr)
        return 0
    except (MurmurError, SttError, InsertionError) as exc:
        notify("Command failed", str(exc))
        doctor = exc.doctor() if isinstance(exc, MurmurError) else str(exc)
        print(f"murmur: {doctor}", file=sys.stderr)
        return 1
    finally:
        keep_audio = args.keep_audio or cfg.privacy.keep_debug_audio
        if audio_path and not args.audio and not keep_audio:
            audio_path.unlink(missing_ok=True)


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
        term_id = _personal_store().add_term(args.term, note=args.note)
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
