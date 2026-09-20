# Articulate classification training

## Bundled English notes candidate

The release notes classifier has a separate reproduction recipe:

```powershell
python scripts/classification/train_notes.py --assort /path/to/assort.exe --output .local/assort-notes-run
```

Use the Assort CLI built from revision
`d6cf161ac6db63ba221cfef6e9d7975d09f38d6d`. `notes_corpus.py` supplies authored
English examples with separate sentence families and topic names for training,
validation, and test. Examples include negated decisions, actual constraints,
quoted assignments, conditional promises and unconfirmed numbers. Conversations
do not have a fixed one-passage-per-category distribution. Training entity words
are mapped to the unknown token so unseen participant names receive a trained
embedding instead of becoming an untested special case.

The recipe trains a 64-wide Assort model for 32 epochs, selects weights using
validation loss, and then evaluates the fixed test split once. It stages only
the checkpoint, tokenizer, inference limits, license, model card and public
provenance in `bundle/`. Datasets, detailed evaluations and logs remain in the
ignored run directory. Reproducing the recipe does not publish any files.
CPU/library versions can affect floating-point training results.

Bundling makes review suggestions usable without model-path configuration. It
does not make synthetic results evidence of real-call accuracy: the limited
English vocabulary and unfamiliar speech still require user review. No automatic
actions or generated meeting facts are enabled by this model.

## Other development recipes

This pipeline trains three small, independent models with the existing Rust Assort
CLI. Python only generates authored synthetic scenarios, starts the trainer, and
evaluates its typed decisions. No real transcripts, dictionaries, Discord data,
or account information are read. The app's speech recognizer remains Qwen ASR.

Normal Articulate users do not run this pipeline. Releases include pretrained
notes and vocabulary classifiers, with **Create notes** and **Review vocabulary**
available without selecting files. These are narrow models trained on authored
synthetic English examples, not general language understanding. Suggestions stay
reviewable and require an explicit choice before they change notes or text.
The vocal-shortcut classifier remains a development task; deterministic shortcuts
remain authoritative. Large speech and acoustic speaker models are still downloaded
separately.

Build Assort separately using its own instructions. Pass its executable explicitly:

```powershell
python scripts/classification/train.py --assort /path/to/assort.exe --output .local/assort-articulate-run --epochs 12 --seed 42
python scripts/classification/train.py --assort /path/to/assort.exe --output .local/assort-correction-run --task corrections --epochs 12 --seed 42
python -m unittest discover -s scripts/classification -p 'test_*.py'
```

The output directory must be new. All generated datasets, tokenizer files,
checkpoints, prediction details, logs, and score reports stay in that local
directory. Do not add generated datasets, logs, private scores or evaluation traces
to the source repository. These scripts neither download nor publish checkpoints.
Release preparation may deliberately select a trained notes/correction package,
public provenance card and license for embedding after review. This selection is
separate from running the trainer.

## Tasks and contracts

`generate.py` is the authoritative prompt/candidate contract. It also writes
`data/contracts.json`. Candidate order is metadata, not a substitute for the
descriptions. An app adapter must use these exact question and candidate texts.

| Task | Candidate IDs | Intended use |
|---|---|---|
| Meeting notes | `decision`, `action`, `key_fact`, `background` | Reviewable source-span highlights with original speaker and time. Uses Assort's existing `assort-transcript` contract. |
| Dictionary correction | `keep_original`, `replace` | Score an already proposed, confirmed, app-scoped dictionary replacement. Never generate replacement text. |
| Vocal shortcut intent | `literal`, `command`, `incomplete`, `unknown` | Classify an utterance against one registered text/template trigger. Never open URLs or perform arbitrary actions. |

Meeting training uses `train-transcripts`, so the checkpoint includes the real
trainer-written `transcript-pipeline.json`. Do not fabricate that metadata for an
older, incompatible checkpoint. The generic classifiers use `train`, then `infer`
for per-example held-out decisions. All three start from random weights.

The correction recipe also writes `checkpoint/articulate-task.json` after a
successful training run, plus `inference-limits.json` using the trainer's token
lengths and a smaller one-question/two-candidate inference batch. These files
make optional advanced imports usable without guessing which task a checkpoint
learned. The included model needs no import. Existing checkpoints are not given invented
task metadata by evaluation mode.

Correction examples include surrounding sentences, destination app labels and
vocabulary cues. Separate families contrast using a registered proper name with
verbatim quotation of its original spelling. These authored distinctions are a
development benchmark, not evidence that a small synthetic model understands
arbitrary user context.

Production callers must keep typed original/proposed/context fields separately;
do not parse user text back out of the multiline model prompt. The evaluator's
simple parsing is exclusively for its controlled synthetic fixtures.

## Splits and evaluation

Seed 42 fixes example shuffling. Authored sentence-template families 0 through 5
are training, 6 through 7 validation, and 8 through 9 test. Topics, dictionary
terms, and shortcut names are also disjoint across splits. The generator rejects
normalized text overlap. Shared task syntax and category vocabulary are
intentional. This is a small compositional development benchmark, not an
independent sample of real conversations. The WordLevel vocabulary is fitted
only on training prompts. Unknown test words remain unknown.

The baseline uses 12 epochs, a 32-wide single-layer model, validation-loss model
selection, and no test-set tuning. Notes use Assort's native training configuration
and label smoothing. Classifier action acceptance uses a fixed 0.90 probability
threshold; it is not a calibrated confidence claim. Reports include confusion
matrices, action false positives, gated actions, and exact-preservation
abstentions. Notes also report the background false-positive rate and Assort's
source-highlight precision/recall against a lead baseline.

For an existing development layout containing `data-v1`, `notes-v1`,
`corrections-v1`, and `shortcuts-v1`, use `--evaluate-existing`. That only evaluates
already selected checkpoints and does not train or change their weights.

## Guardrails and promotion

A learned score cannot bypass deterministic eligibility: dictionary entries must
be confirmed and in scope, protected negations/numbers/currency must remain
unchanged, and shortcuts must start with an exact complete registered trigger
while a text field is focused. The sample protection vocabulary is English; it
does not claim multilingual semantic safety. A production replacement still
requires the actual stored rule, not a model-provided assertion of confirmation.
Abstention preserves the input exactly. The classifier must never rewrite an
in-progress transcript just to make its prediction fit.

Do not promote a model from accuracy alone. A model that rejects every command
can have zero unsafe expansions while being useless. Inspect positive recall,
held-out errors, unseen names, ambiguity, language coverage, latency, and
consenting real-world examples before changing app behavior. Keep deterministic
macros authoritative until a learned model demonstrates useful recall as well as
acceptable false positives. Notes remain reviewable exact quotes rather than
invented summaries. Synthetic scores and generated output files belong in local
reports, not public claims about product quality.

## Preparing the bundled tasks

Release bundles contain exactly `notes` and `corrections`. Each task has a stable
model ID and a list of relative file paths with SHA-256 hashes in a schema-1
`manifest.json`. A task directory contains:

- `checkpoint/manifest.json` and `checkpoint/weights.mpk` from actual training.
- `tokenizer.json` and `inference-limits.json` matching that checkpoint.
- `checkpoint/transcript-pipeline.json` for notes, or
  `checkpoint/articulate-task.json` for corrections.
- `evaluation.json`, a deliberately authored **public model card** describing
  provenance, synthetic-English limitations and required review. Despite its
  filename, this is not a private score report or evaluation trace.
- `LICENSE.txt` with the reviewed model distribution terms.

The build embeds a deliberately selected bundle from `assets/assort`, or from
the developer-supplied `ARTICULATE_ASSORT_MODELS_DIR`. The build verifies hashes;
the runtime independently verifies hashes, trained weights, tokenizer matching,
task compatibility and bounded inference limits. Run
`articulate --verify-assort-models` on the built executable to verify both embedded
tasks without extracting files. A development build may omit the bundle, but
such a build must not pass the release model verification check.

Preserve the exact training contracts. A new task or changed prompt requires
compatible retraining and an adapter change, not relabeling an old manifest.
Do not copy a failed model into a release just to populate its files. Keep
private evaluations local, and do not describe synthetic training or hash
verification as proof of real-world transcription or classification quality.
