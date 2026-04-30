from __future__ import annotations

import unittest
from unittest.mock import patch

from murmur.notify import notify


class NotifyTests(unittest.TestCase):
    @patch("murmur.notify.subprocess.run")
    @patch("murmur.notify.shutil.which", return_value="/usr/bin/notify-send")
    def test_notify_send_uses_short_status_options(self, _which, run):
        notify("Recording")

        cmd = run.call_args.args[0]
        self.assertEqual(cmd[0], "notify-send")
        self.assertIn("--app-name=Murmur", cmd)
        self.assertIn("--expire-time", cmd)
        self.assertIn("1200", cmd)
        self.assertIn("Murmur: recording", cmd)
        self.assertFalse(run.call_args.kwargs["check"])

    @patch("murmur.notify.subprocess.run")
    @patch("murmur.notify.shutil.which", return_value=None)
    def test_notify_is_noop_without_notify_send(self, _which, run):
        notify("Failed", "boom")

        run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
