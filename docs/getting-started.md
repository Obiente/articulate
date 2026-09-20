# Get started

1. Download the Windows x86_64 installer from [Releases](https://github.com/Obiente/articulate/releases/latest), or extract the entire portable ZIP.
2. Open Settings and download the recommended speech model. Initial speech-model downloads require internet. The small Assort notes and vocabulary models are already included.
3. Choose your microphone. If GPU loading fails, select CPU only and load again.
4. Hold Ctrl+Alt+Space while speaking and release to finish. Double-press for hands-free dictation, then press once to finish. Settings lets you change the keys, choose press-to-toggle mode, or turn recording sounds off.
5. To type into another app, enable app insertion and focus its editable field before using the shortcut. Hold mode inserts after finishing, so held modifiers cannot interfere with typing. Toggle mode can update supported fields live. Moving the caret or changing fields pauses insertion while your transcript remains in Articulate.

## Calls

Use headphones. In Calls > Setup, select your microphone. With the Vencord companion selected, wait for its separate-audio connection before starting. Each received participant stream carries its own identity, including when people speak at the same time. The native adapter remains experimental; live behavior still needs validation on supported Discord builds. If the audio connection is unavailable, Articulate asks you to connect it instead of silently switching to mixed capture.

The trailing transcript revises as more context arrives. Finish capture to process remaining audio. Notes contains quoted highlights and possible actions. History lets you revisit saved sessions.

For other calls, use mixed output capture and choose the output device used by your call. Download the speaker model when activity-based names are unavailable. This output includes every app playing on that device and cannot reliably separate simultaneous voices. The optional [debugger connection](discord-integration.md) provides activity-based names only. See the [Vencord companion setup](vencord.md) for separate Discord audio. Relaunching Discord disconnects an active voice call, so rejoin afterward.

## Corrections and shortcuts

Correct a short word or phrase after dictation. A learning notice offers Undo. Review saved rules in Vocabulary. Rules can apply globally or to an app, and can require nearby words.

In Shortcuts, save a trigger such as `signature` and a text expansion. Say “bang signature” as its own dictation. A template can contain one `{text}` slot for words spoken after the trigger. Macros expand text only; they never execute commands or send Enter.

## Updates and removal

Settings > Updates & about checks GitHub only when requested. Finish recording before installing. Pending history saves must complete before the installer opens. Retry failed saves before updating.

Updates preserve models, vocabulary, settings and History. Uninstall removes the application but keeps `%LOCALAPPDATA%\TranscribeLocal`. Remove that folder separately only if you intend to delete all local Articulate data.
