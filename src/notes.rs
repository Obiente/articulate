use crate::calls::{self, Row};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// All note content is copied verbatim from a source row. Timing is the row's
/// range, not an invented sentence timestamp or an inferred task deadline.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Quote {
    pub row: usize,
    pub start_ms: u64,
    pub end_ms: u64,
    pub microphone: bool,
    pub speakers: Vec<i32>,
    pub discord: Option<crate::discord_attribution::Attribution>,
    pub text: String,
}

impl Quote {
    #[allow(
        dead_code,
        reason = "Preserve source-attributed note export for React export controls"
    )]
    pub fn attribution(&self, names: &[String; 4]) -> String {
        let row = Row {
            cues: Vec::new(),
            start_ms: self.start_ms,
            end_ms: self.end_ms,
            microphone: self.microphone,
            speakers: self.speakers.clone(),
            discord: self.discord.clone(),
            text: String::new(),
        };
        let label = calls::label(&row, names);
        format!(
            "{} to {}  {}",
            timestamp(self.start_ms),
            timestamp(self.end_ms),
            if label.is_empty() {
                "Uncertain speaker"
            } else {
                &label
            }
        )
    }
}

#[allow(
    dead_code,
    reason = "Used by the retained source-attributed note export API"
)]
fn timestamp(ms: u64) -> String {
    format!("{:02}:{:02}", ms / 60_000, ms / 1_000 % 60)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Notes {
    pub highlights: Vec<Quote>,
    pub actions: Vec<Quote>,
    source_rows: usize,
    source_end: u64,
    source_last_bytes: usize,
}

struct Candidate<'a> {
    row: usize,
    text: &'a str,
    terms: HashSet<String>,
    action: bool,
    decision: bool,
}

fn terms(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() > 3)
        .map(str::to_lowercase)
        .filter(|word| {
            !matches!(
                word.as_str(),
                "this"
                    | "that"
                    | "with"
                    | "from"
                    | "have"
                    | "will"
                    | "would"
                    | "could"
                    | "should"
                    | "about"
                    | "there"
                    | "their"
                    | "they"
                    | "them"
                    | "then"
                    | "when"
                    | "what"
                    | "your"
                    | "just"
                    | "like"
                    | "some"
                    | "more"
                    | "also"
                    | "been"
                    | "were"
                    | "into"
                    | "think"
                    | "know"
                    | "going"
                    | "really"
            )
        })
        .collect()
}

fn normalized(text: &str) -> String {
    format!(
        " {} ",
        text.replace('’', "'")
            .split(|c: char| !c.is_alphanumeric() && c != '\'')
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
            .join(" ")
    )
}

fn possible_action(text: &str) -> bool {
    let text = normalized(text);
    if [
        " don't need to ",
        " do not need to ",
        " no need to ",
        " won't ",
        " will not ",
        " shouldn't ",
        " should not ",
    ]
    .iter()
    .any(|phrase| text.contains(phrase))
    {
        return false;
    }
    [
        " i will ",
        " i'll ",
        " we will ",
        " we'll ",
        " please ",
        " can you ",
        " could you ",
        " need to ",
        " needs to ",
        " should ",
        " follow up ",
        " action item ",
    ]
    .iter()
    .any(|phrase| text.contains(phrase))
}

// A simple sentence boundary is enough for exact quotes. A decimal or an
// abbreviation without trailing whitespace stays attached to its sentence.
fn sentences(text: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    for (offset, ch) in text.char_indices() {
        let end = offset + ch.len_utf8();
        let boundary = ch == '\n'
            || (matches!(ch, '.' | '!' | '?')
                && text[end..].chars().next().is_none_or(char::is_whitespace));
        if boundary {
            let sentence = text[start..end].trim();
            if !sentence.is_empty() {
                result.push(sentence);
            }
            start = end;
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        result.push(tail);
    }
    result
}

impl Notes {
    pub fn build(rows: &[Row]) -> Self {
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        let mut frequencies = HashMap::<String, usize>::new();
        for (row, source) in rows.iter().enumerate() {
            for text in sentences(&source.text) {
                // Quotes remain intact. A very long unpunctuated passage is
                // still available in the transcript rather than silently cut.
                if text.chars().count() > 800 || !seen.insert(normalized(text)) {
                    continue;
                }
                let terms = terms(text);
                if terms.is_empty() {
                    continue;
                }
                for term in &terms {
                    *frequencies.entry(term.clone()).or_default() += 1;
                }
                let words = normalized(text);
                candidates.push(Candidate {
                    row,
                    text,
                    terms,
                    action: possible_action(text),
                    decision: [
                        " agreed ",
                        " decided ",
                        " decision ",
                        " confirmed ",
                        " deadline ",
                        " blocked ",
                        " next step ",
                    ]
                    .iter()
                    .any(|phrase| words.contains(phrase)),
                });
            }
        }
        let mut ranked: Vec<_> = candidates
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let topic = c
                    .terms
                    .iter()
                    .map(|term| frequencies.get(term).copied().unwrap_or(1).min(8))
                    .sum::<usize>();
                // Average topic repetition prevents a long sentence winning just
                // for containing more words. Selection ties retain source order.
                let score = topic * 10 / c.terms.len().max(1)
                    + usize::from(c.decision) * 100
                    + usize::from(c.action) * 30;
                (i, score)
            })
            .collect();
        ranked.sort_by_key(|&(i, score)| (std::cmp::Reverse(score), i));
        let mut picked: Vec<_> = ranked.into_iter().take(6).map(|(i, _)| i).collect();
        picked.sort_unstable();
        let quote = |c: &Candidate<'_>| {
            let row = &rows[c.row];
            Quote {
                row: c.row,
                start_ms: row.start_ms,
                end_ms: row.end_ms,
                microphone: row.microphone,
                speakers: row.speakers.clone(),
                discord: row.discord.clone(),
                text: c.text.into(),
            }
        };
        Self {
            highlights: picked.iter().map(|&i| quote(&candidates[i])).collect(),
            actions: candidates
                .iter()
                .filter(|c| c.action)
                .take(8)
                .map(quote)
                .collect(),
            source_rows: rows.len(),
            source_end: rows.last().map_or(0, |row| row.end_ms),
            source_last_bytes: rows.last().map_or(0, |row| row.text.len()),
        }
    }

    /// Call rows are append-only; adjacent same-speaker sections can extend the
    /// final row without changing row count.
    #[cfg(test)]
    pub fn is_current(&self, rows: &[Row]) -> bool {
        self.source_rows == rows.len()
            && self.source_end == rows.last().map_or(0, |row| row.end_ms)
            && self.source_last_bytes == rows.last().map_or(0, |row| row.text.len())
    }

    #[allow(
        dead_code,
        reason = "Preserve plain-text note export for React export controls"
    )]
    pub fn text(&self, names: &[String; 4]) -> String {
        let mut text = String::from(
            "Meeting notes\nSelected transcript quotes; timestamps refer to source sections.\n",
        );
        for (title, quotes) in [
            ("Highlights", &self.highlights),
            ("Possible actions to review", &self.actions),
        ] {
            text.push_str(&format!("\n{title}\n"));
            if quotes.is_empty() {
                text.push_str("No excerpts selected.\n");
            }
            for quote in quotes {
                text.push_str(&format!(
                    "[{}]\n{}\n\n",
                    quote.attribution(names),
                    quote.text
                ));
            }
        }
        text.trim_end().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(start_ms: u64, text: &str) -> Row {
        Row {
            cues: Vec::new(),
            start_ms,
            end_ms: start_ms + 8000,
            microphone: false,
            speakers: vec![1],
            discord: None,
            text: text.into(),
        }
    }

    #[test]
    fn notes_are_exact_quotes_with_source_ranges_and_renamable_speakers() {
        let rows = vec![
            row(
                0,
                "The café rollout is blocked by testing. We agreed to ship on Friday.",
            ),
            row(
                8000,
                "I'll send the revised checklist tomorrow. Please review the budget.",
            ),
        ];
        let notes = Notes::build(&rows);
        assert_eq!(notes.actions.len(), 2);
        for quote in notes.highlights.iter().chain(&notes.actions) {
            assert!(rows[quote.row].text.contains(&quote.text));
            assert_eq!(quote.start_ms, rows[quote.row].start_ms);
            assert_eq!(quote.end_ms, rows[quote.row].end_ms);
        }
        let mut names: [String; 4] = Default::default();
        names[0] = "Morgan".into();
        let exported = notes.text(&names);
        assert!(exported.contains("00:08 to 00:16  Morgan"));
        assert!(exported.contains("Possible actions to review"));
        assert!(!exported.contains("Speaker 1"));
    }

    #[test]
    fn actions_preserve_negation_and_do_not_claim_commitments() {
        let rows = vec![row(
            0,
            "We do not need to email anyone. I won't send the report. I will review the draft.",
        )];
        let notes = Notes::build(&rows);
        assert_eq!(notes.actions.len(), 1);
        assert_eq!(notes.actions[0].text, "I will review the draft.");
    }

    #[test]
    fn merged_row_extensions_require_refresh_and_quotes_stay_chronological() {
        let mut rows: Vec<_> = (0..10)
            .map(|i| row(i * 8000, &format!("Topic {i} is relevant to the project.")))
            .collect();
        let notes = Notes::build(&rows);
        assert!(notes.is_current(&rows));
        assert!(
            notes
                .highlights
                .windows(2)
                .all(|pair| pair[0].row <= pair[1].row)
        );
        rows.last_mut()
            .unwrap()
            .text
            .push_str(" We agreed to publish.");
        assert!(!notes.is_current(&rows));
    }

    #[test]
    fn empty_repeated_and_unicode_transcripts_are_safe() {
        assert!(Notes::build(&[]).highlights.is_empty());
        let notes = Notes::build(&[row(0, "Résumé ready. Résumé ready. 東京の会議です。")]);
        assert_eq!(notes.highlights.len(), 2);
        assert_eq!(
            sentences("Budget is 3.14 million. Confirmed."),
            ["Budget is 3.14 million.", "Confirmed."]
        );
    }

    #[test]
    fn explicit_decisions_survive_repeated_topic_chatter() {
        let mut rows: Vec<_> = (0..20)
            .map(|i| {
                row(
                    i * 8000,
                    &format!("Discussion section {i} reviewed the rollout checklist."),
                )
            })
            .collect();
        rows.push(row(160000, "We agreed to delay launch until Friday."));
        let notes = Notes::build(&rows);
        assert!(
            notes
                .highlights
                .iter()
                .any(|quote| quote.text == "We agreed to delay launch until Friday.")
        );
    }
}
