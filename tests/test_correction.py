import json
import unittest
from unittest.mock import patch

from murmur.config import CorrectionConfig
from murmur.context import AppContext
from murmur.correction import _parse_model_response, correct_text


class CorrectionTests(unittest.TestCase):
    def test_disabled_correction_is_skipped(self):
        result = correct_text(
            raw_transcript="um hello",
            deterministic_text="Hello.",
            mode="clean",
            config=CorrectionConfig(enabled=False),
            app_context=AppContext(category="other"),
        )

        self.assertEqual(result.final_text, "Hello.")
        self.assertEqual(result.provider, "deterministic")
        self.assertEqual(result.status, "skipped")

    def test_raw_mode_bypasses_correction_by_default(self):
        result = correct_text(
            raw_transcript="hello",
            deterministic_text="hello",
            mode="raw",
            config=CorrectionConfig(enabled=True),
            app_context=AppContext(category="other"),
        )

        self.assertEqual(result.final_text, "hello")
        self.assertEqual(result.status, "skipped")

    def test_ollama_failure_falls_back_to_deterministic_text(self):
        with patch("murmur.correction.urllib.request.urlopen", side_effect=TimeoutError("slow")):
            result = correct_text(
                raw_transcript="hello",
                deterministic_text="Hello.",
                mode="clean",
                config=CorrectionConfig(enabled=True, timeout_seconds=0.01),
                app_context=AppContext(category="email"),
            )

        self.assertEqual(result.final_text, "Hello.")
        self.assertEqual(result.status, "fallback")

    def test_model_json_response_is_parsed(self):
        self.assertEqual(_parse_model_response(json.dumps({"text": "Hello there."})), "Hello there.")
        self.assertEqual(_parse_model_response("Hello there."), "")
        self.assertEqual(_parse_model_response('<think>nope</think>{"text":"Clean."}'), "Clean.")
        self.assertEqual(_parse_model_response('prefix {"text":"Clean."} suffix'), "Clean.")


if __name__ == "__main__":
    unittest.main()
