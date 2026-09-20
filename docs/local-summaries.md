# Local summaries

Articulate can prepare a generated summary of a completed transcript on this
computer. Each point is shown with source excerpts for review. Applying a summary
does not change the transcript or the existing source-quoted Highlights.

The optional model is **Qwen3.5-4B, Q5_K_M**, an Unsloth GGUF conversion of
[Qwen's Apache-2.0 model](https://huggingface.co/Qwen/Qwen3.5-4B).
The [conversion](https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/tree/e87f176479d0855a907a41277aca2f8ee7a09523)
is pinned to a specific revision and SHA-256. It is a separate download, around
3.14 GB, and is not included in the installer. The managed llama.cpp runtime runs
on the CPU, uses at most eight inference threads, and communicates only through
an authenticated loopback connection. A PC with at least 16 GB of RAM is the
intended starting point; available memory and other running models affect it.

## Reviewing a summary

Generated facts, decisions, and actions are suggestions. Checking a citation
proves which text was referenced; it cannot prove that the model interpreted it
correctly. Review the wording, including names, negations, conditions, owners,
and deadlines, against the displayed excerpts before using a point.
Category labels also need review: the model can describe a decision accurately
while labeling it as a fact. They are not a verified task or decision register.

The application supplies each quotation directly from the transcript rather
than trusting the model to reproduce it. Times always refer to the original
speaker turn. A sentence excerpt does not receive an invented word-level time.
Changing the transcript, speaker attribution, or relevant source metadata
invalidates the draft and requires regeneration. Renaming the saved session
does not change its source content or invalidate the draft.

## Longer conversations

Long inputs are processed as bounded sections. Whole turns are preserved when
they fit; longer turns are split at sentence boundaries. No words are silently
discarded to fit a model window. Very long sentences or conversations that exceed
the total limit produce a clear error and leave the source unchanged.

For multi-section results, points retain their section number. Later sections
can amend or withdraw earlier proposals. These results are section summaries,
not a claim that the software has established a final consensus across a meeting.
The review screen should keep this limitation visible.

Text, source excerpts, and generated drafts are processed locally. The only
network requests needed for this feature download the fixed public model and
runtime. No transcript is sent with those requests. Closing or cancelling a
summary stops its owned model process.

## Finding saved information

Saved-content search matches a word or phrase against retained dictations, call
transcripts, titles, and personal notes. It returns exact excerpts and opens the
source session. It is case-insensitive text search, not a generated answer or a
semantic similarity claim, and does not need a model download.

Search runs off the interface thread. Each scan considers at most 500 sessions,
32 MiB of text, and 50 matches. Partial results are explicitly identified; narrow
the query when the limit is reached. Deleted sessions are not retained in a
separate search index.
