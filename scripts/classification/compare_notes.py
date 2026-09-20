"""Compare two notes checkpoints on exactly the same frozen synthetic fixtures.

This evaluates models, never trains them or rewrites their predictions. Keep
the output directory ignored; detailed scores are not a public model card.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def model_snapshot(model):
    limits = model / "inference-limits.json"
    if not limits.is_file():
        limits = model / "preprocessing-limits.json"
    snapshot = {
        "weights_sha256": digest(model / "checkpoint/weights.mpk"),
        "tokenizer_sha256": digest(model / "tokenizer.json"),
        "limits_sha256": digest(limits),
        "manifest_sha256": digest(model / "checkpoint/manifest.json"),
        "pipeline_sha256": digest(model / "checkpoint/transcript-pipeline.json"),
    }
    return snapshot


def evaluate(executable, model, fixture):
    limits = model / "inference-limits.json"
    if not limits.is_file():
        limits = model / "preprocessing-limits.json"
    try:
        result = subprocess.run([
            str(executable), "evaluate-transcripts", "--input", str(fixture),
            "--checkpoint", str(model / "checkpoint"),
            "--tokenizer", str(model / "tokenizer.json"), "--limits", str(limits),
        ], capture_output=True, text=True, encoding="utf-8", check=True, timeout=120)
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or "No diagnostic returned").strip()[:4096]
        raise RuntimeError(f"Assort evaluation failed: {detail}") from error
    if len(result.stdout) > 1024 * 1024:
        raise ValueError("Evaluation output exceeded the expected report size")
    return summarize(json.loads(result.stdout))


def summarize(report):
    order = report["category_order"]
    confusion = report["confusion"]
    if (len(order) != 4 or set(order) != {"decision", "action", "key_fact", "background"}
            or len(confusion) != 4 or any(len(row) != 4 for row in confusion)
            or any(type(value) is not int or value < 0 for row in confusion for value in row)
            or sum(map(sum, confusion)) != report["segments"] or report["segments"] <= 0):
        raise ValueError("Evaluation report has inconsistent categories or counts")
    report["per_category"] = {}
    for index, name in enumerate(order):
        actual = sum(confusion[index])
        predicted = sum(row[index] for row in confusion)
        report["per_category"][name] = {
            "recall": confusion[index][index] / actual if actual else None,
            "precision": confusion[index][index] / predicted if predicted else None,
        }
    background = order.index("background")
    count = sum(confusion[background])
    report["background_false_positive_rate"] = (
        1 - confusion[background][background] / count if count else None)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assort", type=Path, required=True)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--evaluation", type=Path, action="append", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("Choose a new, ignored output directory")
    args.output.mkdir(parents=True)
    summary = {"scope": "Frozen authored synthetic examples; no real-world accuracy claim",
               "models": {}, "evaluations": []}
    for name in ("baseline", "candidate"):
        model = getattr(args, name)
        summary["models"][name] = model_snapshot(model)
    for index, fixture in enumerate(args.evaluation):
        before = digest(fixture)
        data = json.loads(fixture.read_text(encoding="utf-8"))
        if not isinstance(data, list) or not data:
            raise ValueError("An evaluation fixture must be a nonempty JSON array of labeled transcripts")
        reports = {name: evaluate(args.assort, getattr(args, name), fixture)
                   for name in ("baseline", "candidate")}
        if digest(fixture) != before:
            raise ValueError("The evaluation fixture changed during comparison")
        for name in ("baseline", "candidate"):
            if model_snapshot(getattr(args, name)) != summary["models"][name]:
                raise ValueError("A model artifact changed during comparison")
        if reports["baseline"]["segments"] != reports["candidate"]["segments"]:
            raise ValueError("The model evaluations did not cover the same passages")
        record = {"fixture_sha256": before, **reports}
        (args.output / f"comparison-{index}.json").write_text(
            json.dumps(record, indent=2) + "\n", encoding="utf-8")
        summary["evaluations"].append({
            "fixture_sha256": before,
            "segments": reports["baseline"]["segments"],
            "baseline_accuracy": reports["baseline"]["category_accuracy"],
            "candidate_accuracy": reports["candidate"]["category_accuracy"],
            "baseline_background_false_positive_rate": reports["baseline"]["background_false_positive_rate"],
            "candidate_background_false_positive_rate": reports["candidate"]["background_false_positive_rate"],
        })
    (args.output / "summary.json").write_text(
        json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
