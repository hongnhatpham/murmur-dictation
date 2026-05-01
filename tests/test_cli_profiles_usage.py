from __future__ import annotations

import unittest

from murmur.cli import _set_toml_value


class CliProfileTests(unittest.TestCase):
    def test_set_toml_value_updates_only_named_section(self):
        text = '''[stt]\nprovider = "faster-whisper"\nmodel = "base.en"\n\n[correction]\nprovider = "ollama"\nmodel = "qwen3:1.7b"\n'''
        updated = _set_toml_value(text, "stt", "provider", '"groq"')
        self.assertIn('[stt]\nprovider = "groq"', updated)
        self.assertIn('[correction]\nprovider = "ollama"', updated)

    def test_set_toml_value_adds_missing_key(self):
        updated = _set_toml_value('[stt]\nprovider = "groq"\n', "stt", "language", '"en"')
        self.assertIn('language = "en"', updated)


if __name__ == "__main__":
    unittest.main()
