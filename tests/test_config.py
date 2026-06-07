import tempfile
import unittest
from pathlib import Path

from murmur.config import load_config, write_default_config


class ConfigTests(unittest.TestCase):
    def test_default_config_roundtrip(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "config.toml"
            write_default_config(path)
            cfg = load_config(path)
        self.assertEqual(cfg.stt.provider, "faster-whisper")
        self.assertEqual(cfg.stt.model, "base.en")
        self.assertIsNone(cfg.recording.target)
        self.assertEqual(cfg.command.max_selection_chars, 12000)
        self.assertEqual(cfg.insertion.paste_tool, "wtype")
        self.assertTrue(cfg.privacy.history)
        self.assertEqual(cfg.privacy.keep_audio_days, 1)

    def test_recording_target_config(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "config.toml"
            path.write_text('[recording]\ntarget = "alsa_input.test"\n', encoding="utf-8")
            cfg = load_config(path)

        self.assertEqual(cfg.recording.target, "alsa_input.test")


if __name__ == "__main__":
    unittest.main()
