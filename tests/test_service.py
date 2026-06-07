from __future__ import annotations

import argparse
import socket
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from murmur.config import load_config
from murmur.service import _handle_connection, _recording_monitor, handle_request, request_stop_recording


class ServiceTests(unittest.TestCase):
    def test_handle_stop_recording_request_captures_output(self):
        seen: list[argparse.Namespace] = []

        def stop_recording(args: argparse.Namespace) -> int:
            seen.append(args)
            print("final text")
            print("latency_ms=123", file=sys.stderr)
            return 7

        response = handle_request(
            {
                "version": 1,
                "command": "stop-recording",
                "args": {"mode": "raw", "paste": True, "no_paste": False, "keep_audio": True},
            },
            stop_recording,
        )

        self.assertEqual(response.returncode, 7)
        self.assertEqual(response.stdout, "final text\n")
        self.assertEqual(response.stderr, "latency_ms=123\n")
        self.assertEqual(seen[0].mode, "raw")
        self.assertTrue(seen[0].paste)
        self.assertFalse(seen[0].no_paste)
        self.assertTrue(seen[0].keep_audio)

    def test_missing_service_socket_falls_back_to_direct_cli(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            config_path = Path(tmpdir_s) / "config.toml"
            config_path.write_text(f'[paths]\nstate_dir = "{Path(tmpdir_s) / "state"}"\n', encoding="utf-8")
            cfg = load_config(config_path)
            args = argparse.Namespace(mode=None, paste=False, no_paste=False, keep_audio=False)

            response = request_stop_recording(cfg, args)

        self.assertIsNone(response)

    def test_rejects_unknown_service_command(self):
        response = handle_request({"version": 1, "command": "unknown", "args": {}}, lambda _args: 0)

        self.assertEqual(response.returncode, 1)
        self.assertIn("unsupported service command", response.stderr)

    def test_stop_recording_exception_returns_failure_response(self):
        def stop_recording(_args: argparse.Namespace) -> int:
            print("partial output")
            raise RuntimeError("boom")

        response = handle_request({"version": 1, "command": "stop-recording", "args": {}}, stop_recording)

        self.assertEqual(response.returncode, 1)
        self.assertEqual(response.stdout, "partial output\n")
        self.assertIn("service request failed unexpectedly", response.stderr)

    def test_empty_probe_connection_is_ignored(self):
        called = False

        def stop_recording(_args: argparse.Namespace) -> int:
            nonlocal called
            called = True
            return 0

        server, client = socket.socketpair()
        try:
            client.close()
            _handle_connection(server, stop_recording)
        finally:
            server.close()

        self.assertFalse(called)

    def test_recording_monitor_starts_incremental_worker_for_active_session(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            tmpdir = Path(tmpdir_s)
            config_path = tmpdir / "config.toml"
            config_path.write_text(f'[paths]\nstate_dir = "{tmpdir / "state"}"\n', encoding="utf-8")
            cfg = load_config(config_path)
            cfg.paths.state_dir.mkdir(parents=True)
            session_path = cfg.paths.state_dir / "recording-session.json"
            session_path.write_text(
                '{"pid":123,"audio_path":"' + str(tmpdir / "held.wav") + '","started_at":1}',
                encoding="utf-8",
            )
            import threading

            stop_event = threading.Event()
            started = []

            def stop_after_start(_cfg, session):
                started.append(session.audio_path)
                stop_event.set()
                return True

            with patch("murmur.service.process_alive", return_value=True), patch(
                "murmur.service.start_incremental_transcription", side_effect=stop_after_start
            ):
                _recording_monitor(cfg, stop_event)

        self.assertEqual(started, [tmpdir / "held.wav"])


if __name__ == "__main__":
    unittest.main()
