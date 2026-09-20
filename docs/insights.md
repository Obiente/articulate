# Insights

Insights reads the dictations retained in local History. It never sends usage statistics to a server. Calls and personal notes are excluded.

- **Dictation words:** the final recognized word count before text cleanup, shortcut expansion or manual editing. Older sessions without that count use saved text.
- **Dictation pace:** recognized words divided by captured microphone time, including pauses. Imported files and old sessions without timing are excluded. The overall pace is weighted by recording time.
- **Cleanup edits:** recorded vocabulary replacements plus deterministic cleanup rule applications. This is not an accuracy score or a count of every changed word. Old sessions without these measurements are excluded from correction totals.
- **App usage:** words by the target app's process name. Window titles and document paths are not stored for this metric.
- **Activity:** the last 28 calendar days in the computer's local time zone. The current streak ends today or yesterday; a gap resets it. The interface identifies a UTC fallback if local time is unavailable.

Editing or saving a dictation again does not count it twice. Deleting it removes it from Insights on refresh. Choose Refresh for the latest retained history. This page does not estimate time saved or compare you with other people.
