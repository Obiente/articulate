# Local dictation cleanup

Articulate uses two separate stages. The existing lightweight Rust rules handle
clear stutters and explicit date/number repairs without a model. Optional Polish
edits a finished passage with a local language model and shows a comparison.
Nothing is applied until the user chooses Apply. This changes the saved preview;
it does not replace text in another app. Raw transcription remains available.

## Model and runtime

The initial downloadable profile uses the text component of
[Qwen3.5-0.8B](https://huggingface.co/Qwen/Qwen3.5-0.8B), in the
[ggml-org Q8_0 conversion](https://huggingface.co/ggml-org/Qwen3.5-0.8B-GGUF/tree/8fea620810c4afa23dd6443f999a48574c1611a3).
The model download is 833,592,096 bytes. A lower-precision Q4_0 conversion is
smaller, but this profile selects Q8_0 for the first quality-focused implementation.
The upstream model card describes the 0.8B model as suitable for prototyping and
task-specific development. It is not a guarantee of faithful editing.

The app manages an official
[llama.cpp b10964 Windows x64 CPU runtime](https://github.com/ggml-org/llama.cpp/releases/tag/b10964),
an 18,427,629-byte archive. The source revision is
`b29c606e28a01b1bc8c1351026a0fa6e616bf6c4`. The runtime and model are downloaded
only after an explicit setup action. Neither weights nor runtime executables are
included in the installer. The existing transcribe-cpp API is for audio inference;
it does not provide the general text-generation interface needed here.

Fixed download URLs, exact byte counts and SHA-256 checks bind both artifacts.
Only the server executable and its required CPU DLLs are extracted from the
verified archive. Their individual hashes are compiled into Articulate and
checked before execution. The RPC executable/backend is not installed. Licenses
for Qwen, llama.cpp and LLVM OpenMP accompany the downloaded files. Repair stages
verified replacements before swapping a damaged runtime, preserving a rollback
path if the swap fails.

## Runtime limits and privacy

Rust owns the server process, request queue, verification, cancellation and result
validation. The child listens only on loopback, uses a fresh authentication key,
has its web interface disabled and writes no request logs. The HTTP client uses
no proxy or redirects. No transcript, vocabulary, audio or user identifier is
included in model/tool downloads. Inference requests stay on the same computer.

The server uses CPU inference, at most eight threads, one request slot and a
2,048-token context. A preview accepts at most 200 words and 1,600 UTF-8 bytes,
with at most 512 output tokens and a 60-second request deadline. Startup has a
45-second deadline. On Windows, a job object caps process memory at 4 GiB and
terminates the child if Articulate exits. The warm model unloads after 60 seconds
without another request. Cancelling a running generation terminates that owned
server; a later request starts a fresh process.

## Editing policy

Clear, Professional and Casual select punctuation, capitalization and spacing
preferences. They do not authorize adding facts, greetings or invented words.
The model sees only the selected passage, not other apps, document titles or
conversation history. Expanded voice shortcuts keep their literal contents and
are excluded from Polish.

The comparison checks against the existing conservative-cleaned text, allowing
earlier explicit repairs to remain intact. The current rejection gate preserves
ordered content words, numbers, symbols, saved phrases and unique clauses. It
permits filler/article cleanup, limited subject/verb agreement, equivalent
contractions, and reduction of exact adjacent repeated phrases. It rejects
unfinished output, tool calls, reasoning output and unrequested substantive
rewrites. A rejected result keeps the current wording and explains why.

These checks do not prove semantic equivalence. Users review every accepted
draft. Small-model trials showed useful punctuation, grammar and repeated-clause
editing, but also incorrect handling of ambiguous self-corrections, spoken
punctuation and quote markers. Those behaviors are not advertised as reliable.
The model is not used to guess what missing or misrecognized audio meant.

## Validation

Unit tests cover protected text, negation, numbers, role changes, incomplete
responses, archive inventory and cancellation. The ignored
`polish::tests::managed_cpu_editor_smoke` test exercises the actual Rust worker,
verified local runtime, model, response parser and guard. It requires an explicitly
prepared ignored `ARTICULATE_POLISH_TEST_DIR` containing the pinned model and
`runtime.zip`; it is not part of normal tests or CI downloads. Keep machine-specific
timings, generated text and evaluation traces in ignored local storage.

## Product reference

[Wispr Flow's Auto Cleanup documentation](https://docs.wisprflow.ai/articles/4283510616-Auto-Cleanup:-control-how-much-Flow-edits-your-dictation)
distinguishes raw output, light filler/grammar editing, and more extensive clarity
editing. Articulate uses the same broad need as a reference, while retaining an
explicit local preview for the generative stage. It does not claim equivalent
model quality or identical behavior.
