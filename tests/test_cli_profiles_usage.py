from __future__ import annotations

import tempfile
import unittest
from contextlib import redirect_stdout
from io import StringIO
from argparse import Namespace
from pathlib import Path
from unittest.mock import patch

from murmur.cli import _set_toml_value, cmd_provider_profile
from murmur.config import load_config


class CliProfileTests(unittest.TestCase):
    def test_set_toml_value_updates_only_named_section(self):
        text = '''[stt]\nprovider = "faster-whisper"\nmodel = "base.en"\n\n[correction]\nprovider = "ollama"\nmodel = "qwen3:1.7b"\n'''
        updated = _set_toml_value(text, "stt", "provider", '"groq"')
        self.assertIn('[stt]\nprovider = "groq"', updated)
        self.assertIn('[correction]\nprovider = "ollama"', updated)

    def test_set_toml_value_adds_missing_key(self):
        updated = _set_toml_value('[stt]\nprovider = "groq"\n', "stt", "language", '"en"')
        self.assertIn('language = "en"', updated)

    def test_local_profile_disables_correction(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            config_path = Path(tmpdir_s) / "config.toml"
            config_path.write_text(
                '[stt]\nprovider = "groq"\nmodel = "whisper-large-v3-turbo"\n\n[correction]\nenabled = true\nprovider = "groq"\n',
                encoding="utf-8",
            )
            with patch("murmur.cli.load_config", return_value=load_config(config_path)), redirect_stdout(StringIO()):
                rc = cmd_provider_profile(Namespace(profile="local"))
            updated = config_path.read_text(encoding="utf-8")

        self.assertEqual(rc, 0)
        self.assertIn('provider = "faster-whisper"', updated)
        self.assertIn('model = "small.en"', updated)
        self.assertIn("enabled = false", updated)
        self.assertIn('provider = "ollama"', updated)

    def test_groq_profile_disables_cloud_correction_by_default(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            config_path = Path(tmpdir_s) / "config.toml"
            config_path.write_text(
                '[stt]\nprovider = "faster-whisper"\nmodel = "base.en"\n\n[correction]\nenabled = true\nprovider = "ollama"\n',
                encoding="utf-8",
            )
            with patch("murmur.cli.load_config", return_value=load_config(config_path)), redirect_stdout(StringIO()):
                rc = cmd_provider_profile(Namespace(profile="groq"))
            updated = config_path.read_text(encoding="utf-8")

        self.assertEqual(rc, 0)
        self.assertIn('provider = "groq"', updated)
        self.assertIn('model = "whisper-large-v3-turbo"', updated)
        self.assertIn("enabled = false", updated)
        self.assertIn('endpoint = "https://api.groq.com/openai/v1/chat/completions"', updated)

    def test_cloud_profile_is_groq_alias(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            config_path = Path(tmpdir_s) / "config.toml"
            config_path.write_text("[stt]\nprovider = \"faster-whisper\"\n\n[correction]\nenabled = true\n", encoding="utf-8")
            with patch("murmur.cli.load_config", return_value=load_config(config_path)), redirect_stdout(StringIO()):
                rc = cmd_provider_profile(Namespace(profile="cloud"))
            updated = config_path.read_text(encoding="utf-8")

        self.assertEqual(rc, 0)
        self.assertIn('provider = "groq"', updated)
        self.assertIn("enabled = false", updated)


if __name__ == "__main__":
    unittest.main()
