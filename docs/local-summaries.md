# Local summaries

Articulate builds one editable **Notes** document from a call or dictation on
this computer. During calls, notes update automatically when the optional model
is installed. The transcript remains separate. Supporting excerpts are available
under **Transcript sources**, rather than separate editable cards.

The optional model is **Qwen3.5-4B, Q5_K_M**, an Unsloth GGUF conversion of
[Qwen's Apache-2.0 model](https://huggingface.co/Qwen/Qwen3.5-4B).
The [conversion](https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/tree/e87f176479d0855a907a41277aca2f8ee7a09523)
is pinned to a specific revision and SHA-256. It is a separate download, around
3.14 GB, and is not included in the installer. The managed llama.cpp runtime
communicates only through an authenticated loopback connection.
A PC with at least 16 GB of RAM is the
intended starting point; available memory and other running models affect it.

## Runtime installation

The notes-model download also prepares a CPU runtime and attempts to install a
separate optional Vulkan runtime for graphics acceleration. Both come from the
official [llama.cpp `b10964` release](https://github.com/ggml-org/llama.cpp/releases/tag/b10964). The Vulkan archive is about 31.7 MB; its
archive SHA-256 and each installed server/DLL file are pinned and verified.
Only the server and its required libraries are extracted, excluding the RPC
server and unrelated command-line tools. The CPU runtime remains available as
a fallback when GPU tools cannot be downloaded or used. Cancelling stops the
operation without removing an already working model or runtime.

Runtime archives and models stay in Articulate's local model directory and are
downloaded after installation rather than bundled with the application.
Existing installations can choose **Download acceleration** in Settings. A
compatible graphics card is used automatically, with CPU fallback if loading
fails. The summary model stays loaded for up to 60 seconds after use so live
updates can reuse it. The CPU fallback uses up to eight threads.

## Live notes and editing

During a call, automatic updates use committed speech, leaving revisable ASR text
out of the notes input. An attempt requires at least 40 new words and at least
45 seconds since the previous attempt. After capture ends, remaining speech can
trigger an immediate final update even when it is shorter than 40 words.
Generation time comes on top of this interval and depends on the computer.

Use **Live updates** to pause or resume automatic updates. **Update notes** requests
an update for the open transcript. Each transcript owns its generation state and
saved document, including when you navigate elsewhere. You can write directly in
the document; updates retain manually typed paragraphs and revise generated
paragraphs that still match their previous wording. Review the result when mixing
your edits with generated material.

For an older saved summary without document text, **Restore generated notes**
places it into the editor. The former Highlights and Assort meeting-classification
panels are removed; Assort remains available for vocabulary correction review.

## Conversation titles

The model generates a short descriptive title in the same request as the notes.
For long conversations, the final section also sees topic titles from previous
sections within a fixed budget. No separate title generation pass is needed.
Titles can miss or misinterpret topics and are editable. A manual rename is
retained on later updates. Older drafts without a title remain readable.

New calls use **Conversation** before a model-generated title is available. Legacy
first-utterance titles remain visible until notes are updated; they are then
eligible for a model title. Unknown legacy names are kept as manual titles. Opening an old transcript does not start model work
by itself. **Update notes** can generate its title along with its document.

## Reviewing notes

Generated facts, decisions, and actions are suggestions. Checking a citation
proves which text was referenced; it cannot prove that the model interpreted it
correctly. Review the wording, including names, negations, conditions, owners,
and deadlines, against the displayed excerpts before using a point.
The document is not a verified task or decision register. Important conditions
or changes of plan can be missed.

The application supplies each quotation directly from the transcript rather
than trusting the model to reproduce it. Times always refer to the original
speaker turn. A sentence excerpt does not receive an invented word-level time.
Each generated result is checked against the transcript snapshot used for that
request. The live transcript can continue growing while notes are prepared, so
the current document may lag behind the latest speech. Sources describe that
snapshot; use **Update notes** after changing saved transcript content.

## Longer conversations

Long inputs are processed as bounded sections. Whole turns are preserved when
they fit; longer turns are split at sentence boundaries. No words are silently
discarded to fit a model window. Very long sentences or conversations that exceed
the total limit produce a clear error and leave the source unchanged.

Later sections can amend or withdraw earlier proposals. The model processes
sections separately, so a document can contain earlier proposals that need review
against later speech. It does not establish a final consensus across a meeting.

Text, source excerpts, and generated drafts are processed locally. The only
network requests needed for this feature download the fixed public model and
runtime. No transcript is sent with those requests. Cancelling an active model
request stops its process; a successful request can reuse the loaded model until
the idle timeout. Closing Articulate stops its owned model processes.

First use on a graphics card can take longer while its driver prepares compute
kernels. Later requests reuse the driver cache. Performance also depends on
transcript length and other applications using the graphics card.

## Finding saved information

Saved-content search matches a word or phrase against retained dictations, call
transcripts, titles, and personal notes. It returns exact excerpts and opens the
source session. It is case-insensitive text search, not a generated answer or a
semantic similarity claim, and does not need a model download.

Search runs off the interface thread. Each scan considers at most 500 sessions,
32 MiB of text, and 50 matches. Partial results are explicitly identified; narrow
the query when the limit is reached. Deleted sessions are not retained in a
separate search index.
