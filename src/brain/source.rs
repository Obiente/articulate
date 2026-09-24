use super::Citation;
use crate::classification::{SegmentContext, Transcript};
use anyhow::{Result, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{collections::HashSet, ops::Range};

const MAX_SOURCE_BYTES: usize = 256 * 1024;
const MAX_PROMPT_BYTES: usize = 6000;
const MAX_TURN_BYTES: usize = 4000;
const MAX_SECTIONS: usize = 64;

pub(super) struct Part {
    pub id: String,
    pub citation: Citation,
}

pub(super) fn hash(input: &Transcript) -> Result<String> {
    // Titles and goals are editable library metadata, not model input. Bind
    // the claim to the exact source turns, their identities and timestamps.
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(&input.id, &input.segments))?)
    ))
}

pub(super) fn without_context(input: &Transcript) -> Transcript {
    let mut legacy = input.clone();
    for segment in &mut legacy.segments {
        segment.context = None;
    }
    legacy
}

fn trimmed(text: &str, range: Range<usize>) -> Range<usize> {
    let value = &text[range.clone()];
    let start = range.start + value.len() - value.trim_start().len();
    start..start + value.trim().len()
}

fn ranges(text: &str) -> Result<Vec<Range<usize>>> {
    if text.len() <= MAX_TURN_BYTES {
        return Ok(std::iter::once(0..text.len()).collect());
    }
    let mut result = Vec::new();
    let mut start = 0;
    for (offset, c) in text.char_indices() {
        let end = offset + c.len_utf8();
        if matches!(c, '.' | '?' | '!' | '。' | '！' | '？')
            && (matches!(c, '。' | '！' | '？')
                || text[end..].chars().next().is_none_or(char::is_whitespace))
        {
            let range = trimmed(text, start..end);
            if !range.is_empty() {
                result.push(range);
            }
            start = end;
        }
    }
    let tail = trimmed(text, start..text.len());
    if !tail.is_empty() {
        result.push(tail);
    }
    ensure!(
        result.iter().all(|range| range.len() <= MAX_TURN_BYTES),
        "A sentence is too long for the local summary. Add sentence breaks before summarizing; no text was omitted."
    );
    Ok(result)
}

#[derive(Serialize)]
struct PromptPart<'a> {
    id: &'a str,
    start_ms: u64,
    end_ms: u64,
    speaker: &'a Option<String>,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: &'a Option<SegmentContext>,
}
pub(super) fn prompt(parts: &[Part]) -> Result<String> {
    Ok(serde_json::to_string(
        &parts
            .iter()
            .map(|part| PromptPart {
                id: &part.id,
                start_ms: part.citation.start_ms,
                end_ms: part.citation.end_ms,
                speaker: &part.citation.speaker,
                text: &part.citation.excerpt,
                context: &part.citation.context,
            })
            .collect::<Vec<_>>(),
    )?)
}

pub(super) fn prepare(input: &Transcript) -> Result<Vec<Vec<Part>>> {
    ensure!(
        !input.id.is_empty()
            && input.id.len() <= 256
            && input.title.len() <= 1000
            && input.goal.len() <= 2000
            && !input.segments.is_empty()
            && input.segments.len() <= 5000,
        "Choose a completed transcript to summarize."
    );
    let mut ids = HashSet::new();
    let mut bytes = 0usize;
    let mut sections = Vec::new();
    let mut current = Vec::new();
    let mut count = 0usize;
    for segment in &input.segments {
        ensure!(
            !segment.id.is_empty()
                && segment.id.len() <= 256
                && ids.insert(&segment.id)
                && segment.start_ms <= segment.end_ms
                && segment.speaker.as_ref().is_none_or(|s| s.len() <= 512),
            "The transcript contains an invalid or repeated source ID."
        );
        bytes = bytes.saturating_add(segment.text.len());
        ensure!(
            bytes <= MAX_SOURCE_BYTES,
            "This transcript is too large for one summary. Split it into shorter sessions; no text was omitted."
        );
        ensure!(
            !segment
                .text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')),
            "The transcript contains unsupported control characters."
        );
        if let Some(context) = &segment.context {
            ensure!(
                context.cues.len() <= 64
                    && context.cues.iter().all(|cue| {
                        cue.start_ms < cue.end_ms
                            && cue.start_ms >= segment.start_ms
                            && cue.end_ms <= segment.end_ms
                            && cue.label.len() <= 64
                            && !cue.label.chars().any(char::is_control)
                    }),
                "The transcript contains invalid audio cues."
            );
        }
        if segment.text.trim().is_empty()
            && segment
                .context
                .as_ref()
                .is_none_or(|context| context.cues.is_empty())
        {
            continue;
        }
        for range in ranges(&segment.text)? {
            let part = Part {
                id: format!("s{count}"),
                citation: Citation {
                    source_id: segment.id.clone(),
                    start_byte: range.start,
                    end_byte: range.end,
                    start_ms: segment.start_ms,
                    end_ms: segment.end_ms,
                    speaker: segment.speaker.clone(),
                    excerpt: segment.text[range].to_owned(),
                    context: segment.context.clone(),
                },
            };
            count += 1;
            current.push(part);
            if prompt(&current)?.len() > MAX_PROMPT_BYTES {
                let part = current.pop().unwrap();
                ensure!(
                    !current.is_empty(),
                    "A source passage is too large for the local summary."
                );
                sections.push(std::mem::take(&mut current));
                current.push(part);
                ensure!(
                    prompt(&current)?.len() <= MAX_PROMPT_BYTES,
                    "A source passage is too large for the local summary."
                );
            }
            ensure!(
                sections.len() < MAX_SECTIONS,
                "This transcript needs too many summary sections. Split it into shorter sessions; no text was omitted."
            );
        }
    }
    if !current.is_empty() {
        sections.push(current);
    }
    ensure!(
        !sections.is_empty(),
        "This transcript has no words to summarize."
    );
    Ok(sections)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classification::{Segment, SpeakerOrigin};
    fn input(text: String) -> Transcript {
        Transcript {
            id: "test".into(),
            title: "Test".into(),
            goal: String::new(),
            segments: vec![Segment {
                id: "row-0".into(),
                start_ms: 10,
                end_ms: 9000,
                speaker: Some("Casey".into()),
                text,
                context: None,
            }],
        }
    }
    #[test]
    fn long_turns_are_complete_exact_excerpts_with_original_times() {
        let text = "We did not approve this proposal. We will discuss it tomorrow. ".repeat(180);
        let input = input(text);
        let sections = prepare(&input).unwrap();
        assert!(sections.len() > 1);
        let mut words = Vec::new();
        for section in &sections {
            assert!(prompt(section).unwrap().len() <= MAX_PROMPT_BYTES);
            for part in section {
                let source = &part.citation;
                assert_eq!(
                    &input.segments[0].text[source.start_byte..source.end_byte],
                    source.excerpt
                );
                assert_eq!((source.start_ms, source.end_ms), (10, 9000));
                words.extend(source.excerpt.split_whitespace());
            }
        }
        assert_eq!(
            words,
            input.segments[0]
                .text
                .split_whitespace()
                .collect::<Vec<_>>()
        );
    }
    #[test]
    fn oversized_sentence_is_rejected_without_dropping_negation_or_tail() {
        assert!(prepare(&input("We did not ".to_owned() + &"approve ".repeat(700))).is_err());
        let input = input("The café opens Friday. 明日は休みです。".repeat(150));
        assert!(prepare(&input).is_ok());
    }
    #[test]
    fn duplicate_ids_are_rejected_and_hash_covers_speakers_and_metadata() {
        let mut input = input("The server is ready.".into());
        let original = hash(&input).unwrap();
        input.title = "A renamed session".into();
        input.goal = "Different library metadata".into();
        assert_eq!(original, hash(&input).unwrap());
        input.segments[0].speaker = Some("Jordan".into());
        assert_ne!(original, hash(&input).unwrap());
        input.segments.push(input.segments[0].clone());
        assert!(prepare(&input).is_err());
    }

    #[test]
    fn prompt_includes_timed_audio_context_and_cue_only_rows() {
        let mut transcript = input(String::new());
        transcript.segments[0].context = Some(SegmentContext {
            speaker_origin: SpeakerOrigin::Diarization,
            overlapping_speech: true,
            cues: vec![crate::sensevoice::Cue {
                start_ms: 200,
                end_ms: 600,
                label: "Laughing".into(),
            }],
        });
        let sections = prepare(&transcript).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0][0].citation.excerpt, "");
        let prompt: serde_json::Value =
            serde_json::from_str(&prompt(&sections[0]).unwrap()).unwrap();
        let source = &prompt[0];
        assert_eq!(source["start_ms"], 10);
        assert_eq!(source["end_ms"], 9000);
        assert_eq!(source["speaker"], "Casey");
        assert_eq!(source["context"]["speaker_origin"], "diarization");
        assert_eq!(source["context"]["overlapping_speech"], true);
        assert_eq!(source["context"]["cues"][0]["label"], "Laughing");
        assert_eq!(source["context"]["cues"][0]["start_ms"], 200);
        assert_ne!(
            hash(&transcript).unwrap(),
            hash(&without_context(&transcript)).unwrap()
        );
    }
}
