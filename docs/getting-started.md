# Get started

1. Download the Windows x86_64 installer from [Releases](https://github.com/Obiente/articulate/releases/latest), or extract the entire portable ZIP.
2. Open Settings and download the recommended speech model. Initial speech-model downloads require internet. The small Assort vocabulary model is already included.
3. Choose your microphone. If GPU loading fails, select CPU only and load again.
4. Hold Ctrl+Alt+Space while speaking and release to finish. Double-press for hands-free dictation, then press once to finish. Settings lets you change the keys, choose press-to-toggle mode, or turn recording sounds off.
5. To type into another app, enable app insertion and focus its editable field before using the shortcut. Hold mode inserts after finishing, so held modifiers cannot interfere with typing. Toggle mode can update supported fields live. Moving the caret or changing fields pauses insertion while your transcript remains in Articulate.

## Notetaker

Choose **New note** in Notetaker to write without a microphone or speech model. Personal notes save automatically. Conversations have an editable Notes document alongside their transcript. [Notetaker guide](notetaker.md).

Choose **Speak your thoughts**, or press **Ctrl+Alt+N**, to dictate a personal
note. Press the shortcut again to finish. With the local notes model installed,
your words become a notes document and are filed by topic. Each recording stays
separate and can be moved between notes from the **Dictations** tab. Topic notes
combine their recordings while preserving handwritten text.

Use headphones. In Notetaker, choose **Record a conversation** to open audio setup and select your microphone. With the Vencord companion selected, wait for its separate-audio connection before starting. Each received participant stream carries its own identity, including when people speak at the same time. The native adapter remains experimental; live behavior still needs validation on supported Discord builds. If the audio connection is unavailable, Articulate asks you to connect it instead of silently switching to mixed capture.

The trailing transcript revises as more context arrives. Finish capture to process remaining audio. Open **Notes** for one editable document. Download the optional local notes model to have important information added during the call; use **Live updates** to pause and **Transcript sources** to review excerpts. Updates use committed speech, require 40 new words during capture, and wait at least 45 seconds between attempts. Remaining speech can update the document after finishing. The Notetaker hub lets you revisit saved conversations. Its active-conversation card returns to a capture without restarting it.

For other calls, use mixed output capture and choose the output device used by your call. Download the speaker model when activity-based names are unavailable. This output includes every app playing on that device and cannot reliably separate simultaneous voices. The optional [debugger connection](discord-integration.md) provides activity-based names only. See the [Vencord companion setup](vencord.md) for separate Discord audio. Relaunching Discord disconnects an active voice call, so rejoin afterward.

## Corrections and shortcuts

Choose **Edit text** beside the latest dictation, or open a saved dictation and
choose **Edit text** on its Transcript tab. Save your changes or cancel; the
original transcription stays available in the editor.

To reuse a spelling, enable **Remember a spelling for future dictations**. Enter
the heard and wanted forms, such as `a kit` and `AcmeKit`, and use **Replace all in
this dictation** when needed. Optional context words restrict future corrections
to relevant sentences. Existing matching vocabulary entries gain context instead
of becoming duplicates. General transcript edits do not automatically become
vocabulary rules. Review or change remembered spellings in Vocabulary.

Short corrections made in an external app can also be learned when **Remember
corrections** is enabled in Settings. Those learning notices offer Undo.

In Shortcuts, save a trigger such as `signature` and a text expansion. Say “bang signature” as its own dictation. A template can contain one `{text}` slot for words spoken after the trigger. Macros expand text only; they never execute commands or send Enter.

## Clean up dictation

Enable **Clean up speech** in Settings for automatic filler, repetition and
explicit spoken-correction cleanup. This fast local cleanup does not require an
additional model. Expanded vocal shortcuts keep their exact contents.

The optional writing editor can be downloaded or repaired under **Settings >
Models > Text cleanup**. Manual before/after Polish previews and per-app writing
style editing have not yet been migrated to the Tauri interface. Installing the
editor does not change the automatic cleanup into model-based rewriting.

## Transcription language

In **Settings > Audio > Transcription language**, choose **English** for English
dictation and calls. The choice is passed to the speech model for every passage,
including live revisions and spoken notes. **Auto-detect** remains available for
multilingual recordings. Changing language reloads the speech model and applies
to new audio, not old transcripts. Language selection reduces unintended language
switching; it does not guarantee that quiet sounds will never be misrecognized.

## Sound cues

Under **Settings > Audio context**, download SenseVoice (259 MB), then enable
**Include sound cues** for new calls and spoken notes. Qwen continues to supply
the transcript words; SenseVoice adds separate speaker-linked labels for laughter,
crying, coughing, sneezing and applause. The current runtime supports 64-bit Windows.

The times shown cover the analyzed passage (up to eight seconds), not an exact
event onset. Detection can miss reactions or label them incorrectly.

Analysis runs locally in a bounded background queue. If it falls behind, it skips
sound cues rather than holding up transcription; Settings shows the status. A
finished recording waits at most eight seconds for remaining cues. Earlier saved
recordings are not reanalyzed. Temporary local WAV inputs are removed after each
analysis. Downloaded model and runtime files are checksum verified.

## Updates and removal

Settings > Updates & about checks GitHub only when requested. Finish recording before installing. Pending history saves must complete before the installer opens. Retry failed saves before updating.

Updates preserve models, vocabulary, settings and History. Uninstall removes the application but keeps `%LOCALAPPDATA%\TranscribeLocal`. Remove that folder separately only if you intend to delete all local Articulate data.
