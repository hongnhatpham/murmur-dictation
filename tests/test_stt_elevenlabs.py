from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from murmur.config import SttConfig
from murmur.stt import ElevenLabsBackend, transcribe, _multipart_form_data


class ElevenLabsSttTests(unittest.TestCase):
    def test_multipart_contains_fields_and_file(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            path = Path(tmpdir_s) / "sample.wav"
            path.write_bytes(b"RIFFdemo")
            body, content_type = _multipart_form_data({"model_id": "scribe_v2"}, file_field="file", file_path=path)
        self.assertIn("multipart/form-data", content_type)
        self.assertIn(b'name="model_id"', body)
        self.assertIn(b"scribe_v2", body)
        self.assertIn(b'name="file"; filename="sample.wav"', body)
        self.assertIn(b"RIFFdemo", body)

    def test_elevenlabs_response_is_parsed(self):
        class FakeResponse:
            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def read(self):
                return json.dumps({"text": "Hello from Scribe."}).encode("utf-8")

        with tempfile.TemporaryDirectory() as tmpdir_s:
            path = Path(tmpdir_s) / "sample.wav"
            path.write_bytes(b"RIFFdemo")
            backend = ElevenLabsBackend()
            backend.api_key = "test-key"
            with patch("murmur.stt.urllib.request.urlopen", return_value=FakeResponse()):
                self.assertEqual(backend.transcribe(path), "Hello from Scribe.")

    def test_config_provider_uses_elevenlabs_backend(self):
        class FakeResponse:
            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def read(self):
                return json.dumps({"text": "Configured."}).encode("utf-8")

        with tempfile.TemporaryDirectory() as tmpdir_s:
            path = Path(tmpdir_s) / "sample.wav"
            path.write_bytes(b"RIFFdemo")
            cfg = SttConfig(provider="elevenlabs", model="scribe_v2", language="en")
            with patch.dict("os.environ", {"ELEVENLABS_API_KEY": "test-key"}, clear=False):
                with patch("murmur.stt.urllib.request.urlopen", return_value=FakeResponse()):
                    result = transcribe(path, cfg)
        self.assertEqual(result.provider, "elevenlabs")
        self.assertEqual(result.text, "Configured.")


if __name__ == "__main__":
    unittest.main()
