import unittest

from murmur.history import HistoryStore, format_entries


class HistoryTests(unittest.TestCase):
    def test_append_and_load_recent_history(self):
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as tmpdir:
            store = HistoryStore(Path(tmpdir) / "history.sqlite3")
            store.add(mode="clean", provider="test", transcript="hi", final_text="Hi.", insertion_status="copied-only")
            rows = store.recent()

        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0].mode, "clean")
        self.assertEqual(rows[0].provider, "test")
        self.assertEqual(rows[0].transcript, "hi")
        self.assertEqual(rows[0].final_text, "Hi.")
        self.assertEqual(rows[0].insertion_status, "copied-only")

    def test_format_entries(self):
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as tmpdir:
            store = HistoryStore(Path(tmpdir) / "history.sqlite3")
            store.add(mode="raw", provider="test", transcript="x", final_text="x", insertion_status="failed", error="boom")
            formatted = format_entries(store.recent())
        self.assertIn("failed", formatted)
        self.assertIn("boom", formatted)


if __name__ == "__main__":
    unittest.main()
