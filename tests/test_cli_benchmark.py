from __future__ import annotations

import json
import tempfile
import unittest
from argparse import Namespace
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
from unittest.mock import patch

from murmur.cli import cmd_benchmark
from murmur.config import load_config
from murmur.stt import Transcription


class CliBenchmarkTests(unittest.TestCase):
    def test_benchmark_runs_default_local_and_groq_profiles_without_insertion(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            tmpdir = Path(tmpdir_s)
            config_path = tmpdir / "config.toml"
            config_path.write_text(
                f'''
[paths]
state_dir = "{tmpdir / 'state'}"

[stt]
provider = "groq"
model = "whisper-large-v3-turbo"
language = "en"
''',
                encoding="utf-8",
            )
            audio_path = tmpdir / "sample.wav"
            audio_path.write_bytes(b"fake wav")
            output_path = tmpdir / "bench.jsonl"
            calls = []

            def fake_transcribe(path, stt, dictionary_terms=None):
                calls.append((path, stt.provider, stt.model, stt.language))
                return Transcription(text=f"{stt.provider}:{stt.model}", provider=stt.provider)

            args = Namespace(
                audio=[audio_path],
                local_models=None,
                groq_model=None,
                skip_groq=False,
                language=None,
                output=output_path,
            )
            with patch("murmur.cli.load_config", return_value=load_config(config_path)), patch(
                "murmur.cli.transcribe", side_effect=fake_transcribe
            ), patch("murmur.cli.insert_text", side_effect=AssertionError("benchmark must not insert")), redirect_stdout(StringIO()) as stdout:
                rc = cmd_benchmark(args)

            rows = [json.loads(line) for line in output_path.read_text(encoding="utf-8").splitlines()]
            output = stdout.getvalue()

        self.assertEqual(rc, 0)
        self.assertEqual(
            [(provider, model) for _, provider, model, _ in calls],
            [
                ("faster-whisper", "tiny.en"),
                ("faster-whisper", "base.en"),
                ("groq", "whisper-large-v3-turbo"),
            ],
        )
        self.assertEqual(len(rows), 3)
        self.assertEqual(rows[0]["insertion_status"], "skipped")
        self.assertIn("insertion=skipped", output)
        self.assertIn('transcript="faster-whisper:tiny.en"', output)


if __name__ == "__main__":
    unittest.main()
