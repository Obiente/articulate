"""Synthetic tests only; never starts the real model or reads user recordings."""

import json
from pathlib import Path
import subprocess
import tempfile
import unittest

import evaluate


class EvaluationTests(unittest.TestCase):
    def test_edit_counts_and_normalization(self):
        self.assertEqual(evaluate.score("One, café! Don't stop.", "one café don't stop")["wer"], 0)
        self.assertGreater(evaluate.score("Hi!", "hi", exact=True)["wer"], 0)
        for reference, hypothesis, expected in [
            ("red blue", "red green", (1, 0, 0)),
            ("red blue", "red", (0, 1, 0)),
            ("red", "red blue", (0, 0, 1)),
        ]:
            scored = evaluate.score(reference, hypothesis)
            self.assertEqual(tuple(scored[k] for k in ("substitutions", "deletions", "insertions")), expected)

    def test_silence_and_limits(self):
        score = evaluate.score("", "hallucinated words")
        self.assertIsNone(score["wer"])
        self.assertEqual(score["insertions"], 2)
        with self.assertRaises(ValueError):
            evaluate.score("word " * (evaluate.MAX_WORDS + 1), "")

    def test_manifest_paths_and_duplicate_ids(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "clip.wav").write_bytes(b"synthetic placeholder, not decoded")
            manifest = root / "manifest.json"
            item = {"id": "example", "wav": "clip.wav", "reference": "Hello"}
            manifest.write_text(json.dumps({"items": [item]}), encoding="utf-8")
            self.assertEqual(evaluate.load_manifest(manifest, 5)[0][0], "example")
            for rows in [[item, item], [dict(item, wav="../outside.wav")], [dict(item, wav=str(root / "clip.wav"))]]:
                manifest.write_text(json.dumps({"items": rows}), encoding="utf-8")
                with self.assertRaises(ValueError):
                    evaluate.load_manifest(manifest, 5)

    def test_fake_inference_and_weighted_aggregate(self):
        calls = []
        responses = ["red green", "cat dog bird"]
        def fake_run(command, **options):
            calls.append((command, options))
            row = {"text": responses[len(calls) - 1], "run": 1, "backend": "synthetic", "audio_ms": 2000, "transcribe_ms": 500, "load_ms": 100}
            return subprocess.CompletedProcess(command, 0, "backend diagnostic\n" + json.dumps(row), "")
        report = evaluate.evaluate([
            ("first", Path("first.wav"), "red blue"),
            ("second", Path("second.wav"), "cat dog bird"),
        ], Path("synthetic.exe"), cpu=True, runner=fake_run)
        self.assertEqual(report["aggregate"]["normalized"]["wer"], 0.2)
        self.assertEqual(report["aggregate"]["inference_rtf"], 0.25)
        self.assertIn("--cpu", calls[0][0])
        self.assertFalse(calls[0][1]["shell"])
        self.assertLessEqual(calls[0][1]["timeout"], 180)
        self.assertNotIn("wav", report["items"][0])

    def test_malformed_response_and_timeout_fail_without_scoring(self):
        for response in ["not json", '{"text":"bad","run":1}', '{"text":"bad","run":1,"audio_ms":0,"transcribe_ms":1,"load_ms":1}']:
            with self.assertRaises(ValueError):
                evaluate.parse_response(response)
        def timed_out(*args, **kwargs):
            raise subprocess.TimeoutExpired("synthetic", 1)
        with self.assertRaisesRegex(ValueError, "inference timed out"):
            evaluate.evaluate([("clip", Path("clip.wav"), "hello")], Path("synthetic.exe"), runner=timed_out)


if __name__ == "__main__":
    unittest.main()
