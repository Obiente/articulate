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
- **Review context with Assort.** Included pretrained classifiers suggest meeting highlights and review saved vocabulary choices against the sentence and destination app. Choose **Create notes** or **Review vocabulary** without configuring model paths, then review the suggestions before applying them.

## Install

Download the **Windows x86_64 setup** from [Releases](https://github.com/Obiente/articulate/releases/latest). A portable ZIP is also available. The installer runs for your user without administrator access.

Open **Settings → Audio & models**. Qwen3-ASR 1.7B Q8_0 is a separate download of about **2.19 GB**. Calls can additionally use a **237 MB** acoustic speaker model when Discord activity is unavailable. These large speech models are not bundled; downloads are size- and SHA-256-verified before use. The small pretrained Assort notes and vocabulary classifiers are included in the application and work without a download or manual model setup.

Assort's included models use authored synthetic English training examples. They score source passages or existing vocabulary choices; they do not provide general language understanding or generate new prose. Review their suggestions, especially for unfamiliar names, ambiguous wording, and other languages. [How classification works →](docs/classification.md)

A recent Windows 10/11 PC with **16 GB RAM** is recommended as a starting point. CPU-only operation is supported; compatible Vulkan GPUs can accelerate inference. Performance depends on hardware and speech. A broader hardware compatibility matrix is still in progress.

Press **Ctrl+Alt+Space** to start and finish dictation. Change the shortcut in Settings. [Full setup instructions →](docs/getting-started.md)

## Discord

The optional [Vencord companion](docs/vencord.md) sends current voice participants, avatar identifiers and speaking activity through a paired local connection. Articulate can prepare and update its custom Vencord build. The debugger-based connection remains available for the standard client.

The custom companion includes a [native receive adapter](plugins/discord-native/README.md) for separate participant audio by default. Its synthetic tests pass; real desktop-call validation is still pending. On unsupported clients, mixed output capture can use fresh Discord activity for names without acoustic classification, but cannot reliably separate overlapping voices. The interface distinguishes these audio sources. After pairing, Discord connects automatically; an optional setting starts and finishes transcripts on confirmed call joins and leaves. [Connection details →](docs/discord-integration.md)

## Updates and privacy

Use **Settings → App updates** to check for a release. Updates are downloaded only on request, verified against the GitHub asset's SHA-256 digest, and installed after recording has finished and pending history saves have completed. Models and saved sessions survive updates.

Speech runs locally after models are downloaded. Model downloads contact Hugging Face; explicit update checks and downloads contact GitHub. The Discord companion uses loopback. Participant pictures are fetched from Discord's CDN and cached locally. Text is never sent for transcription or correction. Saved text, notes, preferences and models live under `%LOCALAPPDATA%\TranscribeLocal`; audio is not saved. Uninstalling keeps this local data.

The initial Windows build is **unsigned**, so Windows may show an unknown-publisher warning. Obtain binaries from this repository's Releases page.

## Current limits

Recognition can make mistakes, particularly with noise, names, overlap, and short utterances. Call text is revisable within bounded context windows; continuous speech still needs periodic commits. Notes quote highlights and possible actions rather than generating a narrative summary. App insertion depends on Windows accessibility support. Native participant audio remains experimental. macOS, mobile keyboards and automatic sync are not yet implemented.

## Contribute

See [CONTRIBUTING.md](CONTRIBUTING.md) for local builds and checks, and [SECURITY.md](SECURITY.md) for private vulnerability reports. Please use synthetic or non-sensitive examples in public issues.

Articulate is licensed under **AGPL-3.0-or-later**, following [Obiente's open-source commitment](https://github.com/Obiente/.github/blob/main/profile/README.md). You may use, modify, and redistribute it under those terms; it comes without warranty. Included Assort weights carry their own model cards and license notices. Bundled fonts, icons, runtimes and Rust dependencies retain their own notices. Separately downloaded Qwen weights are Apache-2.0; Sortformer weights use the NVIDIA Open Model License. See [LICENSE](LICENSE), [visual asset notices](assets/LICENSES.md), and the `licenses/` directory inside each binary distribution.
