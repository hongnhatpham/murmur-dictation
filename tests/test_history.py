import unittest

from murmur.history import HistoryStore, format_entries, format_latency_metrics


class HistoryTests(unittest.TestCase):
    def test_append_and_load_recent_history(self):
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as tmpdir:
            store = HistoryStore(Path(tmpdir) / "history.sqlite3")
            store.add(
                mode="clean",
                provider="test",
                transcript="hi",
                deterministic_text="Hi.",
                final_text="Hi.",
                correction_provider="deterministic",
                correction_status="skipped",
                focused_app_id="Alacritty",
                app_category="terminal",
                insertion_status="copied-only",
            )
            rows = store.recent()

        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0].mode, "clean")
        self.assertEqual(rows[0].provider, "test")
        self.assertEqual(rows[0].transcript, "hi")
        self.assertEqual(rows[0].final_text, "Hi.")
        self.assertEqual(rows[0].insertion_status, "copied-only")
        self.assertEqual(rows[0].deterministic_text, "Hi.")
        self.assertEqual(rows[0].correction_provider, "deterministic")
        self.assertEqual(rows[0].correction_status, "skipped")
        self.assertEqual(rows[0].focused_app_id, "Alacritty")
        self.assertEqual(rows[0].app_category, "terminal")

    def test_format_entries(self):
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as tmpdir:
            store = HistoryStore(Path(tmpdir) / "history.sqlite3")
            store.add(mode="raw", provider="test", transcript="x", final_text="x", insertion_status="failed", error="boom")
            formatted = format_entries(store.recent())
        self.assertIn("failed", formatted)
        self.assertIn("correction=skipped", formatted)
        self.assertIn("boom", formatted)

    def test_latency_fields_round_trip_and_format_without_text(self):
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as tmpdir:
            store = HistoryStore(Path(tmpdir) / "history.sqlite3")
            store.add(
                mode="clean",
                provider="groq",
                transcript="private raw words",
                final_text="Private final words.",
                insertion_status="pasted",
                audio_duration_ms=6500,
                latency_ms=380,
                recorder_stop_latency_ms=22,
                stt_latency_ms=250,
                transform_latency_ms=4,
                correction_latency_ms=0,
                clipboard_latency_ms=12,
                paste_latency_ms=83,
                overhead_latency_ms=9,
            )
            rows = store.recent()
            formatted = format_latency_metrics(rows)

        self.assertEqual(rows[0].latency_ms, 380)
        self.assertEqual(rows[0].stt_latency_ms, 250)
        self.assertEqual(rows[0].clipboard_latency_ms, 12)
        self.assertIn("avg_total=380ms", formatted)
        self.assertIn("stt=250ms", formatted)
        self.assertNotIn("private raw words", formatted)
        self.assertNotIn("Private final words.", formatted)

    def test_latency_format_includes_failed_stage(self):
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as tmpdir:
            store = HistoryStore(Path(tmpdir) / "history.sqlite3")
            store.add(
                mode="clean",
                provider="faster-whisper",
                transcript=None,
                final_text=None,
                insertion_status="failed",
                latency_ms=120,
                stt_latency_ms=120,
                failure_stage="stt",
                error="boom",
            )
            formatted = format_latency_metrics(store.recent())

        self.assertIn("failed_stage=stt", formatted)
        self.assertIn("total=120ms", formatted)


if __name__ == "__main__":
    unittest.main()
