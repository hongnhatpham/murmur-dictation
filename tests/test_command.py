import unittest

from murmur.command import route_command_transform


class CommandTests(unittest.TestCase):
    def test_route_uppercase_command(self):
        result = route_command_transform("make this uppercase", "Hello world")
        self.assertTrue(result.supported)
        self.assertEqual(result.command, "uppercase")
        self.assertEqual(result.final_text, "HELLO WORLD")

    def test_route_lowercase_command(self):
        result = route_command_transform("lower case please", "Hello WORLD")
        self.assertTrue(result.supported)
        self.assertEqual(result.command, "lowercase")
        self.assertEqual(result.final_text, "hello world")

    def test_route_concise_command_is_deterministic_and_local(self):
        result = route_command_transform("make this more concise", "This is really very useful in order to ship.")
        self.assertTrue(result.supported)
        self.assertEqual(result.command, "concise")
        self.assertEqual(result.final_text, "This is useful ship.")

    def test_unsupported_command_preserves_selection(self):
        result = route_command_transform("translate to french", "Keep me")
        self.assertFalse(result.supported)
        self.assertEqual(result.final_text, "Keep me")
        self.assertIn("Unsupported command", result.message)


if __name__ == "__main__":
    unittest.main()
