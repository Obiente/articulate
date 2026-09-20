# Offline transcription evaluation

This tool measures the existing `--transcribe` CLI against exact human references.
It uses Python's standard library, starts no downloads, and makes no network calls.
The executable and model must already be available locally.

Keep recordings, manifests, and generated reports in the ignored `.local/accuracy/`
folder. Reports contain reference and recognized text; do not commit or publish them.
Only synthetic examples and tests belong in source control.

Create `.local/accuracy/manifest.json`, with WAV paths relative to that manifest:

```json
{
  "items": [
    {"id": "synthetic-example", "wav": "clips/example.wav", "reference": "The exact words spoken in this recording."}
  ]
}
```

The example is a schema illustration, not a supplied recording or benchmark score.
Paths must remain inside the manifest folder. IDs must be unique and contain only
letters, digits, underscores, or hyphens. An empty reference can test silence.

From the repository root in PowerShell:

```powershell
python scripts/evaluate.py .local/accuracy/manifest.json --exe target/release/transcribe-local.exe > .local/accuracy/gpu-report.json
python scripts/evaluate.py .local/accuracy/manifest.json --exe target/release/transcribe-local.exe --cpu > .local/accuracy/cpu-report.json
python -B -m unittest discover -s scripts -p "test_evaluate.py"
```

Use `--model` to select an already-downloaded model. Each clip starts a fresh process.
`--timeout` bounds each process in seconds (default 180), and `--budget` bounds the
evaluation's inference runs (default 1800 seconds). `--max-items` defaults to 100.
Use short, representative clips; each reference and hypothesis is limited to 2000
words. A failed or timed-out clip aborts the report instead of silently improving
the aggregate by dropping difficult samples.

The report includes normalized and exact word error rate (WER), substitution,
deletion and insertion counts, model-load time, inference time, and process wall
time. Normalization applies Unicode NFKC and case folding, preserves internal
apostrophes, and treats other punctuation and symbols as word separators.
Exact WER compares unmodified whitespace-delimited tokens, so case and punctuation
matter. Languages without word spaces need a language-specific tokenizer or a
separate character error rate evaluation; this tool does not claim valid WER for
those languages.

Aggregate WER is total edit count divided by total reference words, not an average
of clip percentages. Silence has a null per-clip WER; any hallucinated words still
count as insertions in an aggregate with nonempty references. WER can exceed 100%.
Inference real-time factor is inference milliseconds divided by audio milliseconds:
below 1 means inference was faster than playback. It excludes model loading and
does not measure perceived live latency. Aggregate RTF is duration-weighted.

This evaluates raw ASR only, without the app's cleanup, learned dictionary, vocal
macros, live partial revisions, or speaker labels. Keep those acceptance checks
separate. Use a held-out recording set covering your microphone, speaking pace,
names, technical vocabulary, accents, hesitations, background noise, and silence.
Do not tune on every evaluation clip or claim competitive accuracy without running
the same held-out audio through the systems being compared.
