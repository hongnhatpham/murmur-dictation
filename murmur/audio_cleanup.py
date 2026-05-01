from __future__ import annotations

import time
from dataclasses import dataclass
from pathlib import Path

AUDIO_EXTENSIONS = {".wav", ".flac", ".mp3", ".m4a", ".ogg"}


@dataclass(frozen=True)
class AudioCleanupResult:
    removed: int = 0
    freed_bytes: int = 0


def _is_audio_file(path: Path) -> bool:
    return path.is_file() and path.suffix.lower() in AUDIO_EXTENSIONS


def remove_audio_file(path: Path) -> AudioCleanupResult:
    """Remove one captured audio file if it exists."""
    try:
        size = path.stat().st_size
    except OSError:
        return AudioCleanupResult()
    try:
        path.unlink()
    except FileNotFoundError:
        return AudioCleanupResult()
    return AudioCleanupResult(removed=1, freed_bytes=size)


def prune_audio_dir(audio_dir: Path, *, keep_days: int) -> AudioCleanupResult:
    """Remove cached audio older than keep_days.

    keep_days=0 removes all recognized audio files in the cache directory.
    Missing directories are treated as already clean.
    """
    if keep_days < 0:
        keep_days = 0
    if not audio_dir.exists():
        return AudioCleanupResult()

    cutoff = time.time() - (keep_days * 24 * 60 * 60)
    removed = 0
    freed = 0
    for path in audio_dir.iterdir():
        if not _is_audio_file(path):
            continue
        try:
            mtime = path.stat().st_mtime
        except OSError:
            continue
        if keep_days > 0 and mtime >= cutoff:
            continue
        result = remove_audio_file(path)
        removed += result.removed
        freed += result.freed_bytes
    return AudioCleanupResult(removed=removed, freed_bytes=freed)


def cleanup_successful_audio(
    audio_path: Path,
    *,
    audio_dir: Path,
    keep_audio: bool,
    keep_audio_days: int,
) -> AudioCleanupResult:
    """Clean captured audio after a successful transcription.

    Per-invocation `keep_audio` preserves the current capture for debugging. If a
    retention window is configured, old cached audio is still pruned.
    """
    removed = 0
    freed = 0
    if not keep_audio:
        result = remove_audio_file(audio_path)
        removed += result.removed
        freed += result.freed_bytes
        result = prune_audio_dir(audio_dir, keep_days=keep_audio_days)
        removed += result.removed
        freed += result.freed_bytes
    elif keep_audio_days > 0:
        result = prune_audio_dir(audio_dir, keep_days=keep_audio_days)
        removed += result.removed
        freed += result.freed_bytes
    return AudioCleanupResult(removed=removed, freed_bytes=freed)
