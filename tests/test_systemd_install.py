from __future__ import annotations

import os
import subprocess
import tempfile
import unittest
from pathlib import Path


class SystemdInstallTests(unittest.TestCase):
    def test_installer_escapes_execstart_path_spaces(self):
        with tempfile.TemporaryDirectory() as tmpdir_s:
            tmpdir = Path(tmpdir_s)
            config_home = tmpdir / "config"
            murmur_bin = tmpdir / "path with spaces" / "murmur"
            env = {
                "HOME": str(tmpdir / "home"),
                "XDG_CONFIG_HOME": str(config_home),
                "PATH": os.environ.get("PATH", ""),
            }

            subprocess.run(
                ["packaging/systemd/install-user-service.sh", str(murmur_bin)],
                cwd=Path(__file__).resolve().parents[1],
                env=env,
                check=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )

            service_text = (config_home / "systemd/user/murmur.service").read_text(encoding="utf-8")
            dictate_text = (config_home / "systemd/user/murmur-dictate.service").read_text(encoding="utf-8")

        self.assertIn(f'ExecStart="{murmur_bin}" service', service_text)
        self.assertIn(f'ExecStart="{murmur_bin}" dictate --paste', dictate_text)


if __name__ == "__main__":
    unittest.main()
