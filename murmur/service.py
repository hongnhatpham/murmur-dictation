from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
import signal
import socket
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from .config import MurmurConfig
from .stt import warm_stt_backend

SOCKET_FILENAME = "murmur.sock"
PROTOCOL_VERSION = 1


@dataclass(frozen=True)
class ServiceResponse:
    returncode: int
    stdout: str = ""
    stderr: str = ""


StopRecordingFunc = Callable[[argparse.Namespace], int]


def socket_path(state_dir: Path) -> Path:
    return state_dir / SOCKET_FILENAME


def request_stop_recording(cfg: MurmurConfig, args: argparse.Namespace) -> ServiceResponse | None:
    if os.environ.get("MURMUR_SERVICE_BYPASS") or os.environ.get("MURMUR_DISABLE_SERVICE"):
        return None
    path = socket_path(cfg.paths.state_dir)
    if not path.exists():
        return None
    payload = {
        "version": PROTOCOL_VERSION,
        "command": "stop-recording",
        "args": {
            "mode": args.mode,
            "paste": bool(args.paste),
            "no_paste": bool(args.no_paste),
            "keep_audio": bool(args.keep_audio),
        },
    }
    sent = False
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.settimeout(float(os.environ.get("MURMUR_SERVICE_CONNECT_TIMEOUT", "0.2")))
            sock.connect(str(path))
            sock.sendall(json.dumps(payload).encode("utf-8") + b"\n")
            sent = True
            sock.shutdown(socket.SHUT_WR)
            sock.settimeout(None)
            raw = _recv_all(sock)
    except OSError as exc:
        if not sent:
            return None
        return ServiceResponse(1, stderr=f"murmur: service request failed after dispatch: {exc}\n")
    try:
        data = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        return ServiceResponse(1, stderr=f"murmur: invalid service response: {exc}\n")
    return ServiceResponse(
        returncode=int(data.get("returncode", 1)),
        stdout=str(data.get("stdout", "")),
        stderr=str(data.get("stderr", "")),
    )


def run_service(cfg: MurmurConfig, *, stop_recording_func: StopRecordingFunc) -> int:
    cfg.paths.state_dir.mkdir(parents=True, exist_ok=True)
    path = socket_path(cfg.paths.state_dir)
    _remove_stale_socket(path)
    try:
        warmed = warm_stt_backend(cfg.stt)
        warm_status = f"stt={warmed}"
    except Exception as exc:
        warm_status = f"stt_warm=failed error={exc}"
    stop = False

    def request_shutdown(_signum, _frame) -> None:
        nonlocal stop
        stop = True
        raise KeyboardInterrupt

    previous_sigterm = signal.signal(signal.SIGTERM, request_shutdown)
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as server:
        server.bind(str(path))
        os.chmod(path, 0o600)
        server.listen(1)
        print(f"murmur service ready socket={path} {warm_status}", flush=True)
        try:
            while not stop:
                conn, _addr = server.accept()
                with conn:
                    _handle_connection(conn, stop_recording_func)
        except KeyboardInterrupt:
            return 0
        finally:
            signal.signal(signal.SIGTERM, previous_sigterm)
            path.unlink(missing_ok=True)


def handle_request(payload: dict[str, object], stop_recording_func: StopRecordingFunc) -> ServiceResponse:
    if payload.get("version") != PROTOCOL_VERSION:
        return ServiceResponse(1, stderr="murmur: unsupported service protocol\n")
    command = payload.get("command")
    if command != "stop-recording":
        return ServiceResponse(1, stderr=f"murmur: unsupported service command: {command}\n")
    raw_args = payload.get("args")
    if not isinstance(raw_args, dict):
        return ServiceResponse(1, stderr="murmur: missing service command args\n")
    args = argparse.Namespace(
        mode=raw_args.get("mode") or None,
        paste=bool(raw_args.get("paste", False)),
        no_paste=bool(raw_args.get("no_paste", False)),
        keep_audio=bool(raw_args.get("keep_audio", False)),
    )
    stdout = io.StringIO()
    stderr = io.StringIO()
    try:
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            rc = stop_recording_func(args)
    except Exception:
        return ServiceResponse(
            1,
            stdout.getvalue(),
            stderr.getvalue() + "murmur: service request failed unexpectedly\n",
        )
    return ServiceResponse(rc, stdout.getvalue(), stderr.getvalue())


def _handle_connection(conn: socket.socket, stop_recording_func: StopRecordingFunc) -> None:
    payload = _read_request(conn)
    if not payload:
        return
    response = handle_request(payload, stop_recording_func)
    try:
        conn.sendall(json.dumps(response.__dict__).encode("utf-8"))
    except OSError:
        return


def _recv_all(sock: socket.socket) -> bytes:
    chunks: list[bytes] = []
    while True:
        chunk = sock.recv(65536)
        if not chunk:
            break
        chunks.append(chunk)
    return b"".join(chunks)


def _read_request(conn: socket.socket) -> dict[str, object]:
    raw = _recv_all(conn)
    if not raw:
        return {}
    try:
        data = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return {}
    return data if isinstance(data, dict) else {}


def _remove_stale_socket(path: Path) -> None:
    if not path.exists():
        return
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.settimeout(0.2)
            sock.connect(str(path))
    except OSError:
        path.unlink(missing_ok=True)
        return
    raise RuntimeError(f"Murmur service already running at {path}")
