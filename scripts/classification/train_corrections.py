"""Train and compare a review-only context classifier; retain reports locally."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import shutil
import subprocess
import time

from correction_corpus import generate, PAIRS
from generate import CONTRACTS


def replacement_score(prediction, request):
    questions = request.get("questions")
    if not isinstance(questions, list) or len(questions) != 1 or questions[0] != CONTRACTS["corrections"]:
        raise ValueError("Evaluation input does not match the correction task")
    answers = prediction.get("answers") if isinstance(prediction, dict) else None
    if not isinstance(answers, list) or len(answers) != 1 or not isinstance(answers[0], dict):
        raise ValueError("Correction inference must return exactly one answer")
    answer = answers[0]
    ids = [candidate["id"] for candidate in questions[0]["candidates"]]
    if answer.get("question_id") != "correction" or answer.get("candidate_ids") != ids:
        raise ValueError("Inference returned the wrong task or candidate order")
    distribution = answer.get("distribution")
    probabilities = distribution.get("probabilities") if isinstance(distribution, dict) else None
    if (not isinstance(probabilities, list) or len(probabilities) != 2
            or any(type(value) not in (int, float) or not math.isfinite(value) or not 0 <= value <= 1
                   for value in probabilities)
            or not math.isclose(sum(probabilities), 1.0, rel_tol=0.0, abs_tol=1e-5)):
        raise ValueError("Invalid correction probability distribution")
    selected = distribution.get("selected")
    if (type(selected) is not int or selected not in (0, 1)
            or answer.get("selected_id") != ids[selected]
            or probabilities[selected] < max(probabilities)):
        raise ValueError("Inconsistent correction selection")
    return probabilities[1]


def evaluate(executable, model, examples_file, report_file):
    examples = [json.loads(line) for line in examples_file.read_text(encoding="utf-8").splitlines()]
    predictions = []
    start = time.monotonic()
    for offset in range(0, len(examples), 8):
        batch = examples[offset:offset + 8]
        result = subprocess.run([str(executable), "infer", "--checkpoint", str(model / "checkpoint"),
                                 "--tokenizer", str(model / "tokenizer.json"), "--input", "-"],
                                input=json.dumps([row["request"] for row in batch]),
                                capture_output=True, text=True, encoding="utf-8", check=True, timeout=60)
        responses = json.loads(result.stdout)
        if not isinstance(responses, list) or len(responses) != len(batch):
            raise ValueError("Inference response count does not match its request batch")
        predictions.extend(responses)
    if len(predictions) != len(examples):
        raise ValueError("Inference omitted examples; evaluation cannot continue")
    confusion = [[0, 0], [0, 0]]
    details = []
    for example, prediction in zip(examples, predictions):
        score = replacement_score(prediction, example["request"])
        expected = example["targets"][0]["value"]
        confusion[expected][int(score >= 0.5)] += 1
        details.append({"expected_replace": bool(expected), "replace_score": score,
                        "state": example["request"]["state"]})
    report = {"examples": len(examples), "confusion_keep_replace": confusion,
              "accuracy": (confusion[0][0] + confusion[1][1]) / len(examples),
              "unsafe_preferences": confusion[0][1],
              "high_score_false_replacements": sum(not row["expected_replace"] and row["replace_score"] >= 0.90 for row in details),
              "positive_recall": confusion[1][1] / sum(confusion[1]) if sum(confusion[1]) else None,
              "positive_recall_at_0_9": sum(row["expected_replace"] and row["replace_score"] >= 0.90 for row in details) / sum(confusion[1]) if sum(confusion[1]) else None,
              "seconds": time.monotonic() - start,
              "scope": "Raw model scores on authored synthetic cases. No UI or deterministic guards applied.",
              "input_sha256": hashlib.sha256(examples_file.read_bytes()).hexdigest()}
    report_file.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    report_file.with_name(report_file.stem + "-details.json").write_text(json.dumps(details, indent=2) + "\n", encoding="utf-8")
    return report


def write_contract(model):
    contract = {"version": 1, "task": "corrections", "question": CONTRACTS["corrections"]}
    (model / "checkpoint/articulate-task.json").write_text(json.dumps(contract, indent=2) + "\n", encoding="utf-8")
    limits = {"max_batch_size": 1, "max_questions": 1, "max_candidates": 2,
              "max_state_tokens": 512, "max_question_tokens": 128,
              "max_candidate_tokens": 128, "max_padded_tokens": 262144}
    (model / "inference-limits.json").write_text(json.dumps(limits, indent=2) + "\n", encoding="utf-8")


def regularized_inputs(root):
    """Research alternative: smaller model, label smoothing and trained [UNK].

    Synthetic entity tokens are omitted from the training vocabulary so their
    unknown-token embedding sees examples. Runtime prompts and scores remain
    unchanged, and no quote/question filter is applied during evaluation.
    """
    data = root / "data"
    fitted = json.loads((data / "tokenizer.json").read_text(encoding="utf-8"))
    omit = {word.lower() for heard, wanted in PAIRS["train"] for word in (heard + " " + wanted).split()}
    vocabulary = fitted["model"]["vocab"]
    fitted["model"]["vocab"] = {word: index for index, word in enumerate(
        word for word, _ in sorted(vocabulary.items(), key=lambda pair: pair[1]) if word not in omit)}
    tokenizer_path = root / "regularized-tokenizer.json"
    tokenizer_path.write_text(json.dumps(fitted, indent=2) + "\n", encoding="utf-8")
    config = json.loads((data / "model-config.json").read_text(encoding="utf-8"))
    config.update(vocab_size=len(fitted["model"]["vocab"]), hidden_size=32,
                  ffn_size=64, state_layers=1, dropout=0.2)
    config_path = root / "regularized-config.json"
    config_path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")
    rows = [json.loads(line) for line in (data / "train.jsonl").read_text(encoding="utf-8").splitlines()]
    for row in rows:
        row["targets"] = [{"kind": "soft", "value": [0.9, 0.1] if row["targets"][0]["value"] == 0 else [0.1, 0.9]}]
    training_path = root / "regularized-train.jsonl"
    training_path.write_text("".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8")
    return training_path, tokenizer_path, config_path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assort", type=Path, required=True)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--epochs", type=int, default=24)
    parser.add_argument("--regularized", action="store_true", help="Compare a smaller, label-smoothed training alternative")
    args = parser.parse_args()
    if not args.assort.is_file() or args.output.exists() or args.epochs < 1:
        parser.error("Use an existing trainer and a new local output directory")
    root = args.output
    generate(root / "data")
    frozen_hash = hashlib.sha256((root / "data/challenge.jsonl").read_bytes()).hexdigest()
    inputs = regularized_inputs(root) if args.regularized else (
        root / "data/train.jsonl", root / "data/tokenizer.json", root / "data/model-config.json")
    baseline = root / "baseline"
    shutil.copytree(args.baseline, baseline)
    for name, path in (("challenge", root / "data/challenge.jsonl"),
                       ("original-test", root / "data/legacy/corrections/test.jsonl")):
        evaluate(args.assort, baseline, path, root / f"baseline-{name}.json")
    model = root / "model"
    with (root / "train.log").open("w", encoding="utf-8") as log:
        subprocess.run([str(args.assort), "train", "--train", str(inputs[0]),
                        "--validation", str(root / "data/validation.jsonl"),
                        "--tokenizer", str(inputs[1]),
                        "--config", str(inputs[2]),
                        "--output", str(model), "--epochs", str(args.epochs),
                        "--batch-size", "16", "--learning-rate", "0.0015", "--seed", "42"],
                       stdout=log, stderr=subprocess.STDOUT, check=True)
    write_contract(model)
    if hashlib.sha256((root / "data/challenge.jsonl").read_bytes()).hexdigest() != frozen_hash:
        raise ValueError("Frozen challenge changed during training; refusing comparison")
    reports = {}
    for name, path in (("challenge", root / "data/challenge.jsonl"),
                       ("original-test", root / "data/legacy/corrections/test.jsonl")):
        reports[name] = evaluate(args.assort, model, path, root / f"candidate-{name}.json")
    print(json.dumps(reports, indent=2))
    print("Reports and artifacts remain local. Review comparison before selecting a release model.")


if __name__ == "__main__":
    main()
