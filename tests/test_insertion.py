from __future__ import annotations

import unittest
from unittest.mock import patch

from murmur.config import InsertionConfig
from murmur.insertion import _first_available_simulator, format_for_previous_text, insert_text
from murmur.transform import TransformAction, PRESS_ENTER_AFTER_INSERT


class FakeClipboard:
    def __init__(self):
        self.text = None

    def copy(self, text: str) -> None:
        self.text = text


class MissingSimulator:
    name = "missing"

    def available(self) -> bool:
        return False

    def paste(self) -> None:
        raise AssertionError("paste should not be called")

    def press_enter(self) -> None:
        raise AssertionError("enter should not be called")


class FakeSimulator:
    name = "fake"

    def __init__(self):
        self.calls = []

    def available(self) -> bool:
        return True

    def paste(self) -> None:
        self.calls.append("paste")

    def press_enter(self) -> None:
        self.calls.append("enter")


class InsertionTests(unittest.TestCase):
    def test_insert_degrades_to_clipboard_only_when_paste_simulator_missing(self):
        clipboard = FakeClipboard()
        result = insert_text("hello", clipboard=clipboard, simulator=MissingSimulator())
        self.assertEqual(clipboard.text, "hello")
        self.assertEqual(result.status, "copied-only")
        self.assertFalse(result.paste_attempted)
        self.assertFalse(result.enter_sent)

    def test_press_enter_is_sent_only_after_paste_attempt(self):
        clipboard = FakeClipboard()
        simulator = FakeSimulator()
        result = insert_text(
            "hello",
            actions=[TransformAction(PRESS_ENTER_AFTER_INSERT)],
            clipboard=clipboard,
            simulator=simulator,
        )
        self.assertEqual(result.status, "pasted")
        self.assertEqual(simulator.calls, ["paste", "enter"])
        self.assertTrue(result.paste_attempted)
        self.assertTrue(result.enter_sent)

    def test_no_enter_without_explicit_action(self):
        simulator = FakeSimulator()
        result = insert_text("hello", clipboard=FakeClipboard(), simulator=simulator)
        self.assertEqual(result.status, "pasted")
        self.assertEqual(simulator.calls, ["paste"])
        self.assertFalse(result.enter_sent)

    def test_continuation_adds_space_and_lowercases_mid_sentence(self):
        self.assertEqual(format_for_previous_text("Next thing.", "sentence"), " next thing.")

    def test_continuation_does_not_add_space_before_punctuation(self):
        self.assertEqual(format_for_previous_text(", next", "sentence"), ", next")

    def test_continuation_does_not_lowercase_terminal_code(self):
        self.assertEqual(format_for_previous_text("Next", "cmd", category="terminal"), " Next")

    def test_terminal_prefer_uinput_picks_ydotool_over_wtype(self):
        config = InsertionConfig(paste_simulator="wtype", fallback_paste_simulator="ydotool", paste_tool="wtype")
        with patch("shutil.which", return_value="/usr/bin/tool"):
            simulator = _first_available_simulator(config, prefer_uinput=True)
        self.assertIsNotNone(simulator)
        self.assertEqual(simulator.name, "ydotool")

    def test_normal_insertion_keeps_configured_simulator_order(self):
        config = InsertionConfig(paste_simulator="wtype", fallback_paste_simulator="ydotool", paste_tool="wtype")
        with patch("shutil.which", return_value="/usr/bin/tool"):
            simulator = _first_available_simulator(config)
        self.assertIsNotNone(simulator)
        self.assertEqual(simulator.name, "wtype")


if __name__ == "__main__":
    unittest.main()
