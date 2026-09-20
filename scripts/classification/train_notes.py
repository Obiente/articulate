"""Train an English notes candidate from authored synthetic data.

The trainer selects a checkpoint by validation loss. Only after training finishes
does this recipe evaluate the fixed test split. Reports and model artifacts stay
in the explicitly selected new output directory, which should be ignored.
This does not replace the release model or constitute approval to bundle it.
"""
import argparse
import collections
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import time

from notes_corpus import generate, TOPICS

ASSORT_REVISION = "d6cf161ac6db63ba221cfef6e9d7975d09f38d6d"


def model_inputs(training_file, output):
    """Fit only training vocabulary; teach [UNK] through synthetic topic/name tokens.

    Entity words are omitted from the vocabulary, not deleted from sentences.
    This prevents memorizing two actor names as a proxy for action ownership and
    gives the unknown embedding actual training examples before unseen names.
    """
    corpus = json.loads(training_file.read_text(encoding="utf-8"))
    texts = ["Goal Transcript Classify this passage",
             "A final decision or agreed choice to retain in the summary",
             "An assigned action with an owner or concrete next step",
             "An important fact, constraint, result, or blocker",
             "Background, small talk, repetition, or an unconfirmed suggestion"]
    for item in corpus:
        texts.append(item["transcript"]["goal"])
        for segment in item["transcript"]["segments"]:
            texts.append(segment["text"])
            if segment["speaker"]:
                texts.append(segment["speaker"])
    counts = collections.Counter(token for text in texts for token in re.findall(r"\w+|[^\w\s]+", text.lower()))
    omit = {"casey", "jordan"} | {word for topic in TOPICS["train"] for word in topic.split()}
    vocabulary = {"[PAD]": 0, "[UNK]": 1}
    for token, _ in sorted(counts.items(), key=lambda pair: (-pair[1], pair[0])):
        if token not in omit:
            vocabulary.setdefault(token, len(vocabulary))
    tokenizer = {
        "version": "1.0", "truncation": None, "padding": None,
        "added_tokens": [{"id": i, "content": token, "single_word": False,
                          "lstrip": False, "rstrip": False, "normalized": False,
                          "special": True} for token, i in list(vocabulary.items())[:2]],
        "normalizer": {"type": "Lowercase"}, "pre_tokenizer": {"type": "Whitespace"},
        "post_processor": None, "decoder": None,
        "model": {"type": "WordLevel", "vocab": vocabulary, "unk_token": "[UNK]"},
    }
    config = {"vocab_size": len(vocabulary), "hidden_size": 64, "num_heads": 4,
              "state_layers": 1, "query_layers": 1, "ffn_size": 128,
              "max_positions": 512, "dropout": 0.2, "candidate_state_attention": False,
              "max_attention_elements": 16777216}
    for name, value in (("entity-tokenizer.json", tokenizer), ("model-config.json", config)):
        (output / name).write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def run(executable, arguments, output):
    start = time.monotonic()
    with output.open("w", encoding="utf-8") as log:
        subprocess.run([str(executable), *map(str, arguments)], stdout=log,
                       stderr=subprocess.STDOUT, check=True)
    return time.monotonic() - start


def evaluate(executable, model, data, report):
    start = time.monotonic()
    result = subprocess.run([
        str(executable), "evaluate-transcripts", "--input", str(data),
        "--checkpoint", str(model / "checkpoint"),
        "--tokenizer", str(model / "tokenizer.json"),
        "--limits", str(model / "preprocessing-limits.json"),
    ], capture_output=True, text=True, encoding="utf-8", check=True)
    parsed = json.loads(result.stdout)
    parsed["evaluation_seconds"] = time.monotonic() - start
    parsed["scope"] = "Authored English synthetic test only; not real speech accuracy."
    categories = parsed["category_order"]
    confusion = parsed["confusion"]
    parsed["per_category"] = {
        name: {
            "recall": confusion[i][i] / sum(confusion[i]) if sum(confusion[i]) else None,
            "precision": confusion[i][i] / sum(row[i] for row in confusion)
            if sum(row[i] for row in confusion) else None,
        } for i, name in enumerate(categories)
    }
    report.write_text(json.dumps(parsed, indent=2) + "\n", encoding="utf-8")
    return parsed


def bundle(model, destination, seed=42, epochs=32):
    """Stage exact trained artifacts and honest usage terms; never score reports."""
    destination.mkdir(parents=True, exist_ok=False)
    checkpoint = model / "checkpoint"
    manifest = json.loads((checkpoint / "manifest.json").read_text(encoding="utf-8"))
    assert manifest["weights_status"] == "trained"
    assert manifest["architecture"] == "assort-candidate-scoring-v1"
    assert hashlib.sha256((checkpoint / "weights.mpk").read_bytes()).hexdigest() == manifest["weights_sha256"]
    (destination / "checkpoint").mkdir()
    for name in ("manifest.json", "weights.mpk", "transcript-pipeline.json"):
        shutil.copyfile(checkpoint / name, destination / "checkpoint" / name)
    shutil.copyfile(model / "tokenizer.json", destination / "tokenizer.json")
    limits = json.loads((model / "preprocessing-limits.json").read_text(encoding="utf-8"))
    limits["max_batch_size"] = 1
    (destination / "inference-limits.json").write_text(json.dumps(limits, indent=2) + "\n", encoding="utf-8")
    repository = Path(__file__).resolve().parents[2]
    shutil.copyfile(repository / "LICENSE", destination / "LICENSE.txt")
    (destination / "evaluation.json").write_text(json.dumps({
        "version": 1, "model_id": "articulate-notes-en-candidate",
        "distribution_status": "Candidate; independent evaluation required before release selection",
        "training_source": "Authored synthetic English meeting passages; no user or account data",
        "selection": "Minimum validation cross entropy; test split excluded from training and checkpoint selection",
        "held_out_scope": "Disjoint sentence template families and topic names; English synthetic examples only",
        "real_world_evaluation": False, "calibrated_confidence": False,
        "use": "Reviewable source-quote suggestions only; never automatic actions",
        "assort_revision": ASSORT_REVISION,
        "recipe": "scripts/classification/train_notes.py",
        "corpus": "scripts/classification/notes_corpus.py",
        "corpus_source_sha256": hashlib.sha256((repository / "scripts/classification/notes_corpus.py").read_bytes()).hexdigest(),
        "seed": seed, "epochs": epochs,
    }, indent=2) + "\n", encoding="utf-8")
    (destination / "MODEL-CARD.md").write_text(
        "# Articulate English notes training candidate\n\n"
        "This staged candidate has not been selected for release. Compare it with the "
        "existing model on unchanged regression cases and a separately authored sealed audit "
        "before deciding whether to distribute it. Training metrics alone are insufficient.\n\n"
        "This small Assort model was trained before distribution on authored synthetic English "
        "meeting passages. It scores decision, action, key_fact and background categories. "
        "It is a review assistant, not a general language model or a verified record of a meeting.\n\n"
        "Review every proposed highlight. Suggestions quote source passages and never authorize "
        "actions, change dictionary rules, or generate missing facts. Negation, jokes, conditional "
        "promises, unfamiliar vocabulary, mixed languages and recognition errors can still be "
        "misclassified. Importance scores are not calibrated correctness probabilities.\n\n"
        "Training and validation use separate authored sentence families and topic names. "
        "A held-out synthetic evaluation is a development check, not evidence of real-world "
        "meeting accuracy. The model has not been evaluated on a representative recorded-speech corpus. "
        "Its WordLevel tokenizer has a limited English vocabulary and maps unseen words to an unknown token.\n\n"
        "No user transcripts, Discord data, names or account information were used. All inference "
        "runs locally. These weights and authored training materials use AGPL-3.0-or-later; "
        "see LICENSE.txt. Assort source: https://github.com/Obiente/assort/tree/" + ASSORT_REVISION + "\n\n"
        "Reproduction source: https://github.com/Obiente/articulate/tree/main/scripts/classification\n",
        encoding="utf-8")
    files = {p.relative_to(destination).as_posix(): {"sha256": hashlib.sha256(p.read_bytes()).hexdigest(),
                                                  "bytes": p.stat().st_size}
             for p in sorted(destination.rglob("*")) if p.is_file()}
    (destination / "bundle.json").write_text(json.dumps({
        "id": "articulate-notes-en-candidate", "version": 1, "task": "meeting-notes-review",
        "language": "en", "license": "AGPL-3.0-or-later", "assort_revision": ASSORT_REVISION,
        "automatic_actions": False, "files": files,
    }, indent=2) + "\n", encoding="utf-8")
    return files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assort", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--epochs", type=int, default=32)
    parser.add_argument("--seed", type=int, default=42)
    args = parser.parse_args()
    if not args.assort.is_file() or args.output.exists() or args.epochs < 1:
        parser.error("Use an existing Assort executable, a new output directory and positive epochs")
    root = args.output
    generate(root / "data", args.seed)
    model_inputs(root / "data/train.json", root)
    model = root / "model"
    seconds = run(args.assort, ["train-transcripts", "--train", root / "data/train.json",
                  "--validation", root / "data/validation.json", "--output", model,
                  "--tokenizer", root / "entity-tokenizer.json", "--config", root / "model-config.json",
                  "--epochs", args.epochs, "--batch-size", 16, "--learning-rate", 0.0015,
                  "--seed", args.seed], root / "train.log")
    report = evaluate(args.assort, model, root / "data/test.json", root / "test-report.json")
    files = bundle(model, root / "bundle", args.seed, args.epochs)
    print(json.dumps({"training_seconds": seconds, "test": report, "bundle_files": files}, indent=2))


if __name__ == "__main__":
    main()
