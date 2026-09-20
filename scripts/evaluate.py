"""Offline, reference-based evaluation of the existing transcription CLI."""

import argparse
import json
import math
from pathlib import Path
import re
import subprocess
import time
import unicodedata


POLICY = (
    "Unicode NFKC and casefold; words contain letters/digits and internal "
    "apostrophes; curly apostrophes become ASCII; other punctuation/symbols "
    "separate words. Exact WER separately uses unmodified whitespace tokens."
)
MAX_WORDS = 2000


def words(text, exact=False):
    if exact:
        return text.split()
    text = unicodedata.normalize("NFKC", text).casefold().replace("’", "'")
    return re.findall(r"[^\W_]+(?:'[^\W_]+)*", text, flags=re.UNICODE)


def score(reference, hypothesis, exact=False):
    """Levenshtein WER using linear memory; deterministic S, D, I tie order."""
    ref, hyp = words(reference, exact), words(hypothesis, exact)
    if max(len(ref), len(hyp)) > MAX_WORDS:
        raise ValueError(f"Each clip must have at most {MAX_WORDS} words")
    # Cells are (total edits, substitutions, deletions, insertions).
    previous = [(j, 0, 0, j) for j in range(len(hyp) + 1)]
    for i, expected in enumerate(ref, 1):
        current = [(i, 0, i, 0)]
        for j, actual in enumerate(hyp, 1):
            if expected == actual:
                current.append(previous[j - 1])
                continue
            diagonal, above, left = previous[j - 1], previous[j], current[j - 1]
            choices = [
                (diagonal[0] + 1, diagonal[1] + 1, diagonal[2], diagonal[3]),
                (above[0] + 1, above[1], above[2] + 1, above[3]),
                (left[0] + 1, left[1], left[2], left[3] + 1),
            ]
            current.append(min(choices, key=lambda cell: cell[0]))
        previous = current
    edits, substitutions, deletions, insertions = previous[-1]
    return {
        "reference_words": len(ref), "hypothesis_words": len(hyp),
        "substitutions": substitutions, "deletions": deletions,
        "insertions": insertions, "edits": edits,
        # Silence is useful for hallucination checks, but has no WER denominator.
        "wer": edits / len(ref) if ref else None,
    }


def load_manifest(path, max_items):
    manifest = json.loads(path.read_text(encoding="utf-8-sig"))
    items = manifest.get("items") if isinstance(manifest, dict) else None
    if not isinstance(items, list) or not 1 <= len(items) <= max_items:
        raise ValueError(f"Manifest needs 1 to {max_items} items")
    root = path.resolve().parent
    ids, result = set(), []
    for index, item in enumerate(items, 1):
        if not isinstance(item, dict):
            raise ValueError(f"Item {index} must be an object")
        identity = item.get("id", f"clip-{index:03d}")
        wav, reference = item.get("wav"), item.get("reference")
        if not isinstance(identity, str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,80}", identity):
            raise ValueError(f"Item {index} needs a short alphanumeric id")
        if identity in ids:
            raise ValueError(f"Duplicate id: {identity}")
        ids.add(identity)
        if not isinstance(reference, str) or len(reference) > 30000:
            raise ValueError(f"{identity}: reference must be a string under 30000 characters")
        score(reference, "")  # Validate token bounds before starting inference.
        if not isinstance(wav, str) or not wav:
            raise ValueError(f"{identity}: supply a relative WAV path")
        relative = Path(wav)
        resolved = (root / relative).resolve()
        if relative.is_absolute() or not resolved.is_relative_to(root):
            raise ValueError(f"{identity}: WAV must stay inside the manifest folder")
        if resolved.suffix.lower() != ".wav" or not resolved.is_file():
            raise ValueError(f"{identity}: WAV file is missing or has the wrong extension")
        result.append((identity, resolved, reference))
    return result


def parse_response(stdout):
    rows = []
    for line in stdout.splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue  # Native backend diagnostics may precede the JSON.
        if isinstance(value, dict) and isinstance(value.get("text"), str) and "run" in value:
            rows.append(value)
    if len(rows) != 1:
        raise ValueError("Expected exactly one final CLI JSON result")
    row = rows[0]
    for key in ("audio_ms", "transcribe_ms", "load_ms"):
        value = row.get(key)
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
            raise ValueError(f"CLI result has an invalid {key}")
    if row["audio_ms"] == 0:
        raise ValueError("CLI result has zero audio duration")
    return row


def aggregate(items, metric):
    keys = ("reference_words", "hypothesis_words", "substitutions", "deletions", "insertions", "edits")
    total = {key: sum(item[metric][key] for item in items) for key in keys}
    total["wer"] = total["edits"] / total["reference_words"] if total["reference_words"] else None
    return total


def evaluate(items, executable, model=None, cpu=False, timeout=180, budget=1800, runner=subprocess.run):
    started, results = time.monotonic(), []
    for identity, wav, reference in items:
        remaining = budget - (time.monotonic() - started)
        if remaining <= 0:
            raise ValueError("Evaluation time budget exhausted")
        command = [str(executable), "--transcribe", str(wav)]
        if cpu:
            command.append("--cpu")
        if model is not None:
            command.extend(["--model", str(model)])
        wall = time.monotonic()
        try:
            completed = runner(command, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=min(timeout, remaining), shell=False)
        except subprocess.TimeoutExpired:
            raise ValueError(f"{identity}: inference timed out") from None
        if completed.returncode:
            # Do not accidentally expose local paths or transcript data in logs.
            raise ValueError(f"{identity}: CLI exited with code {completed.returncode}")
        row = parse_response(completed.stdout)
        results.append({
            "id": identity, "reference": reference, "hypothesis": row["text"],
            "normalized": score(reference, row["text"]),
            "exact": score(reference, row["text"], exact=True),
            "backend": row.get("backend"), "audio_ms": row["audio_ms"],
            "transcribe_ms": row["transcribe_ms"], "load_ms": row["load_ms"],
            "process_wall_ms": round((time.monotonic() - wall) * 1000, 2),
            "inference_rtf": row["transcribe_ms"] / row["audio_ms"],
        })
    audio_ms = sum(item["audio_ms"] for item in results)
    inference_ms = sum(item["transcribe_ms"] for item in results)
    return {
        "schema_version": 1, "normalization": POLICY,
        "scope": "Raw CLI ASR, no dictionary, cleanup, macros, diarization, or live partials",
        "cpu_requested": cpu, "items": results,
        "aggregate": {
            "clips": len(results), "normalized": aggregate(results, "normalized"),
            "exact": aggregate(results, "exact"), "audio_ms": audio_ms,
            "transcribe_ms": inference_ms, "inference_rtf": inference_ms / audio_ms,
            "load_ms": sum(item["load_ms"] for item in results),
            "process_wall_ms": sum(item["process_wall_ms"] for item in results),
        },
    }


def bounded(minimum, maximum):
    def parse(value):
        value = int(value)
        if not minimum <= value <= maximum:
            raise argparse.ArgumentTypeError(f"Must be {minimum} to {maximum}")
        return value
    return parse


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--model", type=Path)
    parser.add_argument("--cpu", action="store_true")
    parser.add_argument("--timeout", type=bounded(1, 3600), default=180, help="Per-clip seconds")
    parser.add_argument("--budget", type=bounded(1, 7200), default=1800, help="Whole evaluation seconds")
    parser.add_argument("--max-items", type=bounded(1, 500), default=100)
    args = parser.parse_args()
    try:
        executable = args.exe.resolve(strict=True)
        model = args.model.resolve(strict=True) if args.model else None
        items = load_manifest(args.manifest, args.max_items)
        report = evaluate(items, executable, model, args.cpu, args.timeout, args.budget)
    except (OSError, ValueError) as error:
        parser.exit(1, f"Evaluation failed: {error}\n")
    print(json.dumps(report, ensure_ascii=True, indent=2, allow_nan=False))


if __name__ == "__main__":
    main()
