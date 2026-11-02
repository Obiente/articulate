//! Portable transcripts, using the captured row ranges without inventing
//! word-level timings. Overlapping rows remain overlapping subtitle cues.
use crate::{
    calls::{self, Row},
    history::{Kind, Session},
};
use std::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Text,
    Markdown,
    Srt,
    WebVtt,
}

/// Export a snapshot. Empty rows are omitted; subtitle formats additionally
/// omit invalid/zero-duration ranges because they cannot form valid cues.
pub fn export(rows: &[Row], names: &[String], format: Format) -> String {
    export_with(rows, names, &[], format)
}

/// Write one interruption marker where the format allows it. SubRip has no
/// comment syntax, so markers are left out rather than shown as captions.
fn interruption(result: &mut String, gap: &calls::Interruption, format: Format) {
    let message = single_line(&gap.message());
    match format {
        Format::Text => {
            let _ = writeln!(result, "[{message}]\n");
        }
        Format::Markdown => {
            let _ = writeln!(result, "> *{}*\n", markdown(&message));
        }
        // A NOTE block may sit between cues and cannot contain "-->".
        Format::WebVtt => {
            let _ = writeln!(result, "NOTE {}\n", message.replace("-->", "->"));
        }
        Format::Srt => {}
    }
}

/// Export a snapshot with timestamped audio interruptions between turns.
pub fn export_with(
    rows: &[Row],
    names: &[String],
    interruptions: &[calls::Interruption],
    format: Format,
) -> String {
    let mut gaps: Vec<&calls::Interruption> = interruptions.iter().collect();
    gaps.sort_by_key(|gap| gap.start_ms);
    let mut gaps = gaps.into_iter().peekable();
    let mut rows: Vec<_> = rows
        .iter()
        .filter(|r| !r.text.trim().is_empty() || !r.cues.is_empty())
        .filter(|r| !matches!(format, Format::Srt | Format::WebVtt) || r.end_ms > r.start_ms)
        .collect();
    rows.sort_by_key(|r| r.start_ms);
    let mut result = match format {
        Format::Markdown => "# Call transcript\n\n".to_owned(),
        Format::WebVtt => "WEBVTT\n\n".to_owned(),
        _ => String::new(),
    };
    for (index, row) in rows.iter().enumerate() {
        while let Some(gap) = gaps.next_if(|gap| gap.start_ms <= row.start_ms) {
            interruption(&mut result, gap, format);
        }
        let label = calls::label(row, names);
        let label = if label.trim().is_empty() {
            "Uncertain speaker".to_owned()
        } else {
            single_line(&label)
        };
        let text = calls::display_text(row);
        match format {
            Format::Text => {
                let _ = writeln!(
                    result,
                    "[{} to {}] {}\n{}\n",
                    timestamp(row.start_ms, '.'),
                    timestamp(row.end_ms, '.'),
                    label,
                    text.trim()
                );
            }
            Format::Markdown => {
                let _ = writeln!(
                    result,
                    "**{}** · {} to {}\n\n{}\n",
                    markdown(&label),
                    timestamp(row.start_ms, '.'),
                    timestamp(row.end_ms, '.'),
                    markdown(text.trim())
                );
            }
            Format::Srt | Format::WebVtt => {
                let separator = if format == Format::Srt { ',' } else { '.' };
                let _ = writeln!(
                    result,
                    "{}\n{} --> {}\n{}: {}\n",
                    index + 1,
                    timestamp(row.start_ms, separator),
                    timestamp(row.end_ms, separator),
                    subtitle(&label),
                    subtitle(&single_line(&text))
                );
            }
        }
    }
    for gap in gaps {
        interruption(&mut result, gap, format);
    }
    result
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Text => "txt",
            Self::Markdown => "md",
            Self::Srt => "srt",
            Self::WebVtt => "vtt",
        }
    }

    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "txt" => Some(Self::Text),
            "md" => Some(Self::Markdown),
            "srt" => Some(Self::Srt),
            "vtt" => Some(Self::WebVtt),
            _ => None,
        }
    }
}

/// Export any saved session: calls and spoken notes by turn, dictations and
/// written notes as one passage. Markdown and text include the title, date,
/// participants, personal notes and generated notes. Subtitles need timing: a
/// dictation becomes one cue spanning its recorded audio, and an item with no
/// recorded timing cannot be exported as subtitles.
pub fn session(session: &Session, format: Format) -> anyhow::Result<String> {
    let rows: Vec<Row> = if session.rows.is_empty() {
        let text = session.text.trim();
        let duration = session.metrics.audio_duration_ms.unwrap_or(0);
        if matches!(format, Format::Srt | Format::WebVtt) {
            anyhow::ensure!(
                !text.is_empty(),
                "This item has no transcript to export as subtitles."
            );
            anyhow::ensure!(
                duration > 0,
                "This item has no recorded timing. Export it as text or Markdown instead."
            );
        }
        if text.is_empty() {
            Vec::new()
        } else {
            vec![Row {
                cues: Vec::new(),
                start_ms: 0,
                end_ms: duration,
                microphone: true,
                speakers: Vec::new(),
                discord: None,
                text: text.to_owned(),
            }]
        }
    } else {
        session.rows.clone()
    };
    if matches!(format, Format::Srt | Format::WebVtt) {
        return Ok(export_with(
            &rows,
            &session.speaker_names,
            &session.interruptions,
            format,
        ));
    }
    let markdown_format = format == Format::Markdown;
    let heading = |text: &str, level: &str| {
        if markdown_format {
            format!("{level} {text}")
        } else {
            text.to_owned()
        }
    };
    let mut result = String::new();
    let title = single_line(&session.title);
    let title = if title.is_empty() {
        default_title(session.kind).to_owned()
    } else {
        title
    };
    let kind = match session.kind {
        Kind::Dictation => "Dictation",
        Kind::Call => "Call",
        Kind::Note => "Note",
    };
    let mut details = vec![kind.to_owned(), local_time(session.created_ms)];
    let duration = session
        .rows
        .iter()
        .map(|row| row.end_ms)
        .max()
        .or(session.metrics.audio_duration_ms)
        .unwrap_or(0);
    if duration > 0 {
        details.push(format!("Duration {}", clock(duration)));
    }
    let mut participants: Vec<String> = Vec::new();
    for row in &session.rows {
        for name in calls::speaker_labels(row, &session.speaker_names) {
            let name = single_line(&name);
            if !name.is_empty() && !participants.contains(&name) {
                participants.push(name);
            }
        }
    }
    if markdown_format {
        let _ = writeln!(
            result,
            "# {}\n\n{}\n",
            markdown(&title),
            details.join(" · ")
        );
        if !participants.is_empty() {
            let names: Vec<String> = participants.iter().map(|name| markdown(name)).collect();
            let _ = writeln!(result, "Participants: {}\n", names.join(", "));
        }
    } else {
        let _ = writeln!(result, "{title}\n{}", details.join(" · "));
        if !participants.is_empty() {
            let _ = writeln!(result, "Participants: {}", participants.join(", "));
        }
        result.push('\n');
    }
    let personal = session.personal_notes.trim();
    if !personal.is_empty() {
        // Personal notes are the user's own writing, so their Markdown is kept.
        let _ = writeln!(result, "{}\n\n{personal}\n", heading("My notes", "##"));
    }
    let generated: Vec<&str> = session
        .generated_summary
        .iter()
        .flat_map(|summary| summary.items.iter().map(|item| item.text.trim()))
        .filter(|text| !text.is_empty())
        .collect();
    if !generated.is_empty() {
        let _ = writeln!(result, "{}\n", heading("Generated notes", "##"));
        for item in generated {
            if markdown_format {
                let _ = writeln!(result, "- {}", markdown(item));
            } else {
                let _ = writeln!(result, "- {item}");
            }
        }
        result.push('\n');
    }
    if !rows.is_empty() {
        if session.rows.is_empty() {
            let text = rows[0].text.trim();
            let _ = writeln!(
                result,
                "{}\n\n{}",
                heading("Text", "##"),
                if markdown_format {
                    markdown(text)
                } else {
                    text.to_owned()
                }
            );
        } else {
            let _ = writeln!(result, "{}\n", heading("Transcript", "##"));
            let body = export_with(
                &rows,
                &session.speaker_names,
                &session.interruptions,
                format,
            );
            result.push_str(body.strip_prefix("# Call transcript\n\n").unwrap_or(&body));
        }
    }
    Ok(result)
}

pub fn default_title(kind: Kind) -> &'static str {
    match kind {
        Kind::Dictation => "Dictation",
        Kind::Call => "Call transcript",
        Kind::Note => "Note",
    }
}

/// Local date and time, falling back to UTC when the offset is unknown.
pub fn local_time(ms: u64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .ok()
        .map(|timestamp| {
            let local = timestamp.to_offset(
                time::UtcOffset::local_offset_at(timestamp).unwrap_or(time::UtcOffset::UTC),
            );
            format!("{} {:02}:{:02}", local.date(), local.hour(), local.minute())
        })
        .unwrap_or_else(|| "Date unavailable".into())
}

fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

fn timestamp(ms: u64, separator: char) -> String {
    format!(
        "{:02}:{:02}:{:02}{separator}{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1_000 % 60,
        ms % 1_000
    )
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn subtitle(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn markdown(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '`'
                | '*'
                | '_'
                | '{'
                | '}'
                | '['
                | ']'
                | '<'
                | '>'
                | '#'
                | '+'
                | '-'
                | '.'
                | '!'
                | '|'
        ) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(start_ms: u64, end_ms: u64, speakers: &[i32], text: &str) -> Row {
        Row {
            cues: Vec::new(),
            start_ms,
            end_ms,
            speakers: speakers.to_vec(),
            discord: None,
            microphone: false,
            text: text.into(),
        }
    }

    #[test]
    fn srt_preserves_overlap_order_and_precise_long_session_timestamps() {
        let rows = [
            row(3_601_234, 3_605_678, &[2], "Second"),
            row(3_600_123, 3_604_567, &[1], "First"),
        ];
        let output = export(&rows, &[], Format::Srt);
        assert_eq!(
            output,
            "1\n01:00:00,123 --> 01:00:04,567\nSpeaker 1: First\n\n2\n01:00:01,234 --> 01:00:05,678\nSpeaker 2: Second\n\n"
        );
        assert_eq!(rows[0].text, "Second");
    }

    #[test]
    fn subtitle_payload_cannot_create_cues_or_markup_and_keeps_unicode() {
        let rows = [row(0, 1000, &[1], "café 😃\r\n\r\n00:01 --> 00:02 <b> &")];
        let names = [
            "<v fake>\n\nNAME".into(),
            String::new(),
            String::new(),
            String::new(),
        ];
        let output = export(&rows, &names, Format::WebVtt);
        assert!(output.starts_with("WEBVTT\n\n1\n00:00:00.000 --> 00:00:01.000\n"));
        assert!(output.contains("&lt;v fake&gt; NAME: café 😃 00:01 --&gt; 00:02 &lt;b&gt; &amp;"));
        assert_eq!(output.matches(" --> ").count(), 1);
    }

    #[test]
    fn speaker_names_uncertainty_and_overlap_remain_explicit() {
        let mut me = row(0, 1000, &[], "Hello");
        me.microphone = true;
        let rows = [
            me,
            row(1000, 2000, &[1, 2], "Both"),
            row(2000, 3000, &[], "Unknown"),
        ];
        let names = ["Morgan".into(), "Alex".into(), String::new(), String::new()];
        let output = export(&rows, &names, Format::Text);
        assert!(output.contains("You\nHello"));
        assert!(output.contains("Overlap: Morgan + Alex\nBoth"));
        assert!(output.contains("Uncertain speaker\nUnknown"));
    }

    #[test]
    fn empty_and_invalid_subtitle_rows_do_not_leave_numbering_gaps() {
        let rows = [
            row(0, 0, &[1], "No duration"),
            row(10, 1, &[1], "Reversed"),
            row(20, 30, &[1], "  "),
            row(40, 50, &[1], "Valid"),
        ];
        let output = export(&rows, &[], Format::Srt);
        assert_eq!(
            output,
            "1\n00:00:00,040 --> 00:00:00,050\nSpeaker 1: Valid\n\n"
        );
        assert!(export(&rows, &[], Format::Text).contains("No duration"));
        assert_eq!(export(&[], &[], Format::WebVtt), "WEBVTT\n\n");
    }

    #[test]
    fn calls_export_title_participants_notes_and_timed_turns() {
        let mut call = Session::new(Kind::Call);
        call.title = "Planning *sync*".into();
        call.speaker_names = vec!["Morgan".into(), "Alex".into(), String::new(), String::new()];
        call.rows = vec![
            row(0, 1500, &[1], "Hello."),
            row(1500, 4000, &[2], "Ship it."),
        ];
        call.personal_notes = "- my **own** list".into();
        let markdown = session(&call, Format::Markdown).unwrap();
        assert!(markdown.starts_with("# Planning \\*sync\\*\n\nCall · "));
        assert!(markdown.contains("Duration 0:04"));
        assert!(markdown.contains("Participants: Morgan, Alex"));
        assert!(markdown.contains("## My notes\n\n- my **own** list"));
        assert!(
            markdown
                .contains("## Transcript\n\n**Morgan** · 00:00:00.000 to 00:00:01.500\n\nHello\\.")
        );
        assert!(!markdown.contains("# Call transcript"));
        let srt = session(&call, Format::Srt).unwrap();
        assert!(srt.contains("00:00:01,500 --> 00:00:04,000\nAlex: Ship it."));
        let vtt = session(&call, Format::WebVtt).unwrap();
        assert!(vtt.starts_with("WEBVTT\n\n1\n00:00:00.000 --> 00:00:01.500\nMorgan: Hello."));
        assert!(
            session(&call, Format::Text)
                .unwrap()
                .contains("[00:00:01.500 to 00:00:04.000] Alex")
        );
    }

    #[test]
    fn dictations_export_as_one_timed_passage_and_never_invent_timing() {
        let mut dictation = Session::new(Kind::Dictation);
        dictation.title = "Reply".into();
        dictation.text = "Thanks, see you Thursday.".into();
        dictation.metrics.audio_duration_ms = Some(2500);
        let srt = session(&dictation, Format::Srt).unwrap();
        assert_eq!(
            srt,
            "1\n00:00:00,000 --> 00:00:02,500\nYou: Thanks, see you Thursday.\n\n"
        );
        let markdown = session(&dictation, Format::Markdown).unwrap();
        assert!(markdown.contains("## Text\n\nThanks, see you Thursday\\."));
        dictation.metrics.audio_duration_ms = None;
        assert!(session(&dictation, Format::WebVtt).is_err());
        assert!(
            session(&dictation, Format::Text)
                .unwrap()
                .contains("Thanks, see you Thursday.")
        );
        let mut note = Session::new(Kind::Note);
        note.personal_notes = "Only my words.".into();
        assert!(session(&note, Format::Srt).is_err());
        let text = session(&note, Format::Markdown).unwrap();
        assert!(text.starts_with("# Note\n\nNote · "));
        assert!(text.contains("## My notes\n\nOnly my words."));
    }

    #[test]
    fn markdown_retains_spoken_content_without_executing_its_markup() {
        let rows = [row(
            0,
            1000,
            &[1],
            "[click](url)\n# heading <script> **bold**",
        )];
        let names = [
            "*Morgan*".into(),
            String::new(),
            String::new(),
            String::new(),
        ];
        let output = export(&rows, &names, Format::Markdown);
        assert!(output.contains("**\\*Morgan\\***"));
        assert!(output.contains("\\[click\\](url)\n\\# heading \\<script\\> \\*\\*bold\\*\\*"));
    }

    #[test]
    fn interruptions_appear_between_turns_where_the_format_allows() {
        use crate::calls::interruptions::{Cause, Source};
        let rows = [
            row(0, 4000, &[1], "Before the gap."),
            row(9000, 12000, &[1], "After the gap."),
        ];
        let gaps = [
            calls::Interruption::new(5000, 8000, Source::Microphone, Cause::DeviceLost),
            calls::Interruption::new(20000, 21000, Source::Processing, Cause::FellBehind),
        ];
        let names = calls::empty_speaker_names();
        let marker = "Audio interrupted 00:05–00:08: microphone disconnected";
        for format in [Format::Text, Format::Markdown, Format::WebVtt] {
            let output = export_with(&rows, &names, &gaps, format);
            let at = output
                .find(marker)
                .unwrap_or_else(|| panic!("{format:?}: {output}"));
            // Markdown escapes the full stop, so search without it.
            assert!(output.find("Before the gap").unwrap() < at);
            assert!(at < output.find("After the gap").unwrap());
            assert!(output.contains("transcription fell behind"), "{format:?}");
        }
        let vtt = export_with(&rows, &names, &gaps, Format::WebVtt);
        assert!(vtt.contains(&format!("NOTE {marker}")));
        // Cue numbers still count turns only.
        assert!(vtt.contains("\n2\n00:00:09.000 --> 00:00:12.000"));
        let srt = export_with(&rows, &names, &gaps, Format::Srt);
        assert!(!srt.contains("interrupted"));
        let mut session = Session::new(Kind::Call);
        session.rows = rows.to_vec();
        session.interruptions = gaps.to_vec();
        let markdown = super::session(&session, Format::Markdown).unwrap();
        assert!(markdown.contains(&format!("> *{marker}*")));
    }
}
