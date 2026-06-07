from __future__ import annotations

import unittest
from argparse import Namespace
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from unittest.mock import patch

from murmur.cli import cmd_stop_recording
from murmur.config import load_config
from murmur.service import ServiceResponse


class CliServiceTests(unittest.TestCase):
    def test_stop_recording_delegates_to_running_service(self):
        args = Namespace(mode=None, paste=False, no_paste=False, keep_audio=False)
        response = ServiceResponse(3, stdout="delegated output\n", stderr="delegated error\n")
        with patch("murmur.cli.load_config", return_value=load_config("/nonexistent/murmur-defaults.toml")), patch(
            "murmur.cli.ensure_local_dirs"
        ), patch("murmur.cli.request_stop_recording", return_value=response), patch(
            "murmur.cli._cmd_stop_recording_direct", side_effect=AssertionError("must not process twice")
        ), redirect_stdout(StringIO()) as stdout, redirect_stderr(StringIO()) as stderr:
            rc = cmd_stop_recording(args)

        self.assertEqual(rc, 3)
        self.assertEqual(stdout.getvalue(), "delegated output\n")
        self.assertEqual(stderr.getvalue(), "delegated error\n")


if __name__ == "__main__":
    unittest.main()
