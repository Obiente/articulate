import json
from pathlib import Path
import tempfile
import unittest

from correction_corpus import generate, challenge, PAIRS


class CorrectionCorpusTests(unittest.TestCase):
    def test_entities_and_surface_cues_do_not_determine_the_label(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder) / "data"
            generate(root)
            rows = [json.loads(line) for line in (root / "train.jsonl").read_text().splitlines()]
            for heard, _ in PAIRS["train"]:
                labels = {row["targets"][0]["value"] for row in rows
                          if row["request"]["state"].startswith(f"Original: {heard}\n")}
                self.assertEqual(labels, {0, 1})
            for label in (0, 1):
                states = [row["request"]["state"] for row in rows if row["targets"][0]["value"] == label]
                self.assertTrue(any("?" in state for state in states))
                self.assertTrue(any("not " in state for state in states))
                self.assertTrue(any("'" in state for state in states))

    def test_challenge_is_disjoint_and_keeps_positive_and_negative_negations(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder) / "data"
            generate(root)
            train = {json.loads(line)["request"]["state"] for line in (root / "train.jsonl").read_text().splitlines()}
            validation = {json.loads(line)["request"]["state"] for line in (root / "validation.jsonl").read_text().splitlines()}
            frozen = challenge()
            states = {row["request"]["state"] for row in frozen}
            self.assertEqual(len(states), len(frozen))
            self.assertFalse(train & validation or train & states or validation & states)
            self.assertEqual({row["targets"][0]["value"] for row in frozen if "not " in row["request"]["state"]}, {0, 1})


if __name__ == "__main__":
    unittest.main()
