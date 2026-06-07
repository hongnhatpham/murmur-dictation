from __future__ import annotations

import tempfile
import time
import unittest
import wave
from pathlib import Path
from unittest.mock import patch

from murmur.config import load_config
from murmur.incremental import (
    IncrementalTranscript,
    await_incremental_prefill_for_session,
    incremental_enabled,
    incremental_prefill_for_session,
    tail_is_probably_silence,
    write_incremental_transcript,
    write_wav_snapshot,
)
from murmur.session import RecordingSession


def _write_wav(path: Path, pcm: bytes) -> None:
    with wave.open(str(path), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(16000)
        wav.writeframes(pcm)


class IncrementalTests(unittest.TestCase):
    def test_wav_snapshot_repairs_active_pw_record_header(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            tmpdir = Path(tmpdir_s)
            source = tmpdir / "active.wav"
            snapshot = tmpdir / "snapshot.wav"
            pcm = b"\x01\x00" * 16000
            source.write_bytes(b"\x00" * 44 + pcm)

            duration_ms = write_wav_snapshot(source, snapshot)

            with wave.open(str(snapshot), "rb") as wav:
                self.assertEqual(wav.getnchannels(), 1)
                self.assertEqual(wav.getframerate(), 16000)
                self.assertEqual(wav.getnframes(), 16000)
            self.assertEqual(duration_ms, 1000)

    def test_incremental_prefill_accepts_fresh_matching_local_transcript(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            tmpdir = Path(tmpdir_s)
            audio = tmpdir / "held.wav"
            _write_wav(audio, b"\x00\x00" * 16000)
            config_path = tmpdir / "config.toml"
            config_path.write_text(
                f'''
[paths]
state_dir = "{tmpdir / 'state'}"

[stt]
provider = "faster-whisper"
model = "tiny.en"
''',
                encoding="utf-8",
            )
            cfg = load_config(config_path)
            session = RecordingSession(pid=123, audio_path=audio, started_at=time.time() - 1)
            write_incremental_transcript(
                cfg.paths.state_dir,
                IncrementalTranscript(
                    audio_path=str(session.audio_path),
                    provider="faster-whisper",
                    text="hello from partial",
                    updated_at=time.time(),
                    audio_duration_ms=900,
                    source_size_bytes=28844,
                    transcription_latency_ms=80,
                ),
            )

            tx = incremental_prefill_for_session(cfg, session, duration_ms=900)

        self.assertIsNotNone(tx)
        self.assertEqual(tx.text, "hello from partial")
        self.assertEqual(tx.provider, "faster-whisper-incremental")

    def test_incremental_prefill_accepts_short_silent_tail(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            tmpdir = Path(tmpdir_s)
            audio = tmpdir / "held.wav"
            _write_wav(audio, b"\x00\x00" * 16000)
            config_path = tmpdir / "config.toml"
            config_path.write_text(
                f'''
[paths]
state_dir = "{tmpdir / 'state'}"

[stt]
provider = "faster-whisper"
model = "tiny.en"
''',
                encoding="utf-8",
            )
            cfg = load_config(config_path)
            session = RecordingSession(pid=123, audio_path=audio, started_at=time.time() - 1)
            write_incremental_transcript(
                cfg.paths.state_dir,
                IncrementalTranscript(
                    audio_path=str(session.audio_path),
                    provider="faster-whisper",
                    text="speech before pause",
                    updated_at=time.time(),
                    audio_duration_ms=900,
                    source_size_bytes=28844,
                    transcription_latency_ms=80,
                ),
            )

            tx = incremental_prefill_for_session(cfg, session, duration_ms=1000)

        self.assertIsNotNone(tx)
        self.assertEqual(tx.text, "speech before pause")

    def test_incremental_prefill_rejects_non_silent_tail(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            tmpdir = Path(tmpdir_s)
            audio = tmpdir / "held.wav"
            _write_wav(audio, b"\x00\x00" * 14400 + b"\xff\x3f" * 1600)
            config_path = tmpdir / "config.toml"
            config_path.write_text(
                f'''
[paths]
state_dir = "{tmpdir / 'state'}"

[stt]
provider = "faster-whisper"
model = "tiny.en"
''',
                encoding="utf-8",
            )
            cfg = load_config(config_path)
            session = RecordingSession(pid=123, audio_path=audio, started_at=time.time() - 1)
            write_incremental_transcript(
                cfg.paths.state_dir,
                IncrementalTranscript(
                    audio_path=str(session.audio_path),
                    provider="faster-whisper",
                    text="speech before more speech",
                    updated_at=time.time(),
                    audio_duration_ms=900,
                    source_size_bytes=28844,
                    transcription_latency_ms=80,
                ),
            )

            tx = incremental_prefill_for_session(cfg, session, duration_ms=1000)

        self.assertIsNone(tx)

    def test_incremental_prefill_rejects_stale_tail(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            tmpdir = Path(tmpdir_s)
            config_path = tmpdir / "config.toml"
            config_path.write_text(
                f'''
[paths]
state_dir = "{tmpdir / 'state'}"

[stt]
provider = "faster-whisper"
model = "tiny.en"
''',
                encoding="utf-8",
            )
            cfg = load_config(config_path)
            session = RecordingSession(pid=123, audio_path=tmpdir / "held.wav", started_at=time.time() - 5)
            write_incremental_transcript(
                cfg.paths.state_dir,
                IncrementalTranscript(
                    audio_path=str(session.audio_path),
                    provider="faster-whisper",
                    text="too early",
                    updated_at=time.time(),
                    audio_duration_ms=1000,
                    source_size_bytes=32044,
                    transcription_latency_ms=80,
                ),
            )

            tx = incremental_prefill_for_session(cfg, session, duration_ms=4000)

        self.assertIsNone(tx)

    def test_incremental_is_local_only(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            config_path = Path(tmpdir_s) / "config.toml"
            config_path.write_text('[stt]\nprovider = "groq"\n', encoding="utf-8")

            self.assertFalse(incremental_enabled(load_config(config_path)))

    def test_incremental_disabled_when_history_is_disabled(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            config_path = Path(tmpdir_s) / "config.toml"
            config_path.write_text('[stt]\nprovider = "faster-whisper"\n\n[privacy]\nhistory = false\n', encoding="utf-8")

            self.assertFalse(incremental_enabled(load_config(config_path)))

    def test_incremental_prefill_rejects_when_history_is_disabled(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            tmpdir = Path(tmpdir_s)
            audio = tmpdir / "held.wav"
            _write_wav(audio, b"\x00\x00" * 16000)
            config_path = tmpdir / "config.toml"
            config_path.write_text(
                f'''
[paths]
state_dir = "{tmpdir / 'state'}"

[stt]
provider = "faster-whisper"
model = "tiny.en"

[privacy]
history = false
''',
                encoding="utf-8",
            )
            cfg = load_config(config_path)
            session = RecordingSession(pid=123, audio_path=audio, started_at=time.time() - 1)
            write_incremental_transcript(
                cfg.paths.state_dir,
                IncrementalTranscript(
                    audio_path=str(session.audio_path),
                    provider="faster-whisper",
                    text="should not be used",
                    updated_at=time.time(),
                    audio_duration_ms=1000,
                    source_size_bytes=32044,
                    transcription_latency_ms=80,
                ),
            )

            tx = incremental_prefill_for_session(cfg, session, duration_ms=1000)

        self.assertIsNone(tx)

    def test_await_incremental_prefill_accepts_partial_written_during_wait(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            tmpdir = Path(tmpdir_s)
            audio = tmpdir / "held.wav"
            _write_wav(audio, b"\x00\x00" * 16000)
            config_path = tmpdir / "config.toml"
            config_path.write_text(
                f'''
[paths]
state_dir = "{tmpdir / 'state'}"

[stt]
provider = "faster-whisper"
model = "tiny.en"
''',
                encoding="utf-8",
            )
            cfg = load_config(config_path)
            session = RecordingSession(pid=123, audio_path=audio, started_at=time.time() - 1)

            def write_later():
                time.sleep(0.05)
                write_incremental_transcript(
                    cfg.paths.state_dir,
                    IncrementalTranscript(
                        audio_path=str(session.audio_path),
                        provider="faster-whisper",
                        text="ready after release",
                        updated_at=time.time(),
                        audio_duration_ms=1000,
                        source_size_bytes=32044,
                        transcription_latency_ms=80,
                    ),
                )

            import threading

            writer = threading.Thread(target=write_later)
            writer.start()
            try:
                with patch("murmur.incremental.incremental_worker_active", return_value=True):
                    tx = await_incremental_prefill_for_session(cfg, session, duration_ms=1000, timeout_ms=500)
            finally:
                writer.join()

        self.assertIsNotNone(tx)
        self.assertEqual(tx.text, "ready after release")

    def test_tail_silence_detector(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            tmpdir = Path(tmpdir_s)
            silent = tmpdir / "silent.wav"
            loud = tmpdir / "loud.wav"
            _write_wav(silent, b"\x00\x00" * 16000)
            _write_wav(loud, b"\x00\x00" * 8000 + b"\xff\x3f" * 8000)

            self.assertTrue(tail_is_probably_silence(silent, start_ms=500))
            self.assertFalse(tail_is_probably_silence(loud, start_ms=500))


if __name__ == "__main__":
    unittest.main()
