import json
from pathlib import Path
import tempfile
import unittest

from notes_corpus import generate, TRAIN, VALIDATION, TEST


class NotesCorpusTests(unittest.TestCase):
    def test_split_families_and_rendered_passages_are_disjoint(self):
        families = [{template for values in split.values() for template in values}
                    for split in (TRAIN, VALIDATION, TEST)]
        self.assertFalse(families[0] & families[1] or families[0] & families[2] or families[1] & families[2])
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "data"
            generate(path)
            seen = set()
            for split in ("train", "validation", "test"):
                data = json.loads((path / f"{split}.json").read_text(encoding="utf-8"))
                texts = {segment["text"].lower() for item in data for segment in item["transcript"]["segments"]}
                self.assertFalse(texts & seen)
                seen.update(texts)
                for item in data:
                    self.assertEqual({s["id"] for s in item["transcript"]["segments"]},
                                     {label["segment_id"] for label in item["labels"]})
                self.assertTrue(any(len({label["kind"] for label in item["labels"]}) < 4 for item in data))

    def test_negation_is_not_a_background_label_shortcut(self):
        for split in (TRAIN, VALIDATION, TEST):
            self.assertTrue(any("not " in text for text in split["decision"]))
            self.assertTrue(any("not " in text or "cannot " in text for text in split["key_fact"]))
            self.assertTrue(any("commit" in text for text in split["background"]))


if __name__ == "__main__":
    unittest.main()
