from __future__ import annotations

import shutil
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path

from . import paths


@dataclass(frozen=True)
class Recording:
    path: Path
    duration_ms: int


class RecordingError(RuntimeError):
    pass


def record(duration: float = 5.0, output: Path | None = None, target: str | None = None) -> Recording:
    """Record a fixed-duration mono 16 kHz WAV clip using PipeWire's pw-record."""
    if duration <= 0:
        raise RecordingError("Recording duration must be greater than zero; pass --duration SECONDS.")
    if shutil.which("pw-record") is None:
        raise RecordingError("Missing `pw-record`. Install PipeWire tools first, then verify with `pw-record test.wav`.")
    if shutil.which("timeout") is None:
        raise RecordingError("Missing `timeout` from GNU coreutils; needed for fixed-duration recording.")
    paths.ensure_runtime_dirs()
    output = output or (paths.audio_cache_dir() / f"murmur-{int(time.time() * 1000)}.wav")
    start = time.perf_counter()
    cmd = ["timeout", "--signal=INT", str(duration), "pw-record"]
    if target:
        cmd.extend(["--target", target])
    cmd.extend(["--format=s16", "--rate=16000", "--channels=1", str(output)])
    proc = subprocess.run(cmd, capture_output=True, text=True)
    elapsed_ms = int((time.perf_counter() - start) * 1000)
    if proc.returncode not in (0, 124):
        raise RecordingError(proc.stderr.strip() or "pw-record failed")
    if not output.exists() or output.stat().st_size == 0:
        raise RecordingError("Recording produced no audio file. Check the default microphone with `wpctl status`.")
    return Recording(path=output, duration_ms=elapsed_ms)
