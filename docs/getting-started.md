# Get started

1. Download the Windows x86_64 installer from [Releases](https://github.com/Obiente/articulate/releases/latest), or extract the entire portable ZIP.
2. Open Settings and download the recommended speech model. Initial model downloads require internet. No weights are bundled.
3. Choose your microphone. If GPU loading fails, select CPU only and load again.
4. Start dictation, speak, then finish. Ctrl+Alt+Space does the same from another app.
5. To type into another app, enable Type live and focus its editable field before using the shortcut. Moving the caret or changing fields pauses live typing while transcription continues in Articulate.

## Calls

Use headphones. In Calls > Setup, select your microphone and the output device used by your call. Download the speaker model and start capture. The output track includes every app playing on that device. It is not one track per participant.

The trailing transcript revises as more context arrives. Finish capture to process remaining audio. Notes contains quoted highlights and possible actions. History lets you revisit saved sessions.

For Discord names, use the [Vencord companion](vencord.md) or the optional [debugger connection](discord-integration.md). These provide activity-based names; they do not separate audio. Relaunching Discord disconnects an active voice call, so rejoin afterward.

## Corrections and shortcuts

Correct a short word or phrase after dictation. A learning notice offers Undo. Review saved rules in Vocabulary. Rules can apply globally or to an app, and can require nearby words.

In Shortcuts, save a trigger such as `signature` and a text expansion. Say “bang signature” as its own dictation. A template can contain one `{text}` slot for words spoken after the trigger. Macros expand text only; they never execute commands or send Enter.

## Updates and removal

Settings > App updates checks GitHub only when requested. Finish recording before installing. Pending history saves must complete before the installer opens. Retry failed saves before updating.

Updates preserve models, vocabulary, settings and History. Uninstall removes the application but keeps `%LOCALAPPDATA%\TranscribeLocal`. Remove that folder separately only if you intend to delete all local Articulate data.
