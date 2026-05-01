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
        self.assertEqual(cfg.insertion.paste_tool, "wtype")
        self.assertTrue(cfg.privacy.history)
        self.assertEqual(cfg.privacy.keep_audio_days, 1)


if __name__ == "__main__":
    unittest.main()
