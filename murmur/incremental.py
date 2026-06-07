from __future__ import annotations

import json
import math
import os
import struct
import tempfile
import threading
import time
import wave
from dataclasses import asdict, dataclass
from pathlib import Path

from .config import MurmurConfig
from .errors import MurmurError
from .personal import PersonalStore
from .session import RecordingSession, process_alive, read_session, session_path
from .stt import Transcription, is_local_stt_provider, transcribe

PARTIAL_FILENAME = "incremental-transcript.json"
DEFAULT_SAMPLE_RATE = 16000
DEFAULT_CHANNELS = 1
DEFAULT_SAMPLE_WIDTH = 2

_WORKERS: dict[str, threading.Thread] = {}
_WORKERS_LOCK = threading.Lock()


@dataclass(frozen=True)
class IncrementalTranscript:
    audio_path: str
    provider: str
    text: str
    updated_at: float
    audio_duration_ms: int
    source_size_bytes: int
    transcription_latency_ms: int


def partial_transcript_path(state_dir: Path) -> Path:
    return state_dir / PARTIAL_FILENAME


def clear_incremental_transcript(state_dir: Path) -> None:
    partial_transcript_path(state_dir).unlink(missing_ok=True)


def incremental_enabled(cfg: MurmurConfig) -> bool:
    if not cfg.privacy.history:
        return False
    raw = os.environ.get("MURMUR_INCREMENTAL_LOCAL_STT", "1").lower()
    if raw in {"0", "false", "no", "off"}:
        return False
    return is_local_stt_provider(cfg.stt.provider)


def start_incremental_transcription(cfg: MurmurConfig, session: RecordingSession) -> bool:
    if not incremental_enabled(cfg):
        return False
    key = str(session.audio_path)
    with _WORKERS_LOCK:
        existing = _WORKERS.get(key)
        if existing and existing.is_alive():
            return False
        clear_incremental_transcript(cfg.paths.state_dir)
        worker = threading.Thread(
            target=_incremental_worker,
            args=(cfg, session),
            name="murmur-incremental-stt",
            daemon=True,
        )
        _WORKERS[key] = worker
        worker.start()
        return True


def incremental_prefill_for_session(
    cfg: MurmurConfig,
    session: RecordingSession,
    *,
    duration_ms: int | None,
) -> Transcription | None:
    if not incremental_enabled(cfg):
        return None
    item = read_incremental_transcript(cfg.paths.state_dir)
    if item is None:
        return None
    if item.audio_path != str(session.audio_path):
        return None
    if item.provider != cfg.stt.provider:
        return None
    if not item.text.strip():
        return None
    max_age_seconds = _float_env("MURMUR_INCREMENTAL_MAX_AGE_SECONDS", 3.0)
    if time.time() - item.updated_at > max_age_seconds:
        return None
    if duration_ms is not None:
        tail_ms = max(duration_ms - item.audio_duration_ms, 0)
        max_tail_ms = _int_env("MURMUR_INCREMENTAL_MAX_TAIL_MS", 1500)
        if tail_ms > max_tail_ms:
            return None
        if tail_ms > 0 and not tail_is_probably_silence(session.audio_path, start_ms=item.audio_duration_ms):
            return None
    return Transcription(text=item.text, provider=f"{item.provider}-incremental")


def await_incremental_prefill_for_session(
    cfg: MurmurConfig,
    session: RecordingSession,
    *,
    duration_ms: int | None,
    timeout_ms: int | None = None,
) -> Transcription | None:
    deadline = time.monotonic() + (_int_env("MURMUR_INCREMENTAL_PREFILL_WAIT_MS", 1500) if timeout_ms is None else timeout_ms) / 1000
    while True:
        prefill = incremental_prefill_for_session(cfg, session, duration_ms=duration_ms)
        if prefill is not None:
            return prefill
        if not incremental_worker_active(session):
            return incremental_prefill_for_session(cfg, session, duration_ms=duration_ms)
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return None
        time.sleep(min(remaining, 0.05))


def incremental_worker_active(session: RecordingSession) -> bool:
    with _WORKERS_LOCK:
        worker = _WORKERS.get(str(session.audio_path))
        return bool(worker and worker.is_alive())


def read_incremental_transcript(state_dir: Path) -> IncrementalTranscript | None:
    path = partial_transcript_path(state_dir)
    if not path.exists():
        return None
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return IncrementalTranscript(
            audio_path=str(data["audio_path"]),
            provider=str(data["provider"]),
            text=str(data["text"]),
            updated_at=float(data["updated_at"]),
            audio_duration_ms=int(data["audio_duration_ms"]),
            source_size_bytes=int(data["source_size_bytes"]),
            transcription_latency_ms=int(data["transcription_latency_ms"]),
        )
    except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError):
        return None


def write_incremental_transcript(state_dir: Path, item: IncrementalTranscript) -> None:
    path = partial_transcript_path(state_dir)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(asdict(item), sort_keys=True), encoding="utf-8")
    tmp.chmod(0o600)
    tmp.replace(path)


def session_is_current(state_dir: Path, session: RecordingSession) -> bool:
    path = session_path(state_dir)
    if not path.exists():
        return False
    try:
        current = read_session(path)
    except Exception:
        return False
    return current.pid == session.pid and current.audio_path == session.audio_path


def write_wav_snapshot(
    source_path: Path,
    snapshot_path: Path,
    *,
    sample_rate: int = DEFAULT_SAMPLE_RATE,
    channels: int = DEFAULT_CHANNELS,
    sample_width: int = DEFAULT_SAMPLE_WIDTH,
) -> int:
    if not source_path.exists():
        raise MurmurError("Incremental recording source is missing.", str(source_path))
    data = source_path.read_bytes()
    header_bytes = 44
    if len(data) <= header_bytes:
        raise MurmurError("Incremental recording source has no audio yet.", str(source_path))
    frame_bytes = channels * sample_width
    pcm = data[header_bytes:]
    pcm = pcm[: len(pcm) - (len(pcm) % frame_bytes)]
    if not pcm:
        raise MurmurError("Incremental recording source has no complete audio frame.", str(source_path))
    snapshot_path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(snapshot_path), "wb") as out:
        out.setnchannels(channels)
        out.setsampwidth(sample_width)
        out.setframerate(sample_rate)
        out.writeframes(pcm)
    bytes_per_second = sample_rate * frame_bytes
    return int(len(pcm) / bytes_per_second * 1000)


def tail_is_probably_silence(audio_path: Path, *, start_ms: int) -> bool:
    threshold = _int_env("MURMUR_INCREMENTAL_TAIL_SILENCE_RMS", 500)
    try:
        with wave.open(str(audio_path), "rb") as wav:
            channels = wav.getnchannels()
            sample_width = wav.getsampwidth()
            rate = wav.getframerate()
            if sample_width != 2:
                return False
            start_frame = max(int(start_ms / 1000 * rate), 0)
            total_frames = wav.getnframes()
            if start_frame >= total_frames:
                return True
            wav.setpos(start_frame)
            raw = wav.readframes(total_frames - start_frame)
    except Exception:
        return False
    if not raw:
        return True
    samples = struct.unpack("<" + "h" * (len(raw) // 2), raw)
    if channels > 1:
        samples = samples[::channels]
    if not samples:
        return True
    rms = math.sqrt(sum(sample * sample for sample in samples) / len(samples))
    return rms <= threshold


def _incremental_worker(cfg: MurmurConfig, session: RecordingSession) -> None:
    min_audio_ms = _int_env("MURMUR_INCREMENTAL_MIN_AUDIO_MS", 500)
    min_growth_bytes = _int_env("MURMUR_INCREMENTAL_MIN_GROWTH_BYTES", DEFAULT_SAMPLE_RATE * DEFAULT_SAMPLE_WIDTH // 2)
    interval_seconds = _float_env("MURMUR_INCREMENTAL_INTERVAL_SECONDS", 0.5)
    terms = [term.term for term in PersonalStore(cfg.paths.personal_path).list_terms()]
    last_size = 0
    try:
        while process_alive(session.pid):
            time.sleep(max(interval_seconds, 0.1))
            try:
                size = session.audio_path.stat().st_size
            except OSError:
                continue
            if size - last_size < min_growth_bytes:
                continue
            with tempfile.NamedTemporaryFile(prefix="murmur-incremental-", suffix=".wav", dir=cfg.paths.state_dir, delete=False) as tmp:
                snapshot_path = Path(tmp.name)
            try:
                audio_duration_ms = write_wav_snapshot(session.audio_path, snapshot_path)
                if audio_duration_ms < min_audio_ms:
                    continue
                start = time.perf_counter()
                tx = transcribe(snapshot_path, cfg.stt, dictionary_terms=terms)
                latency_ms = int((time.perf_counter() - start) * 1000)
                if tx.text.strip() and session_is_current(cfg.paths.state_dir, session):
                    write_incremental_transcript(
                        cfg.paths.state_dir,
                        IncrementalTranscript(
                            audio_path=str(session.audio_path),
                            provider=cfg.stt.provider,
                            text=tx.text,
                            updated_at=time.time(),
                            audio_duration_ms=audio_duration_ms,
                            source_size_bytes=size,
                            transcription_latency_ms=latency_ms,
                        ),
                    )
                last_size = size
            except Exception:
                continue
            finally:
                snapshot_path.unlink(missing_ok=True)
    finally:
        with _WORKERS_LOCK:
            key = str(session.audio_path)
            if _WORKERS.get(key) is threading.current_thread():
                _WORKERS.pop(key, None)


def _int_env(name: str, default: int) -> int:
    try:
        return int(os.environ.get(name, str(default)))
    except ValueError:
        return default


def _float_env(name: str, default: float) -> float:
    try:
        return float(os.environ.get(name, str(default)))
    except ValueError:
        return default
