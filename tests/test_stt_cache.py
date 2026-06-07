from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from murmur.config import SttConfig
from murmur.stt import SttError, backend_for_config, clear_backend_cache, transcribe, warm_stt_backend


class SttCacheTests(unittest.TestCase):
    def tearDown(self):
        clear_backend_cache()

    def test_faster_whisper_model_is_reused_in_process(self):
        class FakeWhisperModel:
            loads = 0

            def __init__(self, *_args, **_kwargs):
                type(self).loads += 1

            def transcribe(self, *_args, **_kwargs):
                return [SimpleNamespace(text=" hello ")], SimpleNamespace()

        with tempfile.TemporaryDirectory() as tmpdir_s:
            audio = Path(tmpdir_s) / "sample.wav"
            audio.write_bytes(b"RIFFdemo")
            cfg = SttConfig(provider="faster-whisper", model="tiny.en", language="en")
            module = SimpleNamespace(WhisperModel=FakeWhisperModel)
            with patch.dict(sys.modules, {"faster_whisper": module}):
                first = transcribe(audio, cfg)
                second = transcribe(audio, cfg)

        self.assertEqual(FakeWhisperModel.loads, 1)
        self.assertEqual(first.text, "hello")
        self.assertEqual(second.text, "hello")

    def test_warm_stt_backend_preloads_faster_whisper_once(self):
        class FakeWhisperModel:
            loads = 0

            def __init__(self, *_args, **_kwargs):
                type(self).loads += 1

            def transcribe(self, *_args, **_kwargs):
                return [SimpleNamespace(text=" warmed ")], SimpleNamespace()

        with tempfile.TemporaryDirectory() as tmpdir_s:
            audio = Path(tmpdir_s) / "sample.wav"
            audio.write_bytes(b"RIFFdemo")
            cfg = SttConfig(provider="faster-whisper", model="tiny.en", language="en")
            module = SimpleNamespace(WhisperModel=FakeWhisperModel)
            with patch.dict(sys.modules, {"faster_whisper": module}):
                self.assertEqual(warm_stt_backend(cfg), "faster-whisper")
                result = transcribe(audio, cfg)

        self.assertEqual(FakeWhisperModel.loads, 1)
        self.assertEqual(result.text, "warmed")

    def test_remote_stt_backends_are_not_cached(self):
        groq_cfg = SttConfig(provider="groq", model="whisper-large-v3-turbo", language="en")
        elevenlabs_cfg = SttConfig(provider="elevenlabs", model="scribe_v2", language="en")

        self.assertIsNot(backend_for_config(groq_cfg), backend_for_config(groq_cfg))
        self.assertIsNot(backend_for_config(elevenlabs_cfg), backend_for_config(elevenlabs_cfg))

    def test_missing_faster_whisper_dependency_keeps_install_hint(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            audio = Path(tmpdir_s) / "sample.wav"
            audio.write_bytes(b"RIFFdemo")
            cfg = SttConfig(provider="faster-whisper", model="tiny.en", language="en")
            with patch.dict(sys.modules, {"faster_whisper": None}):
                with self.assertRaises(SttError) as ctx:
                    transcribe(audio, cfg)

        self.assertIn("python -m pip install faster-whisper", str(ctx.exception))
        self.assertNotIn("Model=", str(ctx.exception))


if __name__ == "__main__":
    unittest.main()
