from __future__ import annotations

import shutil
import subprocess
import time
from dataclasses import dataclass
from typing import Protocol

from .config import InsertionConfig
from .transform import PRESS_ENTER_AFTER_INSERT, TransformAction


class Clipboard(Protocol):
    def copy(self, text: str) -> None: ...


class Simulator(Protocol):
    name: str
    def available(self) -> bool: ...
    def paste(self) -> None: ...
    def press_enter(self) -> None: ...
    def copy_selection(self) -> None: ...


@dataclass(frozen=True)
class InsertionResult:
    status: str
    message: str = ""
    paste_attempted: bool = False
    enter_sent: bool = False


class InsertionError(RuntimeError):
    pass


class WlClipboard:
    def __init__(self, tool: str = "wl-copy") -> None:
        self.tool = tool

    def copy(self, text: str) -> None:
        if shutil.which(self.tool) is None:
            raise InsertionError(f"Missing `{self.tool}`. Install wl-clipboard.")
        proc = subprocess.run([self.tool], input=text, text=True, capture_output=True)
        if proc.returncode != 0:
            raise InsertionError(proc.stderr.strip() or f"{self.tool} failed")


class ToolSimulator:
    def __init__(self, tool: str = "wtype") -> None:
        self.name = tool

    def available(self) -> bool:
        return shutil.which(self.name) is not None

    def paste(self) -> None:
        if self.name == "wtype":
            proc = subprocess.run(["wtype", "-M", "ctrl", "-P", "v", "-p", "v", "-m", "ctrl"], capture_output=True, text=True)
        elif self.name == "ydotool":
            proc = subprocess.run(["ydotool", "key", "29:1", "47:1", "47:0", "29:0"], capture_output=True, text=True)
        else:
            raise InsertionError(f"Unsupported paste simulator: {self.name}")
        if proc.returncode != 0:
            raise InsertionError(proc.stderr.strip() or f"{self.name} paste failed")

    def press_enter(self) -> None:
        if self.name == "wtype":
            proc = subprocess.run(["wtype", "-P", "Return", "-p", "Return"], capture_output=True, text=True)
        elif self.name == "ydotool":
            proc = subprocess.run(["ydotool", "key", "28:1", "28:0"], capture_output=True, text=True)
        else:
            raise InsertionError(f"Unsupported paste simulator: {self.name}")
        if proc.returncode != 0:
            raise InsertionError(proc.stderr.strip() or f"{self.name} enter failed")

    def copy_selection(self) -> None:
        if self.name == "wtype":
            proc = subprocess.run(["wtype", "-M", "ctrl", "-P", "c", "-p", "c", "-m", "ctrl"], capture_output=True, text=True)
        elif self.name == "ydotool":
            proc = subprocess.run(["ydotool", "key", "29:1", "46:1", "46:0", "29:0"], capture_output=True, text=True)
        else:
            raise InsertionError(f"Unsupported selection copy simulator: {self.name}")
        if proc.returncode != 0:
            raise InsertionError(proc.stderr.strip() or f"{self.name} selection copy failed")


def copy_to_clipboard(text: str, tool: str = "wl-copy") -> None:
    WlClipboard(tool).copy(text)


def read_clipboard(tool: str = "wl-paste") -> str:
    if shutil.which(tool) is None:
        raise InsertionError(f"Missing `{tool}`. Install wl-clipboard.")
    proc = subprocess.run([tool, "--no-newline"], text=True, capture_output=True)
    if proc.returncode != 0:
        raise InsertionError(proc.stderr.strip() or f"{tool} failed")
    return proc.stdout


def copy_selection_to_clipboard(config: InsertionConfig) -> bool:
    simulator = _first_available_simulator(config)
    if simulator is None:
        return False
    try:
        simulator.copy_selection()
        time.sleep(0.08)
        return True
    except InsertionError:
        return False


def _has_enter_action(actions: list[object] | None) -> bool:
    for action in actions or []:
        if isinstance(action, TransformAction) and action.name == PRESS_ENTER_AFTER_INSERT:
            return True
        if action == PRESS_ENTER_AFTER_INSERT:
            return True
    return False


def _candidate_simulators(config: InsertionConfig) -> list[ToolSimulator]:
    seen: set[str] = set()
    candidates: list[ToolSimulator] = []
    for tool in (config.paste_simulator, config.fallback_paste_simulator, getattr(config, "paste_tool", "")):
        if tool and tool not in seen:
            seen.add(tool)
            candidates.append(ToolSimulator(tool))
    return candidates


def _first_available_simulator(config: InsertionConfig) -> ToolSimulator | None:
    for simulator in _candidate_simulators(config):
        if simulator.available():
            return simulator
    return None


def paste_from_clipboard(config: InsertionConfig) -> bool:
    simulator = _first_available_simulator(config)
    if simulator is None:
        return False
    try:
        simulator.paste()
        return True
    except InsertionError:
        return False


def press_enter(config: InsertionConfig) -> bool:
    simulator = _first_available_simulator(config)
    if simulator is None:
        return False
    try:
        simulator.press_enter()
        return True
    except InsertionError:
        return False


def insert_text(
    text: str,
    actions: list[object] | None = None,
    config: InsertionConfig | None = None,
    paste: bool = True,
    *,
    clipboard: Clipboard | None = None,
    simulator: Simulator | None = None,
) -> InsertionResult:
    """Copy text and optionally paste it, with injectable test doubles."""
    config = config or InsertionConfig()
    clipboard = clipboard or WlClipboard(config.clipboard_tool)
    simulator = simulator or _first_available_simulator(config)

    if text:
        clipboard.copy(text)
    if not paste:
        return InsertionResult(status="copied-only", message="Copied to clipboard")

    if simulator is None or not simulator.available():
        return InsertionResult(
            status="copied-only",
            message="Copied to clipboard; install wtype or ydotool to paste into the focused app.",
            paste_attempted=False,
        )

    paste_attempted = False
    enter_sent = False
    try:
        if text:
            paste_attempted = True
            simulator.paste()
            time.sleep(0.08)
        if _has_enter_action(actions):
            simulator.press_enter()
            enter_sent = True
    except InsertionError as exc:
        return InsertionResult(status="copied-only", message=f"Copied to clipboard; paste failed: {exc}", paste_attempted=paste_attempted)

    return InsertionResult(status="pasted", message="Pasted", paste_attempted=paste_attempted, enter_sent=enter_sent)
