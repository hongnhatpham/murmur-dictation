from __future__ import annotations

import shutil
import subprocess

_STATUS_OPTIONS = {
    "recording": ("Murmur: recording", "normal", "1200"),
    "processing": ("Murmur: processing", "low", "1200"),
    "inserted": ("Murmur: inserted", "low", "900"),
    "copied": ("Murmur: copied", "normal", "1600"),
    "copied-only": ("Murmur: copied", "normal", "1600"),
    "fallback": ("Murmur: copied", "normal", "1600"),
    "canceled": ("Murmur: canceled", "low", "900"),
    "failed": ("Murmur: failed", "normal", "3500"),
}


def notify(summary: str, body: str = "") -> None:
    """Send a terse focus-safe desktop notification when libnotify is available."""
    if shutil.which("notify-send") is None:
        return

    title, urgency, timeout_ms = _STATUS_OPTIONS.get(summary.casefold(), (f"Murmur: {summary.casefold()}", "low", "1500"))
    cmd = [
        "notify-send",
        "--app-name=Murmur",
        "--expire-time",
        timeout_ms,
        "--urgency",
        urgency,
        title,
    ]
    if body:
        cmd.append(body)
    try:
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)
    except OSError:
        return
