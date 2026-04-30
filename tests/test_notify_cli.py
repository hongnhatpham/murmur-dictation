from __future__ import annotations

import tempfile
import unittest
from argparse import Namespace
from pathlib import Path
from unittest.mock import patch

from murmur import notify
from murmur.cli import _cmd_insert_text
from murmur.history import HistoryStore


class NotifyTests(unittest.TestCase):
    def test_notify_is_best_effort_and_uses_app_name(self):
        calls = []

        def fake_run(cmd, **kwargs):
            calls.append((cmd, kwargs))
            raise OSError("ignored failure")

        with patch("murmur.notify.shutil.which", return_value="/usr/bin/notify-send"), patch(
            "murmur.notify.subprocess.run", side_effect=fake_run
        ):
            notify.notify("Copied", "manual paste available")

        self.assertEqual(calls[0][0][:4], ["notify-send", "--app-name=Murmur", "--expire-time", "1600"])
        self.assertIn("Murmur: copied", calls[0][0])


class CliInsertTests(unittest.TestCase):
    def test_copy_command_writes_history_with_configured_path(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            tmpdir = Path(tmpdir_s)
            config_path = tmpdir / "config.toml"
            history_path = tmpdir / "history.sqlite3"
            config_path.write_text(
                f'''
[paths]
history_db = "{history_path}"

[insertion]
clipboard_tool = "wl-copy"
''',
                encoding="utf-8",
            )
            args = Namespace(text=["hello", "world"], mode="raw", private=False)
            with patch("murmur.cli.load_config", return_value=__import__("murmur.config").config.load_config(config_path)), patch(
                "murmur.insertion.WlClipboard.copy", return_value=None
            ):
                rc = _cmd_insert_text(args, paste=False)
            self.assertEqual(rc, 0)
            self.assertEqual(HistoryStore(history_path).last().final_text, "hello world")


if __name__ == "__main__":
    unittest.main()
