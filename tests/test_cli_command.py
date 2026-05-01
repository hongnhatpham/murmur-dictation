from __future__ import annotations

import tempfile
import unittest
from argparse import Namespace
from pathlib import Path
from unittest.mock import patch

from murmur.cli import cmd_command
from murmur.config import load_config


class CliCommandTests(unittest.TestCase):
    def test_command_rejects_oversized_selection(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            config_path = Path(tmpdir_s) / "config.toml"
            config_path.write_text("[command]\nmax_selection_chars = 4\n", encoding="utf-8")
            args = Namespace(
                selection=False,
                clipboard=True,
                duration=3.0,
                audio=None,
                instruction=["uppercase"],
                paste=False,
                keep_audio=False,
            )
            with patch("murmur.cli.load_config", return_value=load_config(config_path)), patch(
                "murmur.cli.read_clipboard", return_value="12345"
            ):
                rc = cmd_command(args)

        self.assertEqual(rc, 1)


if __name__ == "__main__":
    unittest.main()
