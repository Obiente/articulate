# Articulate

A Windows dictation app with local speech recognition, written in Rust with a native C/C++ inference engine. No subscription, account, remote inference, or telemetry.

## Run

The native interface uses Inter, Phosphor SVG icons, a path-based Articulate mark and native vertex-color gradients. No raster mockup is used as application UI.

Build once with Rust and Visual Studio C++ Build Tools installed:

```powershell
.\scripts\build-windows.ps1
.\Start.cmd
```

The build script needs GitHub CLI (`gh`) and downloads a pinned, SHA-256 verified CPU/Vulkan runtime. It constructs an MSVC import library from the official DLL's exports and links the matching `transcribe-cpp` 0.2.3 bindings. DLLs must stay beside the executable. The runtime selects a compatible CPU implementation; GPU acceleration uses Vulkan if available.

In Settings, choose **Download recommended model**, or enter a compatible GGUF model path and choose **Load local model**. Model downloads require internet; the optional Discord connector communicates only with the local Discord client. Once a model is installed, dictation works offline. Models and preferences retain the existing `%LOCALAPPDATA%\TranscribeLocal` location so upgrading to Articulate preserves your local library. The executable keeps its existing `transcribe-local.exe` name for launch-script compatibility.

Choose a microphone in Settings, click **Start dictation**, and speak. Provisional text appears while you speak and is revised with additional context. Dictation continues until you stop it, committing sections near quiet boundaries about every 18 to 20 seconds. Click **Finish dictation** to check the remaining audio. Edit or copy the result. **Ctrl+Alt+Space** toggles recording by default. Change it in **Settings > Change keyboard shortcut** using Ctrl, Alt, Shift, Win and a supported key. Apply the change without restarting; if Windows rejects it, the previous binding remains active. Choose **Type live** in Dictate (or insertion on finish in Settings), click an editable field in another app, and use the shortcut to start and finish. A small, non-focusing indicator shows recording and completion. With **Type into the app while I speak** enabled, provisional text appears directly in supported fields and is revised as speech recognition gains context. Only the changed suffix of the current dictation is replaced. The final pass revises the same text instead of appending a duplicate. Moving the caret, changing the selection, editing the field, or switching fields pauses live typing; dictation continues inside Articulate. Fields without a usable TextPattern receive text after you finish.

## Learning corrections

**Remember spelling corrections I make** is enabled by default and can be disabled in Settings. After dictation, edit the transcript here or edit the inserted text in a supported Windows text field. After two seconds without another change, one short word/phrase replacement is saved to the local dictionary. A visible notice offers **Undo learning**; entries can also be edited, disabled, or removed in **Vocabulary**. Refining an existing replacement updates that rule, and Undo restores its previous value without changing the user's edited text.

External observation is scoped to the exact field and inserted span. It stops on focus loss, a new dictation, an unsupported edit, or after two minutes. Surrounding text is used in memory to identify that span and is never saved. Fields exceeding 16,000 UTF-8 bytes are excluded from learning. The worker uses Windows Value/Text accessibility patterns; app support varies. It excludes password fields, additions, deletions, punctuation-only edits, numbers, protected negations/function words, separated substitutions and larger rewrites. Spoken mentions such as `at ExampleHandle` corrected to `@ExampleHandle` are recognized as an explicit exception, saving the whole mention phrase rather than replacing every occurrence of `at`. Matching is whole-word and exact-case by default; each entry can optionally match any capitalization. Learned spoken mentions default to any capitalization, so a sentence starting with At is also recognized. Older mention rules without an explicit capitalization setting are migrated; explicit choices are preserved. Small rewrites can resemble spelling corrections, so review learned entries and use Undo when needed. This is local text correction, not voice training or improved acoustic recognition.

The worker checks field identity roughly every 300 ms, with 500 ms UI Automation provider timeouts. There is no global keystroke logger or background scan of other fields. Live field updates use verified TextPattern selections and Unicode input on the worker. At most one preview update is in flight and one latest update is queued. Unsupported fields fall back to insertion on finish. Field snapshots are bounded to 16,000 UTF-8 bytes; larger fields pause live typing without limiting local recording duration. Discarding a recording leaves text already typed into another app intact.

App edits are learned for that executable, for example `discord.exe`. Entries learned in the built-in transcript remain global unless the transcript originated in another app. Existing dictionaries keep their global behavior. You can add comma-separated cues, such as `crate, compiler`, to require a whole-word cue in the same sentence within 120 characters of a replacement. App and cue conditions both apply. App-specific rules take priority over otherwise equal global rules. This uses the dictated sentence and executable basename, not surrounding documents, browser history, or a semantic correction model. Browser sites share the browser executable's scope.

## Voice macros

Open **Shortcuts**, enter a trigger and expansion, try the preview, then save. Say the word **bang** followed by your trigger as a separate dictation:

- Trigger `signature`, expansion `Thanks,` followed by a line break and your name: say “bang signature”.
- Trigger `quick reply`, expansion `Thanks for the update. {text}`: say “bang quick reply I will check tomorrow.”

The single `{text}` slot includes words after the trigger. Macros can be global or app-specific, edited, disabled or removed. Fixed snippets require the whole utterance to match; mentioning a trigger inside ordinary prose does not expand it. Matching ignores ASR punctuation and capitalization around the trigger. Candidates beginning with bang are held from live insertion until the final result; an unknown command remains ordinary text. Literal expansion text is not passed through correction rules or automatically relearned. Missing template arguments produce a message instead of automatic insertion. No code execution, URL opening or Enter key is involved. Multiline insertion depends on the target field's Unicode newline support; **Copy text** remains available.

In **Settings → Move your corrections and macros**, copy a portable JSON library for backup or manual transfer. Import validates the entire library before merging; same-key entries are updated and unrelated entries retained. Audio, transcripts and model paths are not included. This is manual transfer, not automatic synchronization or a mobile app.

## Calls

Open **Calls**, select your microphone and the output your call uses, then **Start call capture**. Windows WASAPI loopback captures all audio played on that output. There is no fixed call duration limit. The app processes short windows and releases audio as it goes; only transcript text and bounded speaker reference clips remain in memory. Use headphones and a microphone source that does not already mix in system audio. The app does not perform echo cancellation.

The microphone track is labelled **You**. Calls use the same loaded speech model as dictation, by default Qwen3-ASR 1.7B Q8_0. A separate local NVIDIA Sortformer F16 model (237 MB) supplies anonymous **Speaker 1** through **Speaker 4** labels; it does not produce transcript words. Rename acoustic speakers in the call view, or connect Discord for activity-based names. Names are saved with that session and are not reused as a voice identity in later calls. Up to four distinct acoustic remote voices are supported. Overlaps are explicitly labelled with multiple speakers; the ASR hears mixed output audio, so it cannot guarantee recovery of every overlapping word. Uncertain matches stay unassigned.

The pinned native diarizer exposes offline windows, not a usable persistent public streaming API. This app prepends a bounded clean voice reference for each established speaker and remaps each new window by reference overlap. This is best-effort continuity, not guaranteed stable identity. The trailing transcript refreshes after roughly two seconds of new audio plus inference time. A bounded draft stays revisable until both tracks are quiet or the 24-second context cap is reached. A call itself has no duration cap. Slower CPUs combine pending audio into the next revision. Brief diarizer changes are grouped into contiguous speech blocks so ASR receives more context; mixed speaker blocks remain uncertain instead of assigning generated words to guessed timestamps. Short replies are retained. Window boundaries can still affect words and punctuation. No automatic text cleanup is applied to call transcripts. Consecutive sections from the same identified speaker share one label. **Copy transcript** copies the grouped, timestamped transcript, and **History** retains saved sessions locally.

**Create notes** selects up to six verbatim highlights and eight possible action excerpts from the current transcript, with speaker labels and source time ranges. Selection uses local word frequency and English decision/action cues. These are extractive notes, not a generative summary, and possible actions need review. Existing notes refresh with transcript revisions; speaker renames are reflected immediately. Notes are saved with their transcript for later review in **History**.

## Saved sessions

**History** stores dictations, call transcripts, speaker labels, and generated notes on this device. Completed dictations retain the recognized original and edited text. Call text saves as new sections arrive; notes and naming updates belong to the same session. Open a saved entry for review without replacing an active recording, search titles and previews, rename it, copy or export its contents, or delete it. Sessions remain until deleted.

Storage is local plaintext JSON under `%LOCALAPPDATA%\TranscribeLocal\history`. No call audio is saved and no cloud sync is used. Disk work runs on a background worker. Atomic replacement and recovery protect the preceding saved version if a write is interrupted; failed saves are reported in the app. This cannot recover speech that has not yet been transcribed or transcripts from older app versions that were never saved. Export any needed text from an older running version before closing it to upgrade.

Sources: [native diarizer documentation](https://github.com/handy-computer/transcribe.cpp/blob/v0.2.3/docs/models/diar_streaming_sortformer_4spk-v2.1.md), [NVIDIA model card](https://huggingface.co/nvidia/diar_streaming_sortformer_4spk-v2.1). The F16 file is pinned to revision `ae4afbb5c3d33b71cf2dbf600022b655ee706dd0`, SHA-256 `62faec7b99ad23e323087597604b50728abe85089b6364970b019a845547bf99`.

## Accuracy choices

- Default: **Qwen3-ASR 1.7B Q8_0**, a 2.19 GB download. This uses the larger Qwen3-ASR model and conservative 8-bit quantization rather than a smaller or heavily compressed default. It is a starting candidate, not a claim of universal superiority.
- Automatic language detection, source-language transcription, and the model's native decoding defaults. This native Qwen implementation does not expose language hints, contextual vocabulary prompts, timestamps, or streaming.
- Live preview starts after 1.2 seconds and requests another pass after at least 0.8 seconds of new audio. There is only one request in flight; slower CPUs naturally update less often. This repeatedly decodes the active window, not the entire growing recording. Stop cancels provisional inference, preserves any in-flight section commit, and checks the tail. Generation IDs prevent discarded or cancelled results from replacing a newer recording.
- Windows font fallbacks render CJK, Korean, Cyrillic and other scripts missing from egui's default fonts. Fonts are read from the local Windows installation, not downloaded or redistributed. This fixes missing glyph rendering, not incorrect language recognition or hallucinated text.
- Optional conservative English cleanup runs on each preview and the final text. It handles limited function-word repetitions, fragments such as `pro- product`, and explicit single-token day/month/number corrections such as `Tuesday, sorry, Thursday`. It preserves negation, emphasis, and ambiguous phrases. It does not repair arbitrary homophones, missing words, grammar, or paragraph-level restarts. Original ASR text remains available and changes can be undone. No second generative model is used. The ASR itself can still normalize punctuation, numbers, and wording.
- Band-limited conversion to 16 kHz avoids aliasing. Capture uses a preallocated ring buffer without locks, allocation, filesystem access, or inference on the audio callback.
- The model stays loaded between recordings. Inference runs on a worker thread. Cancelled, failed, or truncated outputs are never automatically inserted.
- Dictionary corrections are explicit whole-word/phrase replacements, optionally ignoring case. Longest matches win without cascading replacements. Changes can be undone. With **Remember spelling corrections I make** enabled, short stable edits can create local dictionary entries with a visible Undo action. This does not train the speech model.

Sources: [upstream model card](https://huggingface.co/Qwen/Qwen3-ASR-1.7B), [native model implementation and quantization measurements](https://github.com/handy-computer/transcribe.cpp/blob/v0.2.3/docs/models/qwen3-asr-1.7b.md), [GGUF conversion](https://huggingface.co/handy-computer/Qwen3-ASR-1.7B-gguf).

The recommended model is pinned to repository revision `3555bd238a8572bbace3ebf60d23b036dc0a5dbe`, SHA-256 `9a0d81792dfea2d5f278b8a63deb3ea6e02139ce42c2301f32ea19c4f77526b7`. The download is not accepted until both its byte count and hash match.

## Development and evaluation

Use the [offline accuracy evaluator](../scripts/accuracy.md) with reference transcripts to measure word error rate and CPU/GPU inference speed. Its synthetic tests verify scoring and invocation, not recognition quality.

```powershell
.\scripts\build-windows.ps1 -Test
.\scripts\build-windows.ps1 -Check
cargo fmt --all -- --check
.\target\release\transcribe-local.exe --devices
.\target\release\transcribe-local.exe --transcribe sample.wav --repeat 3
.\target\release\transcribe-local.exe --transcribe sample.wav --cpu --repeat 3
.\target\release\transcribe-local.exe --transcribe sample.wav --model alternate.gguf
.\target\release\transcribe-local.exe --transcribe sample.wav --live
.\target\release\transcribe-local.exe --call-file multi-speaker.wav
.\target\release\transcribe-local.exe --capture-check
```

The CLI emits JSON with text, actual backend, audio duration, model-load time, and inference time. Model load (including GPU warmup) and warm inference are reported separately. The first GPU setup may take tens of seconds while shaders compile. `--live` replays growing audio prefixes as fast as inference permits, for inspecting provisional revisions; it is not a wall-clock latency benchmark. WAV import supports integer PCM and float audio, with resampling and mono mixing. Use actual speaker recordings to compare word error rate and errors in names, numbers, dates, and negations. Synthetic speech and public samples are smoke tests, not evidence of performance on a particular person's voice.

The optional real-engine contract test uses a locally installed model and supplied WAV, with no network:

```powershell
$env:TRANSCRIBE_DIR = Join-Path $PWD '.local\native\install'
$env:TRANSCRIBE_TEST_WAV = 'sample.wav'
cargo test --release --features dynamic-backends -- --ignored
```

An alternative source-build path is `cargo build --release` for a static CPU build, or `cargo build --release --features vulkan` with the Vulkan SDK and a working CMake C++ environment. The supplied Windows script avoids the native CMake build by using the verified upstream runtime. Building and testing the static/source configurations on a hardware matrix remains separate work.

## Current boundaries

This release focuses on Windows dictation and call transcripts.

- Windows first. This is an initial working dictation application, not feature parity with Wispr Flow.
- Microphone selection is available. No noise suppression, voice-activity endpointing, or background recording overlay yet.
- Dictation and call capture have no fixed duration cap. Audio buffers are bounded. If inference or capture cannot keep up, recording stops with an explicit error instead of silently discarding speech. Long WAV imports are held in memory and decoded in windows.
- Meeting notes select verbatim highlights and possible action excerpts. No generative summaries, automatic meeting detection, mobile keyboard, or automatic dictionary sync yet.
- Corrections can be added manually or learned from edits. Model-level vocabulary biasing remains future work.
- Automatic insertion uses Unicode input packets and checks both native focus and the exact UI Automation element on a dedicated COM worker. It leaves the clipboard alone and never sends Enter. Password, read-only and unsupported fields are excluded. Moving to another field stops tracking. Elevated/protected apps and editors with incomplete accessibility support may require Copy text.
- **Copy text** deliberately changes the clipboard and requests exclusion from Windows clipboard history/cloud sync. This does not prevent third-party clipboard utilities or the destination app from reading copied text.
- Audio stays in process memory and is not saved by session history. Transcripts, notes, and session display names are saved locally and are not included in diagnostic logs. OS paging/crash dumps, imported source files, clipboard consumers, and text inserted into other apps are outside that boundary.
- Settings, dictionary entries and macros are local plaintext JSON. There is no background account or sync service.
- Accuracy and latency across accents, microphones, languages and CPUs require a larger evaluation corpus. No guarantee of perfect transcription.

## Layout

`audio.rs` handles capture and resampling; `engine.rs` owns the native model session; `cleanup.rs` handles conservative disfluency rules; `dictionary.rs` applies explicit corrections; `platform.rs` handles Windows hotkeys, insertion and clipboard flags; `model.rs` contains the isolated download path; `app.rs` is the native egui interface and preview/final-pass coordination.

Local recordings, diagnostics, builds, models and captures belong in ignored `.local/` or outside the repository. Do not commit real user speech, transcripts or machine-specific paths.

## Licenses

Application: MIT. Qwen3-ASR model: Apache-2.0. Sortformer model: NVIDIA Open Model License. `transcribe.cpp` and `ggml`: MIT. The build copies the native runtime's supplied license notices beside the binary. Rust dependencies retain their respective licenses. Model weights are downloaded separately, not included in this repository.

## Writing styles and call exports

Calls opens on a full-height transcript workspace with readable speaker blocks and selectable text. Recording controls, connection status, and export actions remain at the top. Search stays above the transcript; scrolling up pauses following until you return to the bottom. **Notes** and **Setup** have separate tabs so devices, speaker names, and notes do not squeeze the conversation out of view. Connect or disconnect Discord from **Setup > Discord speakers**, including during capture. A new connection supplies names for subsequent speech; completed transcript labels are preserved.

In Settings, open **Writing style by app** to choose No cleanup, Clean, or Casual chat for an executable such as discord.exe. The last-used app can fill this field. No cleanup preserves recognition output before your dictionary and explicit snippets. Casual chat conservatively removes the final period on short simple messages. No wording or tone is generated, and the original remains available.

In Calls, search words or speaker names. **Export** saves or copies plain text, Markdown, SRT, or WebVTT using captured section timestamps and current speaker labels. Overlap remains explicit. Grouped sections do not provide word-level subtitle timing. Saving uses a local file dialog; exports do not upload content.

See [Discord connection setup and boundaries](discord-integration.md). In Calls, connect Discord to use current participant names when speaking activity sufficiently matches the captured audio. **Relaunch Discord** opens a confirmation, then restarts the installed stable client with local debugging and connects automatically. Relaunching disconnects any active Discord call; rejoin afterward. The manual launch command remains available. This optional connection does not use an OAuth application, StreamKit credentials, or a modified client. Uncertain sections retain acoustic labels. Discord's internal interfaces can change between releases.
