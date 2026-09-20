<p align="center"><img src="assets/brand/banner.svg" alt="Articulate. Your words, ready to use." width="100%"></p>

<p align="center"><a href="https://github.com/Obiente/articulate/releases/latest">Download for Windows</a> · <a href="docs/getting-started.md">Get started</a> · <a href="https://github.com/Obiente/articulate/issues">Feedback</a> · <a href="docs/development.md">Developer guide</a></p>

Articulate turns speech into text wherever you write, helps you remember conversations, and learns the spellings you correct. Recognition runs on your computer. No account, subscription, telemetry, or cloud transcription.

Built with Rust, a native egui interface, and a local C/C++ inference runtime. Windows first, including CPU-only PCs.

## Make room for your voice

- **Dictate into your apps.** A customizable shortcut starts recording. Supported text fields update as you speak, with a final revision when you finish.
- **Teach it your vocabulary.** Correct a short word or phrase, review the learning notice, and undo it if needed. Vocabulary can be scoped to an app or nearby words.
- **Say your shortcuts.** Say “bang signature” to expand a snippet, or use a template with a spoken `{text}` argument.
- **Keep the conversation.** Capture your microphone and call output, review speaker-labelled transcripts, and create quoted highlights and possible actions.
- **Return to it later.** Local History keeps transcripts, originals, notes, and speaker names. Search, rename, copy, export, or delete a session.

## Install

Download the **Windows x86_64 setup** from [Releases](https://github.com/Obiente/articulate/releases/latest). A portable ZIP is also available. The installer runs for your user without administrator access.

Open **Settings → Download recommended model**. Qwen3-ASR 1.7B Q8_0 is a separate download of about **2.19 GB**. Calls additionally use a **237 MB** speaker model. **No model weights are included in either release package.** Downloads are size- and SHA-256-verified before use.

A recent Windows 10/11 PC with **16 GB RAM** is recommended as a starting point. CPU-only operation is supported; compatible Vulkan GPUs can accelerate inference. Performance depends on hardware and speech. A broader hardware compatibility matrix is still in progress.

Press **Ctrl+Alt+Space** to start and finish dictation. Change the shortcut in Settings. [Full setup instructions →](docs/getting-started.md)

## Discord

The optional [Vencord companion](docs/vencord.md) sends current voice participants and speaking activity through a paired local connection. The debugger-based connection remains available for the standard client.

**Speaker activity is metadata, not isolated audio.** Calls currently transcribe microphone and mixed output audio. Neither connection separates overlapping participants into individual audio tracks. Attribution is conservative and uncertain speech remains unassigned. [Connection details →](docs/discord-integration.md)

## Updates and privacy

Use **Settings → App updates** to check for a release. Updates are downloaded only on request, verified against the GitHub asset's SHA-256 digest, and installed after recording has finished and pending history saves have completed. Models and saved sessions survive updates.

Speech runs locally after models are downloaded. Model downloads contact Hugging Face; explicit update checks and downloads contact GitHub. The Discord companion uses loopback. Text is never sent for transcription or correction. Saved text, notes, preferences and models live under `%LOCALAPPDATA%\TranscribeLocal`; audio is not saved. Uninstalling keeps this local data.

The initial Windows build is **unsigned**, so Windows may show an unknown-publisher warning. Obtain binaries from this repository's Releases page.

## Current limits

Recognition can make mistakes, particularly with noise, names, overlap, and short utterances. Call text is revisable within bounded context windows; continuous speech still needs periodic commits. Notes quote highlights and possible actions rather than generating a narrative summary. App insertion depends on Windows accessibility support. macOS, mobile keyboards, automatic sync, and participant-isolated audio are not included.

## Contribute

See [CONTRIBUTING.md](CONTRIBUTING.md) for local builds and checks, and [SECURITY.md](SECURITY.md) for private vulnerability reports. Please use synthetic or non-sensitive examples in public issues.

Articulate is licensed under **AGPL-3.0-or-later**, following [Obiente's open-source commitment](https://github.com/Obiente/.github/blob/main/profile/README.md). You may use, modify, and redistribute it under those terms; it comes without warranty. Bundled fonts, icons, runtimes and Rust dependencies retain their own notices. Qwen model weights are Apache-2.0; Sortformer weights use the NVIDIA Open Model License. Weights are downloaded separately. See [LICENSE](LICENSE), [visual asset notices](assets/LICENSES.md), and the `licenses/` directory inside each binary distribution.
