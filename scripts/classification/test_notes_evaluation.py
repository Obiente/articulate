import unittest

from compare_notes import summarize


class NotesEvaluationTests(unittest.TestCase):
    def report(self):
        return {"category_order": ["decision", "action", "key_fact", "background"],
                "segments": 8, "confusion": [[2, 0, 0, 0], [1, 1, 0, 0],
                                               [0, 0, 2, 0], [0, 1, 0, 1]]}

    def test_confusion_rows_are_actual_and_columns_predicted(self):
        result = summarize(self.report())
        self.assertEqual(result["per_category"]["decision"]["recall"], 1)
        self.assertAlmostEqual(result["per_category"]["decision"]["precision"], 2 / 3)
        self.assertEqual(result["background_false_positive_rate"], 0.5)

    def test_rejects_inconsistent_counts_and_duplicate_categories(self):
        report = self.report()
        report["segments"] += 1
        with self.assertRaises(ValueError):
            summarize(report)
        report = self.report()
        report["category_order"][0] = "background"
        with self.assertRaises(ValueError):
            summarize(report)

    def test_missing_category_observations_are_unmeasured_not_perfect(self):
        report = self.report()
        report["confusion"] = [[0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 8]]
        result = summarize(report)
        self.assertIsNone(result["per_category"]["decision"]["recall"])
        self.assertIsNone(result["per_category"]["decision"]["precision"])


if __name__ == "__main__":
    unittest.main()
