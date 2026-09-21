//! Portable call transcripts, using the captured row ranges without inventing
//! word-level timings. Overlapping rows remain overlapping subtitle cues.
use crate::calls::{self, Row};
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
pub fn export(rows: &[Row], names: &[String; 4], format: Format) -> String {
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
    result
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
        let output = export(&rows, &Default::default(), Format::Srt);
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
        let output = export(&rows, &Default::default(), Format::Srt);
        assert_eq!(
            output,
            "1\n00:00:00,040 --> 00:00:00,050\nSpeaker 1: Valid\n\n"
        );
        assert!(export(&rows, &Default::default(), Format::Text).contains("No duration"));
        assert_eq!(
            export(&[], &Default::default(), Format::WebVtt),
            "WEBVTT\n\n"
        );
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
}
