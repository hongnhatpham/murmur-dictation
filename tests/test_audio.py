from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from murmur.audio import record_audio


class AudioTests(unittest.TestCase):
    def test_record_audio_passes_pipewire_target(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            audio_path = Path(tmpdir_s) / "capture.wav"

            def fake_run(cmd, **_kwargs):
                audio_path.write_bytes(b"RIFF")
                return SimpleNamespace(returncode=124, stdout="", stderr="")

            with patch("murmur.audio.shutil.which", return_value="/usr/bin/pw-record"), patch("murmur.audio.subprocess.run", side_effect=fake_run) as run:
                record_audio(audio_path, 1.0, target="alsa_input.test")

        cmd = run.call_args.args[0]
        self.assertIn("--target", cmd)
        self.assertEqual(cmd[cmd.index("--target") + 1], "alsa_input.test")
        self.assertLess(cmd.index("--target"), cmd.index("--format=s16"))

    def test_record_audio_keeps_default_source_when_target_unset(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            audio_path = Path(tmpdir_s) / "capture.wav"

            def fake_run(cmd, **_kwargs):
                audio_path.write_bytes(b"RIFF")
                return SimpleNamespace(returncode=124, stdout="", stderr="")

            with patch("murmur.audio.shutil.which", return_value="/usr/bin/pw-record"), patch("murmur.audio.subprocess.run", side_effect=fake_run) as run:
                record_audio(audio_path, 1.0)

        self.assertNotIn("--target", run.call_args.args[0])


if __name__ == "__main__":
    unittest.main()
