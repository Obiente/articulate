# Assort classification in Articulate

[Assort](https://github.com/Obiente/assort) is Obiente's Rust model for scoring explicit choices. Articulate links its inference, tokenizer, model, data, and transcript crates into the application with Burn Flex on CPU. Users do not install an Assort executable, Python, CUDA, or a service. The speech recognizer remains Qwen ASR.

The application includes two small pretrained Assort models: meeting notes and contextual vocabulary review. **Create notes** and **Review vocabulary** use them without downloads, executable selection or manual model paths. Large ASR and acoustic speaker models remain separate downloads. Assort finds reviewable decisions, actions and key facts, or scores existing vocabulary choices; it never generates replacement prose or automatically changes dictionary entries or shortcuts. Vocabulary review changes the finished preview only after an explicit user choice.

These task-specific models use authored synthetic English examples. They are not general language models, and synthetic training does not establish accuracy on real meetings, unfamiliar names, ambiguous speech, or other languages. Their scores are suggestions to review, not permission to perform an action. Public model cards describe provenance and limitations; private datasets, score reports and evaluation traces are not distribution assets.

## Built-in model packages

The release embeds a versioned manifest and the selected trained weights, tokenizer, inference limits, training contract, public provenance card and license for each task. On first use it materializes the selected package in a content-addressed local cache. Existing valid files are reused; missing or damaged cache files are repaired from the executable. Cache directory junctions, reparse points and symlink files are rejected. Models never execute code.

Both the parent and classification worker check every package file against hashes pinned in the executable. They also verify the trained manifest, tokenizer fingerprint and exact task contract. A notes model cannot be used for corrections. `articulate --verify-assort-models` validates both embedded tasks without extracting files or loading model weights. Releases must include both tasks; a development build without the bundle explicitly reports that its models are missing.

**Settings → Audio & models → Assort → Advanced: import a model** optionally selects developer-trained files. Imported packages require a separate explicit preview opt-in and the same trained/task checks. They do not silently replace the built-in models merely because an old configuration contains file paths.

## Runtime

The application starts a hidden copy of its own executable in internal classification-worker mode. That process uses the bundled Rust libraries directly. The parent sends one bounded JSON transcript through standard input, reads a bounded JSON summary, and supervises cancellation, output limits, and a 60-second timeout. Classification cannot occupy the recording or UI thread. No external executable path is accepted.

The package verifier requires trained weights, a matching tokenizer fingerprint, bounded preprocessing limits, and the transcript trainer's `transcript-pipeline.json` with version 1 and the exact ordered categories: `decision`, `action`, `key_fact`, `background`. The manifest must name `assort-candidate-scoring-v1`. Legacy checkpoints must be retrained with Assort; changing their manifest is not a substitute for matching training.

The worker uses Assort's token-aware transcript windows, inference engine, and source-passage selection. Returned text, speaker, segment identity, and timestamps must exactly match the input. Unknown, duplicated, reordered, changed, or over-budget quotes are rejected. A result can be added to notes only after the user reviews it and only while the source transcript is unchanged. An empty selection is a valid abstention.

Assort's importance score is `1 - P(background)`. It ranks candidate passages; it is not a calibrated probability that a passage is correct or important.

## Contextual vocabulary review

Finished dictation has a **Review vocabulary in context** panel. Its **Review vocabulary** button uses the included correction model. This model scores two fixed choices, `keep_original` and `replace`, for an existing enabled vocabulary rule. Its inputs include the original phrase, saved spelling, nearby recognized text, destination app, and the rule's context cues. Text inside that prompt is never parsed into authorization or replacement instructions.

The default dictionary pipeline remains deterministic. Assort's review shows whether its model favors the existing correction, the original phrase, or neither clearly. Only an explicit choice can change the finished preview. Acceptance rechecks the original recognition, current preview, app, unchanged saved rule, and a unique matching phrase; stale or ambiguous results require a fresh review or manual edit. This does not change text already inserted into another app, create vocabulary entries, or alter unrelated cleanup. Protected numbers, currency and English negations cannot be changed by this review path. These checks do not claim full multilingual semantic understanding.

Developers can train the correction task using the public recipe with `--task corrections`. After successful training it writes `checkpoint/articulate-task.json` with the exact prompt/candidate contract and `inference-limits.json`. An optional advanced import uses those files and the matching tokenizer. Normal use needs no training or file selection. Old checkpoints without this contract are rejected; the app does not infer their intended task from a filename. Model preference is not calibrated correctness, and no synthetic benchmark enables automatic rewriting.

Both classification tasks share the supervised worker. On Windows its job limits committed memory to 2 GiB and ends the worker if the parent closes. Neither task sends model inputs to a network service.

## Discord profile pictures

Discord avatar display is independent of Assort. The connection forwards only a validated user ID and avatar hash. Articulate constructs a fixed Discord CDN PNG address, downloads at most 256 KiB with no redirects, verifies image dimensions and decoding, and caches it locally. The UI uses cached bytes with initials as a fallback. Audio, transcript text and participant display names are not included in avatar requests. Cache entries are bounded and separate from saved session data.

## Training and model promotion

The [training recipes](../scripts/classification/README.md) create independent synthetic baselines for notes, proposed dictionary replacements, and vocal-shortcut intent. They run the separate developer training CLI. Generated datasets, checkpoints, and reports stay in ignored local directories. The scripts do not publish anything. Release preparation deliberately selects only the two trained task packages, public model cards and license notices for embedding; vocal-shortcut intent remains a development model.

Before changing a bundled model, pin its checkpoint, tokenizer, preprocessing limits, task metadata, license and public provenance card. Evaluate by conversation and template family, keep validation and test data separate, and measure per-class precision/recall, incorrect actions, abstention, positive recall, CPU latency, memory, noise, names, supported languages, and out-of-domain behavior. Keep detailed scores and private examples local. Synthetic accuracy alone is not sufficient to enable automatic actions.

Existing deterministic text macros remain authoritative. Correction candidates must come from confirmed dictionary rules with matching scope and preserved protected content. A classifier's score cannot create a new rule or bypass these checks. Assort does not generate prose, separate audio, identify speakers, or recover missing speech.

## License and source

Articulate and Assort use AGPL-3.0-or-later. The application pins the public Assort source revision in Cargo and includes the appropriate notices in its distribution. Each selected model package includes its reviewed license and public provenance card. Third-party Rust crates keep their own licenses.
