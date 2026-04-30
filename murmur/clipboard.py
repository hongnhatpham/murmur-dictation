from __future__ import annotations

import subprocess


def copy_text(text: str, tool: str = "wl-copy") -> None:
    subprocess.run([tool], input=text, text=True, check=True)
