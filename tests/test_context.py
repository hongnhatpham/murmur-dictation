import unittest

from murmur.config import ContextConfig, load_config
from murmur.context import categorize_app


class ContextTests(unittest.TestCase):
    def test_config_maps_app_categories(self):
        cfg = ContextConfig(app_categories={"Alacritty": "terminal", "Slack": "work_message"})

        self.assertEqual(categorize_app("Alacritty", cfg), "terminal")
        self.assertEqual(categorize_app("Slack", cfg), "work_message")
        self.assertEqual(categorize_app("unknown-app", cfg), "other")

    def test_heuristics_classify_common_apps(self):
        cfg = ContextConfig(app_categories={})

        self.assertEqual(categorize_app("com.mitchellh.ghostty", cfg), "terminal")
        self.assertEqual(categorize_app("codium", cfg), "code")
        self.assertEqual(categorize_app("discord", cfg), "personal_message")
        self.assertEqual(categorize_app(None, cfg), "other")

    def test_load_config_context_table(self):
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "config.toml"
            path.write_text('[context]\napp_categories = {"Foo" = "email"}\n')
            cfg = load_config(path)

        self.assertEqual(cfg.context.app_categories, {"Foo": "email"})


if __name__ == "__main__":
    unittest.main()
