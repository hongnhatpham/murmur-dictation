from __future__ import annotations

import shutil
import subprocess
import time

from .errors import MurmurError
from .insertion import ToolSimulator


def read_clipboard(tool: str = "wl-paste") -> str:
    if shutil.which(tool) is None:
        raise MurmurError(f"Missing `{tool}`.", "Install wl-clipboard to read selected text via clipboard fallback.")
    proc = subprocess.run([tool, "--no-newline"], capture_output=True, text=True)
    if proc.returncode != 0:
        raise MurmurError("Clipboard read failed.", proc.stderr.strip() or proc.stdout.strip())
    return proc.stdout


def capture_selection_via_clipboard(paste_tool: str = "wtype") -> str:
    """Capture current selection by simulating Ctrl+C then reading wl-paste."""
    simulator = ToolSimulator(paste_tool)
    if not simulator.available():
        raise MurmurError(f"Missing paste simulator `{paste_tool}`.", "Install wtype/ydotool or use --text/--clipboard mode.")
    if paste_tool == "wtype":
        proc = subprocess.run(["wtype", "-M", "ctrl", "-P", "c", "-p", "c", "-m", "ctrl"], capture_output=True, text=True)
    elif paste_tool == "ydotool":
        proc = subprocess.run(["ydotool", "key", "29:1", "46:1", "46:0", "29:0"], capture_output=True, text=True)
    else:
        raise MurmurError(f"Unsupported paste simulator `{paste_tool}`.")
    if proc.returncode != 0:
        raise MurmurError("Selection copy failed.", proc.stderr.strip() or proc.stdout.strip())
    time.sleep(0.08)
    text = read_clipboard()
    if not text:
        raise MurmurError("No selected text captured.", "Select text first or pass --text.")
    return text
