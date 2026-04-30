from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from murmur.errors import MurmurError
from murmur.session import RecordingSession, assert_no_active_session, read_session, session_from_dict, session_path


class SessionTests(unittest.TestCase):
    def test_session_round_trip(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            tmp_path = Path(tmpdir_s)
            session = RecordingSession(
                pid=123,
                audio_path=tmp_path / "sample.wav",
                started_at=42.5,
                mode="raw",
                paste=True,
                keep_audio=True,
            )
            path = tmp_path / "session.json"
            path.write_text(session.to_json(), encoding="utf-8")

            loaded = read_session(path)

        self.assertEqual(loaded, session)

    def test_session_from_dict_defaults_optional_fields(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            tmp_path = Path(tmpdir_s)
            session = session_from_dict({"pid": "5", "audio_path": str(tmp_path / "a.wav"), "started_at": "1.25"})

        self.assertEqual(session.pid, 5)
        self.assertEqual(session.audio_path, tmp_path / "a.wav")
        self.assertEqual(session.started_at, 1.25)
        self.assertIsNone(session.mode)
        self.assertFalse(session.paste)
        self.assertFalse(session.keep_audio)

    def test_assert_no_active_session_removes_stale_session(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            tmp_path = Path(tmpdir_s)
            path = session_path(tmp_path)
            path.write_text(
                json.dumps({"pid": 99999999, "audio_path": str(tmp_path / "a.wav"), "started_at": 1}),
                encoding="utf-8",
            )

            assert_no_active_session(path)

            self.assertFalse(path.exists())

    def test_assert_no_active_session_rejects_corrupt_file(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            path = session_path(Path(tmpdir_s))
            path.write_text("not-json", encoding="utf-8")

            with self.assertRaisesRegex(MurmurError, "session file is corrupt"):
                assert_no_active_session(path)


if __name__ == "__main__":
    unittest.main()
