//! Exact excerpts for grouped speaker turns. Ranges keep the parent turn's time
//! span: splitting text cannot establish more precise audio timestamps.
use super::{Segment, Transcript};
use anyhow::{Context, Result, ensure};
use assort_data::BatchLimits;
use assort_tokenizer::TextTokenizer;
use std::ops::Range;

fn prefix(input: &Transcript) -> String {
    let mut prefix = "excerpt:".to_owned();
    while input.segments.iter().any(|s| s.id.starts_with(&prefix)) {
        prefix.insert(0, '#');
    }
    prefix
}

fn trimmed(text: &str, range: Range<usize>) -> Range<usize> {
    let value = &text[range.clone()];
    let start = range.start + value.len() - value.trim_start().len();
    start..start + value.trim().len()
}

fn sentences(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (offset, character) in text.char_indices() {
        let end = offset + character.len_utf8();
        if matches!(character, '.' | '?' | '!')
            && text[end..].chars().next().is_none_or(char::is_whitespace)
        {
            let range = trimmed(text, start..end);
            if !range.is_empty() {
                ranges.push(range);
            }
            start = end;
        }
    }
    let range = trimmed(text, start..text.len());
    if !range.is_empty() {
        ranges.push(range);
    }
    ranges
}

pub(super) fn prepare(
    input: &Transcript,
    tokenizer: &impl TextTokenizer,
    limits: &BatchLimits,
) -> Result<Transcript> {
    let mut prepared = Transcript {
        id: input.id.clone(),
        title: input.title.clone(),
        goal: input.goal.clone(),
        segments: Vec::with_capacity(input.segments.len()),
    };
    let prefix = prefix(input);
    for (index, source) in input.segments.iter().enumerate() {
        let fits = |text: &str| -> Result<bool> {
            let speaker = source
                .speaker
                .as_ref()
                .map(|s| format!("{s}: "))
                .unwrap_or_default();
            let state = format!("Goal: {}\nTranscript:\n{speaker}{text}\n", input.goal);
            Ok(text.split_whitespace().count() <= super::MAX_WORDS
                && tokenizer
                    .encode(&format!("Classify this passage: {text}"))?
                    .len()
                    <= limits.max_question_tokens
                && tokenizer.encode(&state)?.len() <= limits.max_state_tokens)
        };
        for range in sentences(&source.text) {
            // A mid-sentence cut can lose the scope of a negation or condition.
            // Reject it explicitly instead of offering a misleading partial quote.
            ensure!(
                fits(&source.text[range.clone()])?,
                "A sentence is too long for notes review. Your transcript is unchanged."
            );
            let id = if range == (0..source.text.len()) {
                source.id.clone()
            } else {
                format!("{prefix}{index}:{}:{}", range.start, range.end)
            };
            prepared.segments.push(Segment {
                id,
                start_ms: source.start_ms,
                end_ms: source.end_ms,
                speaker: source.speaker.clone(),
                text: source.text[range].to_owned(),
            });
            ensure!(
                prepared.segments.len() <= super::MAX_SEGMENTS,
                "Too many passages for one notes review"
            );
        }
    }
    Ok(prepared)
}

/// Validate a returned quote against an immutable original transcript.
pub(crate) fn locate(input: &Transcript, source: &Segment) -> Result<(usize, Range<usize>)> {
    let (index, range) = if let Some(index) = input.segments.iter().position(|s| s.id == source.id)
    {
        (index, 0..input.segments[index].text.len())
    } else {
        let prefix = prefix(input);
        let mut parts = source
            .id
            .strip_prefix(&prefix)
            .context("Unknown source passage")?
            .split(':');
        let index: usize = parts.next().context("Missing source row")?.parse()?;
        let start: usize = parts.next().context("Missing passage start")?.parse()?;
        let end: usize = parts.next().context("Missing passage end")?.parse()?;
        ensure!(
            parts.next().is_none() && start < end,
            "Invalid source range"
        );
        (index, start..end)
    };
    let parent = input.segments.get(index).context("Unknown source row")?;
    ensure!(
        parent.text.get(range.clone()) == Some(source.text.as_str())
            && (range == (0..parent.text.len()) || sentences(&parent.text).contains(&range))
            && parent.start_ms == source.start_ms
            && parent.end_ms == source.end_ms
            && parent.speaker == source.speaker,
        "The source passage changed"
    );
    Ok((index, range))
}

#[cfg(test)]
mod tests {
    use super::*;
    use assort_tokenizer::ByteTokenizer;

    fn input(text: &str) -> Transcript {
        Transcript {
            id: "synthetic".into(),
            title: "Test".into(),
            goal: "Notes".into(),
            segments: vec![Segment {
                id: "row-0".into(),
                start_ms: 10,
                end_ms: 9000,
                speaker: Some("Alex".into()),
                text: text.into(),
            }],
        }
    }

    #[test]
    fn grouped_turns_become_exact_token_bounded_excerpts() {
        let original = input(&format!(
            "Hello. {}\nThe café closes at 8.5 tonight.",
            "We will review the document and send it tomorrow. ".repeat(14)
        ));
        let limits = assort_transcript::transcript_limits(1);
        let prepared = prepare(&original, &ByteTokenizer, &limits).unwrap();
        assert!(prepared.segments.len() > 5);
        let mut covered = Vec::new();
        for excerpt in &prepared.segments {
            let (index, range) = locate(&original, excerpt).unwrap();
            assert_eq!(index, 0);
            assert!(
                ByteTokenizer
                    .encode(&format!("Classify this passage: {}", excerpt.text))
                    .unwrap()
                    .len()
                    <= limits.max_question_tokens
            );
            assert_eq!((excerpt.start_ms, excerpt.end_ms), (10, 9000));
            covered.extend(original.segments[0].text[range].split_whitespace());
        }
        assert_eq!(
            covered,
            original.segments[0]
                .text
                .split_whitespace()
                .collect::<Vec<_>>()
        );
        assert!(prepared.segments.last().unwrap().text.contains("8.5"));
    }

    #[test]
    fn rejects_changed_quotes_times_and_invalid_unicode_ranges() {
        let original = input("Hello. Café tomorrow.");
        let prepared = prepare(
            &original,
            &ByteTokenizer,
            &assort_transcript::transcript_limits(1),
        )
        .unwrap();
        let mut forged = prepared.segments[1].clone();
        forged.text = "Cafe tomorrow.".into();
        assert!(locate(&original, &forged).is_err());
        forged = prepared.segments[1].clone();
        forged.start_ms += 1;
        assert!(locate(&original, &forged).is_err());
        forged.id = "excerpt:0:11:20".into();
        assert!(locate(&original, &forged).is_err());
    }

    #[test]
    fn original_ids_cannot_collide_with_excerpt_ids() {
        let mut original = input("One sentence. Another sentence.");
        original.segments[0].id = "excerpt:0:0:13".into();
        let prepared = prepare(
            &original,
            &ByteTokenizer,
            &assort_transcript::transcript_limits(1),
        )
        .unwrap();
        assert!(prepared.segments[0].id.starts_with("#excerpt:"));
        for excerpt in &prepared.segments {
            locate(&original, excerpt).unwrap();
        }
    }

    #[test]
    fn oversized_single_words_fail_without_truncation() {
        let original = input(&"x".repeat(300));
        assert!(
            prepare(
                &original,
                &ByteTokenizer,
                &assort_transcript::transcript_limits(1)
            )
            .is_err()
        );
    }
    #[test]
    fn sentences_never_lose_negation_or_conditions_at_token_boundaries() {
        let original = input(&format!(
            "Only if {} we should not approve this release.",
            "the review passes and ".repeat(30)
        ));
        assert!(
            prepare(
                &original,
                &ByteTokenizer,
                &assort_transcript::transcript_limits(1)
            )
            .is_err()
        );
        let original = input("Do not\nsend it.");
        let prepared = prepare(
            &original,
            &ByteTokenizer,
            &assort_transcript::transcript_limits(1),
        )
        .unwrap();
        assert_eq!(prepared.segments[0].text, original.segments[0].text);
        let mut forged = original.segments[0].clone();
        forged.id = "excerpt:0:7:15".into();
        forged.text = "send it.".into();
        assert!(locate(&original, &forged).is_err());
    }
}
