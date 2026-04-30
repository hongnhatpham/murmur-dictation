import tempfile
import unittest
from pathlib import Path

from murmur.personal import PersonalStore, apply_dictionary_terms, apply_snippets


class PersonalTests(unittest.TestCase):
    def test_dictionary_crud(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            store = PersonalStore(Path(tmpdir) / "personal.sqlite3")
            term_id = store.add_term("ARIA-03", note="assistant name")
            self.assertEqual(store.list_terms()[0].term, "ARIA-03")
            self.assertEqual(store.list_terms()[0].note, "assistant name")
            self.assertTrue(store.remove_term(str(term_id)))
            self.assertEqual(store.list_terms(), [])

    def test_snippet_crud_and_expand(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            store = PersonalStore(Path(tmpdir) / "personal.sqlite3")
            store.add_snippet(";sig", "Regards, Murmur")
            self.assertEqual(store.snippet_map(), {";sig": "Regards, Murmur"})
            self.assertEqual(apply_snippets(";sig", store.snippet_map()), "Regards, Murmur")
            self.assertTrue(store.remove_snippet(";sig"))
            self.assertEqual(store.list_snippets(), [])

    def test_dictionary_terms_restore_preferred_casing(self):
        self.assertEqual(apply_dictionary_terms("I use niri every day.", ["Niri"]), "I use Niri every day.")


if __name__ == "__main__":
    unittest.main()
