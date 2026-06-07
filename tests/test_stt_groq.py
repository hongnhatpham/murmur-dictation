from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from murmur.config import SttConfig
from murmur.stt import GroqBackend, transcribe


class GroqSttTests(unittest.TestCase):
    def test_groq_response_is_parsed(self):
        class FakeResponse:
            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def read(self):
                return json.dumps({"text": "Hello from Groq."}).encode("utf-8")

        with tempfile.TemporaryDirectory() as tmpdir_s:
            path = Path(tmpdir_s) / "sample.wav"
            path.write_bytes(b"RIFFdemo")
            backend = GroqBackend()
            backend.api_key = "test-key"
            with patch("murmur.stt.urllib.request.urlopen", return_value=FakeResponse()):
                self.assertEqual(backend.transcribe(path), "Hello from Groq.")

    def test_groq_does_not_send_dictionary_prompt(self):
        class FakeResponse:
            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def read(self):
                return json.dumps({"text": "I think we should remove it completely."}).encode("utf-8")

        with tempfile.TemporaryDirectory() as tmpdir_s:
            path = Path(tmpdir_s) / "sample.wav"
            path.write_bytes(b"RIFFdemo")
            backend = GroqBackend()
            backend.api_key = "test-key"
            with patch("murmur.stt.urllib.request.urlopen", return_value=FakeResponse()) as urlopen:
                self.assertEqual(backend.transcribe(path, dictionary_terms=["Gemma 4", "Murmur", "Niri", "Qwen3 1.7B"]), "I think we should remove it completely.")

        request = urlopen.call_args.args[0]
        body = request.data
        self.assertNotIn(b'name="prompt"', body)
        self.assertNotIn(b"Vocabulary:", body)
        self.assertNotIn(b"Gemma 4", body)

    def test_config_provider_uses_groq_backend(self):
        class FakeResponse:
            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def read(self):
                return json.dumps({"text": "Configured Groq."}).encode("utf-8")

        with tempfile.TemporaryDirectory() as tmpdir_s:
            path = Path(tmpdir_s) / "sample.wav"
            path.write_bytes(b"RIFFdemo")
            cfg = SttConfig(provider="groq", model="whisper-large-v3-turbo", language="en")
            with patch.dict("os.environ", {"GROQ_API_KEY": "test-key"}, clear=False):
                with patch("murmur.stt.urllib.request.urlopen", return_value=FakeResponse()):
                    result = transcribe(path, cfg)
        self.assertEqual(result.provider, "groq")
        self.assertEqual(result.text, "Configured Groq.")


if __name__ == "__main__":
    unittest.main()
