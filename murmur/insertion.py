from __future__ import annotations

import json
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
        try:
            proc = subprocess.run(
                [self.tool],
                input=text,
                text=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=3,
            )
        except subprocess.TimeoutExpired as exc:
            raise InsertionError(f"{self.tool} timed out") from exc
        if proc.returncode != 0:
            raise InsertionError(f"{self.tool} failed")


class ToolSimulator:
    def __init__(self, tool: str = "wtype") -> None:
        self.name = tool

    def available(self) -> bool:
        return shutil.which(self.name) is not None

    def paste(self, *, terminal: bool = False) -> None:
        if self.name == "wtype":
            if terminal:
                proc = subprocess.run(["wtype", "-M", "ctrl", "-M", "shift", "-k", "v", "-m", "shift", "-m", "ctrl"], capture_output=True, text=True)
            else:
                proc = subprocess.run(["wtype", "-M", "ctrl", "-k", "v", "-m", "ctrl"], capture_output=True, text=True)
        elif self.name == "ydotool":
            if terminal:
                proc = subprocess.run(["ydotool", "key", "29:1", "42:1", "47:1", "47:0", "42:0", "29:0"], capture_output=True, text=True)
            else:
                proc = subprocess.run(["ydotool", "key", "29:1", "47:1", "47:0", "29:0"], capture_output=True, text=True)
        else:
            raise InsertionError(f"Unsupported paste simulator: {self.name}")
        if proc.returncode != 0:
            raise InsertionError(proc.stderr.strip() or f"{self.name} paste failed")

    def release_modifiers(self) -> None:
        if self.name == "wtype":
            # Defensive cleanup for virtual-keyboard state. Some terminal/compositor
            # combinations can behave as if a modifier is still held after synthetic
            # paste unless we explicitly release common modifiers.
            subprocess.run(["wtype", "-m", "ctrl", "-m", "shift", "-m", "alt", "-m", "logo", "-m", "win"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        elif self.name == "ydotool":
            subprocess.run(["ydotool", "key", "29:0", "42:0", "56:0", "125:0", "126:0"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

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
    try:
        proc = subprocess.run(
            [tool, "--no-newline", "--type", "text/plain"],
            capture_output=True,
            timeout=1,
        )
    except subprocess.TimeoutExpired as exc:
        raise InsertionError(f"{tool} timed out") from exc
    if proc.returncode != 0:
        stderr = proc.stderr.decode("utf-8", errors="replace").strip()
        raise InsertionError(stderr or f"{tool} failed")
    return proc.stdout.decode("utf-8", errors="replace")


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


def _first_available_simulator(config: InsertionConfig, *, prefer_uinput: bool = False) -> ToolSimulator | None:
    candidates = _candidate_simulators(config)
    if prefer_uinput:
        # Prefer a real uinput event source for terminal insertion. On niri,
        # wtype's Wayland virtual-keyboard path can leave some terminal windows
        # in an input-stalled state after synthetic text/paste. ydotool goes
        # through the kernel input stack instead, so the focused app sees a
        # normal key sequence and does not need a compositor focus nudge.
        candidates = sorted(candidates, key=lambda simulator: simulator.name != "ydotool")
    for simulator in candidates:
        if simulator.available():
            return simulator
    return None


def focused_window() -> dict | None:
    if shutil.which("niri") is None:
        return None
    try:
        proc = subprocess.run(["niri", "msg", "-j", "focused-window"], capture_output=True, text=True, timeout=1)
    except Exception:
        return None
    if proc.returncode != 0 or not proc.stdout.strip():
        return None
    try:
        data = json.loads(proc.stdout)
    except json.JSONDecodeError:
        return None
    return data if isinstance(data, dict) else None


def focused_app_id(window: dict | None = None) -> str | None:
    data = window if window is not None else focused_window()
    if not data:
        return None
    app_id = data.get("app_id")
    return str(app_id) if app_id else None


def is_terminal_focused(config: InsertionConfig, window: dict | None = None) -> bool:
    app_id = focused_app_id(window)
    if not app_id:
        return False
    return app_id in set(config.terminal_app_ids)


def format_for_previous_text(text: str, previous_text: str, *, auto_leading_space: bool = True, category: str = "other") -> str:
    if not text or not previous_text:
        return text
    no_space_before = set(",.;:!?)]}%\"'”’")
    last = previous_text[-1]
    result = text
    if auto_leading_space and not text[0].isspace() and text[0] not in no_space_before and not last.isspace() and last not in "([{/$#@\n\t":
        result = " " + result
    if category not in {"terminal", "code"} and last not in ".!?…\n":
        prefix_len = len(result) - len(result.lstrip())
        if prefix_len < len(result) and result[prefix_len].isupper():
            result = result[:prefix_len] + result[prefix_len].lower() + result[prefix_len + 1 :]
    return result


def should_prepend_space(text: str, config: InsertionConfig) -> bool:
    if not text or not getattr(config, "auto_leading_space", True):
        return False
    no_space_before = set(",.;:!?)]}%\"'”’")
    if text[0].isspace() or text[0] in no_space_before:
        return False
    try:
        existing = read_clipboard()
    except InsertionError:
        return False
    if not existing:
        return False
    return format_for_previous_text(text, existing, auto_leading_space=True) != text


def paste_from_clipboard(config: InsertionConfig) -> bool:
    window = focused_window()
    terminal = is_terminal_focused(config, window)
    simulator = _first_available_simulator(config, prefer_uinput=terminal)
    if simulator is None:
        return False
    try:
        simulator.paste(terminal=terminal)
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
    injected_clipboard = clipboard is not None
    clipboard = clipboard or WlClipboard(config.clipboard_tool)
    window = focused_window()
    terminal = is_terminal_focused(config, window)
    simulator = simulator or _first_available_simulator(config, prefer_uinput=terminal)

    if not injected_clipboard:
        try:
            previous_text = read_clipboard()
        except Exception:
            previous_text = ""
        text = format_for_previous_text(text, previous_text, auto_leading_space=getattr(config, "auto_leading_space", True))

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
        try:
            if text:
                paste_attempted = True
                if isinstance(simulator, ToolSimulator):
                    simulator.paste(terminal=terminal)
                else:
                    simulator.paste()
                time.sleep(0.08)
            if _has_enter_action(actions):
                simulator.press_enter()
                enter_sent = True
        finally:
            if isinstance(simulator, ToolSimulator):
                simulator.release_modifiers()
    except InsertionError as exc:
        return InsertionResult(status="copied-only", message=f"Copied to clipboard; paste failed: {exc}", paste_attempted=paste_attempted)

    return InsertionResult(status="pasted", message="Pasted", paste_attempted=paste_attempted, enter_sent=enter_sent)
