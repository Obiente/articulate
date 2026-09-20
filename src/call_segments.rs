//! Recognition needs speech context, whereas diarization may change its mind
//! every few frames. Keep jittery speech together instead of independently
//! decoding tiny fragments and guessing which generated words belong to whom.
use crate::speakers::Turn;

const PAUSE: usize = 4_800; // 300 ms at 16 kHz
const MIN_CONTEXT: usize = 24_000; // 1.5 seconds
const PADDING: usize = 4_000; // Adjacent silence only, never another turn.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub start: usize,
    pub end: usize,
    pub audio_start: usize,
    pub audio_end: usize,
    pub speakers: Vec<i32>,
}

/// Input is the diarizer's chronological, nonoverlapping partition. Every
/// detected speech sample is retained once. No PCM is concatenated across gaps;
/// each segment is a contiguous slice of the original capture window.
pub fn plan(turns: &[Turn], samples: usize) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut island = Vec::<Turn>::new();
    for turn in turns {
        let start = turn.start.min(samples);
        let end = turn.end.min(samples);
        if end <= start {
            continue;
        }
        if island
            .last()
            .is_some_and(|last| start.saturating_sub(last.end) >= PAUSE)
        {
            finish_island(&mut island, &mut segments);
        }
        if let Some(last) = island.last_mut()
            && last.speakers == turn.speakers
        {
            last.end = end;
        } else {
            island.push(Turn {
                start,
                end,
                speakers: turn.speakers.clone(),
            });
        }
    }
    finish_island(&mut island, &mut segments);

    for index in 0..segments.len() {
        let start = segments[index].start;
        let end = segments[index].end;
        // Split any shared silent gap at its midpoint so adjacent recognition
        // clips never duplicate speech, including at a zero-gap speaker change.
        let before = if index == 0 {
            0
        } else {
            let previous_end = segments[index - 1].end;
            previous_end + start.saturating_sub(previous_end) / 2
        };
        let after = if index + 1 == segments.len() {
            samples
        } else {
            end + segments[index + 1].start.saturating_sub(end) / 2
        };
        segments[index].audio_start = start.saturating_sub(PADDING).max(before);
        segments[index].audio_end = end.saturating_add(PADDING).min(after);
    }
    segments
}

fn finish_island(island: &mut Vec<Turn>, result: &mut Vec<Segment>) {
    if island.is_empty() {
        return;
    }
    if island
        .iter()
        .any(|turn| turn.end - turn.start < MIN_CONTEXT)
    {
        let first = &island[0];
        let last = island.last().unwrap();
        // Different speaker sets may be rapid turns or brief overlap. Calling
        // their entire sentence an overlap or assigning it to a dominant voice
        // would invent attribution. Keep that row explicitly uncertain.
        let speakers = if island.iter().all(|turn| turn.speakers == first.speakers) {
            first.speakers.clone()
        } else {
            vec![0]
        };
        result.push(segment(first.start, last.end, speakers));
    } else {
        result.extend(
            island
                .iter()
                .map(|turn| segment(turn.start, turn.end, turn.speakers.clone())),
        );
    }
    island.clear();
}

fn segment(start: usize, end: usize, speakers: Vec<i32>) -> Segment {
    Segment {
        start,
        end,
        audio_start: start,
        audio_end: end,
        speakers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(start_ms: usize, end_ms: usize, speakers: &[i32]) -> Turn {
        Turn {
            start: start_ms * 16,
            end: end_ms * 16,
            speakers: speakers.to_vec(),
        }
    }

    #[test]
    fn clean_long_turns_retain_labels_and_do_not_redecode_neighbor_speech() {
        let input = [turn(100, 2600, &[1]), turn(2600, 5100, &[2])];
        let result = plan(&input, 6_000 * 16);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].speakers, [1]);
        assert_eq!(result[1].speakers, [2]);
        assert_eq!(result[0].audio_end, result[1].audio_start);
        assert_eq!(result[0].audio_end, 2600 * 16);
    }

    #[test]
    fn micro_overlap_is_decoded_in_sentence_context_without_claiming_a_speaker() {
        let result = plan(
            &[
                turn(0, 2600, &[1]),
                turn(2600, 2780, &[1, 2]),
                turn(2780, 7000, &[1]),
            ],
            8_000 * 16,
        );
        assert_eq!(result.len(), 1);
        assert_eq!((result[0].start, result[0].end), (0, 7000 * 16));
        assert_eq!(result[0].speakers, [0]);
    }

    #[test]
    fn same_voice_short_segments_recover_context_across_small_silences() {
        let result = plan(
            &[
                turn(0, 300, &[1]),
                turn(400, 950, &[1]),
                turn(1100, 2500, &[1]),
            ],
            3_000 * 16,
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].speakers, [1]);
        assert_eq!((result[0].start, result[0].end), (0, 2500 * 16));
    }

    #[test]
    fn rapid_replies_stay_contiguous_and_uncertain_instead_of_being_discarded() {
        let result = plan(
            &[
                turn(0, 100, &[1]),
                turn(100, 400, &[2]),
                turn(400, 2000, &[1]),
            ],
            2500 * 16,
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].speakers, [0]);
        assert_eq!((result[0].start, result[0].end), (0, 2000 * 16));
    }

    #[test]
    fn isolated_short_replies_and_real_overlap_are_preserved() {
        let result = plan(
            &[
                turn(100, 200, &[1]),
                turn(900, 1150, &[2]),
                turn(2000, 4000, &[1, 2]),
            ],
            4500 * 16,
        );
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].speakers, [1]);
        assert_eq!(result[1].speakers, [2]);
        assert_eq!(result[2].speakers, [1, 2]);
    }

    #[test]
    fn a_pause_keeps_a_short_clean_turn_separate_from_the_next_jittery_utterance() {
        // Short turns can use different languages. Do not feed the previous
        // speaker's sentence as context merely to reach a minimum duration.
        let result = plan(
            &[
                turn(0, 1400, &[1]),
                turn(1720, 2200, &[2]),
                turn(2200, 2680, &[1, 2]),
                turn(2680, 4000, &[2]),
            ],
            4000 * 16,
        );
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].speakers, [1]);
        assert_eq!(result[1].speakers, [0]);
        assert_eq!((result[1].start, result[1].end), (1720 * 16, 4000 * 16));
    }

    #[test]
    fn all_detected_samples_are_covered_once_with_bounded_nonoverlapping_clips() {
        let input = [
            turn(0, 1800, &[1]),
            turn(1800, 4000, &[2]),
            turn(4600, 4700, &[3]),
            turn(5000, 6100, &[2]),
            turn(7200, 9000, &[4]),
        ];
        let samples = 8000 * 16;
        let result = plan(&input, samples);
        assert!(
            result
                .windows(2)
                .all(|pair| pair[0].audio_end <= pair[1].audio_start)
        );
        for block in &result {
            assert!(block.audio_start <= block.start && block.start < block.end);
            assert!(block.end <= block.audio_end && block.audio_end <= samples);
        }
        for turn in input {
            for position in turn.start..turn.end.min(samples) {
                assert_eq!(
                    result
                        .iter()
                        .filter(|block| block.start <= position && position < block.end)
                        .count(),
                    1
                );
            }
        }
        assert!(plan(&[], samples).is_empty());
        assert!(plan(&[turn(8000, 9000, &[1])], samples).is_empty());
    }
}
