import unittest

from murmur.config import StyleConfig, load_config
from murmur.styles import apply_style, style_for_category


class StyleTests(unittest.TestCase):
    def test_style_can_remove_trailing_period_for_messages(self):
        style = style_for_category("personal_message", StyleConfig())
        self.assertEqual(apply_style("Hello.", style), "Hello")

    def test_email_style_keeps_period(self):
        style = style_for_category("email", StyleConfig())
        self.assertEqual(apply_style("Hello.", style), "Hello.")

    def test_load_config_style_presets(self):
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "config.toml"
            path.write_text('[styles.presets.personal_message]\ntrailing_period = false\nformality = "very casual"\n')
            cfg = load_config(path)

        self.assertFalse(cfg.styles.presets["personal_message"]["trailing_period"])
        self.assertEqual(cfg.styles.presets["personal_message"]["formality"], "very casual")


if __name__ == "__main__":
    unittest.main()
