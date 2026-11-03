# Local dictation cleanup

Clean and Chat writing styles automatically remove common English fillers,
short repeated phrases and supported explicit spoken corrections during dictation
and before final insertion into another app. These shared Rust rules run without
a language model or download. Verbatim keeps the original wording. Enabled
vocabulary phrases and literal shortcut templates are preserved.

Optional Polish adds a local language-model editing preview for punctuation and
style and saved vocabulary. Manual previews require Apply and update Articulate's saved text;
they do not revise an external app after insertion. Raw transcription stays available.

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

Downloads retain a hash-specific partial file and reconnect up to six times.
A resumed response must provide the exact expected byte range and total size;
servers that ignore Range restart the file safely. A 30-second network idle
limit is separate from the one-hour transfer budget. Cancellation is checked
between 250-millisecond socket polls. Final size and SHA-256 verification are
mandatory before a file can be loaded. Failed network transfers can be resumed
by choosing Download again. The complete first-time editor download is
852,019,725 bytes; already verified files are reused.

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
The model sees the selected passage and up to 20 enabled vocabulary entries scoped
to the destination app. Heard/wanted pairs and context words are ranked by spelling
and sound similarity, then fitted within the 3,500-byte combined prompt budget.
It does not see other apps, document titles or conversation history. Expanded voice shortcuts keep their literal contents and
are excluded from Polish.

Polish builds a safe baseline by removing supported fillers such as an isolated
"um", reducing short adjacent repetitions, and handling explicit bounded repairs
such as "Tuesday, sorry, Thursday". It preserves ambiguous repetitions and
phrases that could change negation, quantities or intent. The comparison checks
the model output against this baseline, allowing those supported repairs to
remain intact. The current rejection gate preserves
ordered content words, numbers, symbols, saved phrases and unique clauses. It
permits filler/article cleanup, limited subject/verb agreement, equivalent
contractions, and reduction of exact adjacent repeated phrases. It rejects
unfinished output, tool calls, reasoning output and unrequested substantive
rewrites. If the model result is rejected, the After preview retains the safe baseline
cleanup and explains that the additional model edit was rejected. When the rules
find nothing safe to change, that preview can remain identical to the original.
Manual previews require **Apply changes**.

These checks do not prove semantic equivalence. Manual drafts remain reviewable. Small-model trials showed useful punctuation, grammar and repeated-clause
editing, but also incorrect handling of ambiguous self-corrections, spoken
punctuation and quote markers. The bounded rules do not make arbitrary spoken mistakes or paragraph-level
restarts reliable.
Saved vocabulary can replace a similar span of one to four words. Matching uses
normalized letters and a compact phonetic key: at most 20% letter edits with the
same initial, or 34% with an identical phonetic key. Terms shorter than four
letters and spans containing non-ASCII letters are excluded. Negations, numbers, identifiers, symbols and existing saved
phrases remain protected; content order outside the replacement stays unchanged.

## Automatic cleanup

Enable **Clean up finished dictations automatically** in Dictation settings after
installing the advanced cleanup model. It defaults to off. Eligible finished
passages run before final insertion, including replacement of live typing previews.
Verbatim style, snippets and passages beyond the input limits skip this step.
The total deadline is four seconds, including queueing and cold loading. There is
no cold-start extension: a slow load, error or rejected draft keeps the existing
rule-based text. The overlay shows Cleaning up while waiting, then reports the
insertion result. Cancellation or another dictation abandons the pending result.
History keeps raw speech and the final text separately. Automatic results are never
learned as corrections. Manual previews retain their longer request deadline.

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

## Desktop review

Choose **Polish** beside a finished dictation. The dialog shows the safe baseline and a textual diff of the proposed edit. **Apply** saves an allowed preview; **Keep original** and **Cancel** discard it. Blocked previews explain why the edit was rejected; cleanup-only previews can still be applied explicitly. A missing cleanup model offers a download with progress and cancellation.
