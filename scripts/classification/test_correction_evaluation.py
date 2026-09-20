import copy
import unittest

from generate import CONTRACTS
from train_corrections import replacement_score


class CorrectionEvaluationTests(unittest.TestCase):
    def setUp(self):
        self.request = {"state": "synthetic", "questions": [CONTRACTS["corrections"]]}
        self.prediction = {"answers": [{"question_id": "correction", "selected_id": "replace",
                                       "candidate_ids": ["keep_original", "replace"],
                                       "distribution": {"probabilities": [0.2, 0.8], "selected": 1}}]}

    def test_accepts_normalized_matching_answer(self):
        self.assertEqual(replacement_score(self.prediction, self.request), 0.8)

    def test_rejects_missing_extra_wrong_task_or_reordered_answer(self):
        malformed = []
        for field, value in (("question_id", "shortcut_intent"),
                             ("candidate_ids", ["replace", "keep_original"]),
                             ("selected_id", "keep_original")):
            prediction = copy.deepcopy(self.prediction)
            prediction["answers"][0][field] = value
            malformed.append(prediction)
        malformed.extend([{"answers": []}, {"answers": self.prediction["answers"] * 2}])
        for prediction in malformed:
            with self.subTest(prediction=prediction), self.assertRaises(ValueError):
                replacement_score(prediction, self.request)

    def test_rejects_invalid_distributions(self):
        for probabilities in ([0.8], [0.2, 0.3, 0.5], [0.2, 0.9], [-0.1, 1.1],
                              [float("nan"), 0.8], [0.2, float("inf")], [False, True]):
            prediction = copy.deepcopy(self.prediction)
            prediction["answers"][0]["distribution"]["probabilities"] = probabilities
            with self.subTest(probabilities=probabilities), self.assertRaises(ValueError):
                replacement_score(prediction, self.request)


if __name__ == "__main__":
    unittest.main()
