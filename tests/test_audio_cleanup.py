import os
import tempfile
import time
import unittest
from pathlib import Path

from murmur.audio_cleanup import cleanup_successful_audio, prune_audio_dir


class AudioCleanupTests(unittest.TestCase):
    def test_successful_audio_is_removed_by_default(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            audio_dir = Path(tmpdir)
            audio = audio_dir / "murmur.wav"
            audio.write_bytes(b"audio")

            result = cleanup_successful_audio(audio, audio_dir=audio_dir, keep_audio=False, keep_audio_days=0)

            self.assertEqual(result.removed, 1)
            self.assertEqual(result.freed_bytes, 5)
            self.assertFalse(audio.exists())

    def test_keep_audio_preserves_current_capture(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            audio_dir = Path(tmpdir)
            audio = audio_dir / "murmur.wav"
            audio.write_bytes(b"audio")

            result = cleanup_successful_audio(audio, audio_dir=audio_dir, keep_audio=True, keep_audio_days=0)

            self.assertEqual(result.removed, 0)
            self.assertTrue(audio.exists())

    def test_prune_audio_dir_respects_retention_window(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            audio_dir = Path(tmpdir)
            old_audio = audio_dir / "old.wav"
            new_audio = audio_dir / "new.wav"
            note = audio_dir / "note.txt"
            old_audio.write_bytes(b"old")
            new_audio.write_bytes(b"new")
            note.write_text("not audio")
            old_time = time.time() - 3 * 24 * 60 * 60
            os.utime(old_audio, (old_time, old_time))

            result = prune_audio_dir(audio_dir, keep_days=1)

            self.assertEqual(result.removed, 1)
            self.assertFalse(old_audio.exists())
            self.assertTrue(new_audio.exists())
            self.assertTrue(note.exists())


if __name__ == "__main__":
    unittest.main()
