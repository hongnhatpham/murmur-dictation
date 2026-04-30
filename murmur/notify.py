from __future__ import annotations

import shutil
import subprocess


def notify(summary: str, body: str = "") -> None:
    if shutil.which("notify-send") is None:
        return
    cmd = ["notify-send", "Murmur", summary]
    if body:
        cmd.append(body)
    subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
