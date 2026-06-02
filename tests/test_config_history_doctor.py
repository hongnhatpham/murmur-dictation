from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from murmur.config import load_config
from murmur.doctor import Check, format_checks, has_required_failures, run_checks
from murmur.history import HistoryStore, format_entries


class ConfigHistoryDoctorTests(unittest.TestCase):
    def test_load_config_defaults_when_missing(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            cfg = load_config(Path(tmpdir) / "missing.toml")
        self.assertEqual(cfg.stt.provider, "faster-whisper")
        self.assertEqual(cfg.stt.model, "base.en")
        self.assertEqual(cfg.cleanup.default_mode, "clean")
        self.assertTrue(cfg.privacy.history)
        self.assertEqual(cfg.paths.history_path.name, "history.sqlite3")

    def test_load_config_overrides_paths_and_stt(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            tmpdir = Path(tmpdir_s)
            config_path = tmpdir / "config.toml"
            history_path = tmpdir / "state" / "custom.sqlite3"
            config_path.write_text(
                f'''
[paths]
history_db = "{history_path}"
model_dir = "{tmpdir / 'models'}"

[stt]
provider = "whisper-cpp"
whisper_cpp_model = "{tmpdir / 'models' / 'ggml.bin'}"

[privacy]
history = false
''',
                encoding="utf-8",
            )
            cfg = load_config(config_path)
            self.assertEqual(cfg.paths.history_path, history_path)
            self.assertEqual(cfg.stt.provider, "whisper-cpp")
            self.assertEqual(cfg.stt.whisper_cpp_model, tmpdir / "models" / "ggml.bin")
            self.assertFalse(cfg.privacy.history)

    def test_history_add_list_last(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            store = HistoryStore(Path(tmpdir) / "history.sqlite3")
            entry_id = store.add(
                mode="clean",
                provider="test",
                transcript="hello comma world",
                final_text="Hello, world.",
                insertion_status="copied-only",
            )
            self.assertEqual(entry_id, 1)
            last = store.last()
            self.assertIsNotNone(last)
            self.assertEqual(last.final_text, "Hello, world.")
            formatted = format_entries(store.recent())
        self.assertIn("copied-only", formatted)
        self.assertIn("Hello, world.", formatted)

    def test_doctor_required_failures_only_count_required(self):
        checks = [
            Check("optional", False, "missing optional", required=False),
            Check("required", True, "ok", required=True),
        ]
        self.assertFalse(has_required_failures(checks))
        self.assertIn("[WARN] optional", format_checks(checks))

    def test_doctor_redacts_available_groq_key(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            config_path = Path(tmpdir_s) / "config.toml"
            config_path.write_text('[stt]\nprovider = "groq"\n', encoding="utf-8")
            cfg = load_config(config_path)
            with patch.dict("os.environ", {"MURMUR_GROQ_API_KEY": "secret-value"}, clear=False):
                checks = run_checks(cfg)

        groq = next(check for check in checks if check.name == "groq api key")
        self.assertTrue(groq.ok)
        self.assertEqual(groq.detail, "key available; value hidden")
        self.assertNotIn("secret-value", format_checks(checks))


if __name__ == "__main__":
    unittest.main()
