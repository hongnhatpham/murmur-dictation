from __future__ import annotations

import fcntl
import json
import os
import signal
import shutil
import subprocess
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

from .errors import MurmurError

SESSION_FILENAME = "recording-session.json"
LOCK_FILENAME = "recording-session.lock"


@dataclass(frozen=True)
class RecordingSession:
    pid: int
    audio_path: Path
    started_at: float
    mode: str | None = None
    paste: bool = False
    keep_audio: bool = False

    def to_json(self) -> str:
        return json.dumps({**asdict(self), "audio_path": str(self.audio_path)}, indent=2, sort_keys=True)


def session_path(state_dir: Path) -> Path:
    return state_dir / SESSION_FILENAME


def lock_path(state_dir: Path) -> Path:
    return state_dir / LOCK_FILENAME


def session_from_dict(data: dict[str, Any]) -> RecordingSession:
    return RecordingSession(
        pid=int(data["pid"]),
        audio_path=Path(str(data["audio_path"])).expanduser(),
        started_at=float(data["started_at"]),
        mode=None if data.get("mode") in (None, "") else str(data.get("mode")),
        paste=bool(data.get("paste", False)),
        keep_audio=bool(data.get("keep_audio", False)),
    )


def read_session(path: Path) -> RecordingSession:
    return session_from_dict(json.loads(path.read_text(encoding="utf-8")))


def process_alive(pid: int) -> bool:
    if pid <= 0:
        return False
    try:
        stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
        if ") Z" in stat:
            return False
    except OSError:
        pass
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def acquire_lock(path: Path):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd = path.open("w")
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError as exc:
        fd.close()
        raise MurmurError(
            "Another Murmur recording operation is already running.",
            "Wait for it to finish, or check for a stuck murmur command before retrying.",
        ) from exc
    return fd


def release_lock(path: Path, fd) -> None:
    try:
        fcntl.flock(fd, fcntl.LOCK_UN)
    finally:
        fd.close()
        path.unlink(missing_ok=True)


def clear_session(state_dir: Path) -> None:
    session_path(state_dir).unlink(missing_ok=True)


def assert_no_active_session(path: Path) -> None:
    if not path.exists():
        return
    try:
        session = read_session(path)
    except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError) as exc:
        raise MurmurError(
            "Murmur recording session file is corrupt.",
            f"Remove {path} if no recording is active, then try again.",
        ) from exc
    if process_alive(session.pid):
        raise MurmurError("Recording already in progress.", f"pid={session.pid}; run `murmur cancel-recording` if needed.")
    path.unlink(missing_ok=True)


def load_active_session(state_dir: Path) -> RecordingSession:
    path = session_path(state_dir)
    if not path.exists():
        raise MurmurError("No recording in progress.", "Run `murmur start-recording` first.")
    try:
        session = read_session(path)
    except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError) as exc:
        raise MurmurError(
            "Murmur recording session file is corrupt.",
            f"Remove {path} if no recording is active, then try again.",
        ) from exc
    if not process_alive(session.pid):
        clear_session(state_dir)
        raise MurmurError("Recording process is not running.", "The stale session was cleared; start recording again.")
    return session


def start_recording_session(
    *,
    state_dir: Path,
    audio_dir: Path,
    recorder: str = "pw-record",
    mode: str | None = None,
    paste: bool = False,
    keep_audio: bool = False,
) -> RecordingSession:
    assert_no_active_session(session_path(state_dir))
    if shutil.which(recorder) is None:
        raise MurmurError(f"Missing recorder dependency: {recorder}", "Install PipeWire tools first.")
    state_dir.mkdir(parents=True, exist_ok=True)
    audio_dir.mkdir(parents=True, exist_ok=True)
    audio_path = audio_dir / f"murmur-held-{int(time.time() * 1000)}.wav"
    cmd = [recorder, "--format=s16", "--rate=16000", "--channels=1", str(audio_path)]
    log = (state_dir / "recording-session.log").open("ab")
    try:
        proc = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    finally:
        log.close()
    session = RecordingSession(proc.pid, audio_path, time.time(), mode=mode, paste=paste, keep_audio=keep_audio)
    path = session_path(state_dir)
    path.write_text(session.to_json(), encoding="utf-8")
    path.chmod(0o600)
    return session


def stop_recording_process(session: RecordingSession, *, timeout: float = 5.0, validate_audio: bool = True) -> None:
    if process_alive(session.pid):
        try:
            os.killpg(session.pid, signal.SIGINT)
        except ProcessLookupError:
            pass
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline and process_alive(session.pid):
            time.sleep(0.05)
        if process_alive(session.pid):
            try:
                os.killpg(session.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            deadline = time.monotonic() + 2.0
            while time.monotonic() < deadline and process_alive(session.pid):
                time.sleep(0.05)
    if process_alive(session.pid):
        raise MurmurError("Recorder did not stop cleanly.", f"Manual cleanup may be required for pid {session.pid}.")
    if validate_audio and (not session.audio_path.exists() or session.audio_path.stat().st_size == 0):
        raise MurmurError("Recording produced no audio.", "Check microphone/PipeWire and try again.")


# Compatibility wrappers used by simpler callers/tests.
def start_recording(config):
    return start_recording_session(state_dir=config.paths.state_dir, audio_dir=config.paths.debug_audio_dir)


def stop_recording(config) -> Path:
    session = load_active_session(config.paths.state_dir)
    stop_recording_process(session)
    clear_session(config.paths.state_dir)
    return session.audio_path


def cancel_recording(config) -> bool:
    try:
        session = load_active_session(config.paths.state_dir)
    except MurmurError:
        return False
    if process_alive(session.pid):
        stop_recording_process(session, validate_audio=False)
    session.audio_path.unlink(missing_ok=True)
    clear_session(config.paths.state_dir)
    return True
