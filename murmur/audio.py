from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

from .errors import MurmurError


def record_audio(output: Path, duration: float, recorder: str = "pw-record", target: str | None = None) -> None:
    """Record default microphone audio to a mono 16kHz WAV file."""
    if duration <= 0:
        raise MurmurError("Recording duration must be greater than zero.", "Pass --duration SECONDS, for example --duration 5.")
    if shutil.which(recorder) is None:
        raise MurmurError(
            f"Missing recorder dependency: {recorder}",
            "Install PipeWire tools (often the pipewire package) or pass --recorder with a compatible command.",
        )

    output.parent.mkdir(parents=True, exist_ok=True)
    cmd = [
        "timeout",
        "--signal=INT",
        str(duration),
        recorder,
    ]
    if target:
        cmd.extend(["--target", target])
    cmd.extend(
        [
            "--format=s16",
            "--rate=16000",
            "--channels=1",
            str(output),
        ]
    )
    result = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if result.returncode not in (0, 124):
        raise MurmurError(
            "Audio recording failed.",
            f"Command {' '.join(cmd)!r} exited {result.returncode}: {result.stderr.strip() or result.stdout.strip()}",
        )
    if not output.exists() or output.stat().st_size == 0:
        raise MurmurError(
            "Audio recording produced no data.",
            "Check that the default microphone works and PipeWire can see it (try `wpctl status` and `pw-record test.wav`).",
        )
