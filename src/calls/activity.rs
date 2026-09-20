//! Mixed-output activity is timing evidence, not an isolated participant stream.
//! Short changing activity sets need speech context before decoding, just like
//! acoustic diarization. Their combined text cannot be assigned to one person.
use crate::{
    call_segments,
    discord_attribution::{ActivitySegment, Attribution},
    speakers::Turn,
};

pub(super) struct Clip {
    pub start: usize,
    pub end: usize,
    pub audio_start: usize,
    pub audio_end: usize,
    pub attribution: Option<Attribution>,
}

pub(super) fn plan(segments: &[ActivitySegment], samples: usize) -> Vec<Clip> {
    let mut identities = Vec::<&Attribution>::new();
    let turns: Vec<_> = segments
        .iter()
        .map(|segment| {
            // Treat each complete observed speaker set as an opaque identity. A
            // brief overlap must not donate its names to a surrounding sentence.
            let index = identities
                .iter()
                .position(|item| **item == segment.attribution)
                .unwrap_or_else(|| {
                    identities.push(&segment.attribution);
                    identities.len() - 1
                });
            Turn {
                start: segment.start,
                end: segment.end,
                speakers: vec![index as i32 + 1],
            }
        })
        .collect();
    call_segments::plan(&turns, samples)
        .into_iter()
        .map(|segment| {
            let attribution = if segment.speakers.len() == 1 && segment.speakers[0] > 0 {
                identities
                    .get(segment.speakers[0] as usize - 1)
                    .map(|value| (*value).clone())
            } else {
                None
            };
            Clip {
                start: segment.start,
                end: segment.end,
                audio_start: segment.audio_start,
                audio_end: segment.audio_end,
                attribution,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn segment(start_ms: usize, end_ms: usize, ids: &[&str]) -> ActivitySegment {
        ActivitySegment {
            start: start_ms * 16,
            end: end_ms * 16,
            audio_start: start_ms * 16,
            audio_end: end_ms * 16,
            attribution: Attribution {
                generation: 1,
                channel_id: "synthetic-room".into(),
                speakers: ids
                    .iter()
                    .map(|id| crate::discord_attribution::NamedSpeaker {
                        id: (*id).into(),
                        name: (*id).into(),
                        avatar: None,
                    })
                    .collect(),
            },
        }
    }

    #[test]
    fn brief_activity_overlap_keeps_sentence_context_without_claiming_a_voice() {
        let clips = plan(
            &[
                segment(0, 2600, &["Casey"]),
                segment(2600, 2780, &["Casey", "Jordan"]),
                segment(2780, 7000, &["Casey"]),
            ],
            8000 * 16,
        );
        assert_eq!(clips.len(), 1);
        assert_eq!((clips[0].start, clips[0].end), (0, 7000 * 16));
        assert!(clips[0].attribution.is_none());
    }

    #[test]
    fn sustained_speakers_and_overlap_keep_their_exact_sets_without_repeated_pcm() {
        let clips = plan(
            &[
                segment(0, 2600, &["Casey"]),
                segment(2600, 4600, &["Casey", "Jordan"]),
                segment(4600, 7600, &["Jordan"]),
            ],
            8000 * 16,
        );
        assert_eq!(clips.len(), 3);
        assert_eq!(
            clips
                .iter()
                .map(|c| c.attribution.as_ref().unwrap().speakers.len())
                .collect::<Vec<_>>(),
            [1, 2, 1]
        );
        assert!(
            clips
                .windows(2)
                .all(|pair| pair[0].audio_end <= pair[1].audio_start)
        );
        assert_eq!(clips[0].audio_end, clips[1].audio_start);
        assert_eq!(clips[1].audio_end, clips[2].audio_start);
    }

    #[test]
    fn rapid_replies_are_not_mislabelled_as_simultaneous_overlap() {
        let clips = plan(
            &[
                segment(0, 100, &["Casey"]),
                segment(100, 400, &["Jordan"]),
                segment(400, 3000, &["Casey"]),
            ],
            4000 * 16,
        );
        assert_eq!(clips.len(), 1);
        assert!(clips[0].attribution.is_none());
        assert_eq!((clips[0].start, clips[0].end), (0, 3000 * 16));
    }

    #[test]
    fn companion_selection_cannot_fall_back_when_its_adapter_is_unavailable() {
        assert!(super::super::select_native_source(true, false).is_err());
        assert!(super::super::select_native_source(true, true).unwrap());
        assert!(!super::super::select_native_source(false, false).unwrap());
        assert!(!super::super::select_native_source(false, true).unwrap());
    }
}
