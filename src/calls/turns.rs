//! Conservative turn boundaries on independent, time-aligned 16 kHz tracks.
//! Speaker identity comes from the track; simultaneous speech stays independent.

const FRAME: usize = 320; // 20 ms at 16 kHz.
const PADDING: usize = 640; // 40 ms, taken only from the same speaker's track.
const EXCHANGE_PAUSE: usize = 2_560; // 160 ms of this speaker's silence.
const OTHER_SPEECH: usize = 640; // Ignore isolated activity in another track.
const LONG_PAUSE: usize = 9_600; // 600 ms without a speaker exchange.
const DIGITAL_FLOOR: f64 = 0.000_01;

pub(super) struct Track<'a> {
    pub pcm: &'a [f32],
    pub start_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Turn {
    pub track: usize,
    /// Unpadded voiced bounds, relative to this track, in 16 kHz samples.
    pub start: usize,
    pub end: usize,
    /// Decode bounds, with a small amount of this speaker's surrounding audio.
    pub audio_start: usize,
    pub audio_end: usize,
    /// Enough silence has arrived to make this turn's decode bounds stable.
    pub complete: bool,
}

fn activity(pcm: &[f32]) -> Vec<bool> {
    let mut result = Vec::with_capacity(pcm.len().div_ceil(FRAME));
    let mut quiet_floor = 0.000_06_f64;
    let mut faint_start = None;
    let mut faint_samples = 0;
    let mut faint_promoted = false;
    for (index, frame) in pcm.chunks(FRAME).enumerate() {
        let energy = frame
            .iter()
            .map(|&value| {
                let value = if value.is_finite() {
                    f64::from(value)
                } else {
                    0.0
                };
                value * value
            })
            .sum::<f64>()
            / frame.len() as f64;
        let rms = energy.sqrt();
        let gate = (quiet_floor * 3.0).clamp(0.000_3, 0.000_8);
        let confident = rms >= gate;
        if !confident {
            quiet_floor = quiet_floor * 0.9 + rms.min(gate) * 0.1;
        }
        result.push(confident);
        if rms > DIGITAL_FLOOR {
            let first = *faint_start.get_or_insert(index);
            faint_samples += frame.len();
            // A causal fallback preserves quiet speech, including tracks that
            // never reach the main gate. Recomputing after an append cannot
            // erase an earlier faint turn just because louder speech appeared.
            if confident || faint_samples >= FRAME * 2 {
                if faint_promoted {
                    result[index] = true;
                } else {
                    result[first..=index].fill(true);
                    faint_promoted = true;
                }
            }
        } else {
            faint_start = None;
            faint_samples = 0;
            faint_promoted = false;
        }
    }
    result
}

fn other_active(tracks: &[Track<'_>], activity: &[Vec<bool>], own: usize, at_ms: u64) -> bool {
    tracks.iter().enumerate().any(|(index, track)| {
        if index == own || at_ms < track.start_ms {
            return false;
        }
        let elapsed = at_ms - track.start_ms;
        let Ok(frame) = usize::try_from(elapsed / 20) else {
            return false;
        };
        // The final frame may be partial; do not extend it into future audio.
        elapsed.saturating_mul(16) < track.pcm.len() as u64
            && activity[index].get(frame).copied().unwrap_or(false)
    })
}

fn turn(track: usize, start: usize, end: usize, length: usize, complete: bool) -> Turn {
    Turn {
        track,
        start,
        end,
        audio_start: start.saturating_sub(PADDING),
        audio_end: end.saturating_add(PADDING).min(length),
        complete,
    }
}

pub(super) fn plan(tracks: &[Track<'_>]) -> Vec<Turn> {
    let active = tracks
        .iter()
        .map(|track| activity(track.pcm))
        .collect::<Vec<_>>();
    let mut turns = Vec::new();
    for (index, track) in tracks.iter().enumerate() {
        let mut start = None;
        let mut voiced_end = 0;
        let mut other_samples = 0;
        for (frame, &voiced) in active[index].iter().enumerate() {
            let begin = frame * FRAME;
            let end = (begin + FRAME).min(track.pcm.len());
            if voiced {
                start.get_or_insert(begin);
                voiced_end = end;
                other_samples = 0;
            } else if let Some(first) = start {
                let center_ms = track
                    .start_ms
                    .saturating_add(((begin + end) / 2 / 16) as u64);
                if other_active(tracks, &active, index, center_ms) {
                    other_samples += end - begin;
                }
                let quiet = end - voiced_end;
                if quiet >= LONG_PAUSE || (quiet >= EXCHANGE_PAUSE && other_samples >= OTHER_SPEECH)
                {
                    turns.push(turn(index, first, voiced_end, track.pcm.len(), true));
                    start = None;
                }
            }
        }
        if let Some(first) = start {
            turns.push(turn(index, first, voiced_end, track.pcm.len(), false));
        }
    }
    turns.sort_by_key(|turn| {
        (
            tracks[turn.track]
                .start_ms
                .saturating_add((turn.start / 16) as u64),
            turn.track,
        )
    });
    turns
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speech(length_ms: usize, spans: &[(usize, usize)], volume: f32) -> Vec<f32> {
        let mut pcm = vec![0.0; length_ms * 16];
        for &(start, end) in spans {
            for (index, value) in pcm[start * 16..end * 16].iter_mut().enumerate() {
                *value = if index % 2 == 0 { volume } else { -volume };
            }
        }
        pcm
    }

    #[test]
    fn alternating_people_are_chronological_independent_turns() {
        let a = speech(2200, &[(0, 600), (1000, 1500)], 0.01);
        let b = speech(2200, &[(640, 960)], 0.01);
        let turns = plan(&[
            Track {
                pcm: &a,
                start_ms: 5000,
            },
            Track {
                pcm: &b,
                start_ms: 5000,
            },
        ]);
        assert_eq!(
            turns.iter().map(|turn| turn.track).collect::<Vec<_>>(),
            [0, 1, 0]
        );
        assert_eq!(
            turns
                .iter()
                .map(|turn| (turn.start / 16, turn.end / 16))
                .collect::<Vec<_>>(),
            [(0, 600), (640, 960), (1000, 1500)]
        );
        assert!(turns.iter().all(|turn| turn.complete));
        assert_eq!(turns[0].audio_end / 16, 640);
        assert_eq!(turns[2].audio_start / 16, 960);
    }

    #[test]
    fn continuous_simultaneous_speech_is_not_split_by_the_other_voice() {
        let a = speech(2000, &[(0, 1600)], 0.01);
        let b = speech(1600, &[(0, 800)], 0.01);
        let turns = plan(&[
            Track {
                pcm: &a,
                start_ms: 0,
            },
            Track {
                pcm: &b,
                start_ms: 400,
            },
        ]);
        assert_eq!(turns.len(), 2);
        assert_eq!((turns[0].start, turns[0].end), (0, 1600 * 16));
        assert_eq!((turns[1].start, turns[1].end), (0, 800 * 16));
    }

    #[test]
    fn intrasentence_pause_stays_with_its_speaker() {
        let pcm = speech(1800, &[(0, 400), (760, 1200)], 0.01);
        let turns = plan(&[Track {
            pcm: &pcm,
            start_ms: 0,
        }]);
        assert_eq!(turns.len(), 1);
        assert_eq!((turns[0].start, turns[0].end), (0, 1200 * 16));
        assert!(turns[0].complete);
    }

    #[test]
    fn brief_own_pause_during_overlap_does_not_create_an_exchange() {
        let a = speech(1600, &[(0, 400), (500, 1000)], 0.01);
        let b = speech(1600, &[(0, 1000)], 0.01);
        let turns = plan(&[
            Track {
                pcm: &a,
                start_ms: 0,
            },
            Track {
                pcm: &b,
                start_ms: 0,
            },
        ]);
        assert_eq!(turns.len(), 2);
    }

    #[test]
    fn quiet_speech_and_short_answers_are_preserved_below_the_main_gate() {
        let pcm = speech(1400, &[(100, 180), (800, 820)], 0.000_04);
        let turns = plan(&[Track {
            pcm: &pcm,
            start_ms: 0,
        }]);
        assert_eq!(turns.len(), 1); // Sustained faint answer, not an isolated tiny click.
        assert_eq!((turns[0].start / 16, turns[0].end / 16), (100, 180));
        let short = speech(700, &[(100, 120)], 0.001);
        assert_eq!(
            plan(&[Track {
                pcm: &short,
                start_ms: 0
            }])
            .len(),
            1
        );
    }

    #[test]
    fn completed_turns_and_padding_are_stable_when_audio_is_appended() {
        let a = speech(2200, &[(0, 400), (1000, 1300)], 0.000_04);
        let b = speech(2200, &[(440, 840)], 0.01);
        let prefix = plan(&[
            Track {
                pcm: &a[..900 * 16],
                start_ms: 0,
            },
            Track {
                pcm: &b[..900 * 16],
                start_ms: 0,
            },
        ]);
        let full = plan(&[
            Track {
                pcm: &a,
                start_ms: 0,
            },
            Track {
                pcm: &b,
                start_ms: 0,
            },
        ]);
        assert!(prefix[0].complete);
        assert_eq!(prefix[0], full[0]);
    }

    #[test]
    fn empty_digital_silence_and_nonfinite_samples_do_not_make_turns() {
        assert!(plan(&[]).is_empty());
        let pcm = vec![f32::NAN; 1600];
        assert!(
            plan(&[
                Track {
                    pcm: &[],
                    start_ms: 0
                },
                Track {
                    pcm: &pcm,
                    start_ms: 0
                }
            ])
            .is_empty()
        );
        let silence = vec![0.000_001; 1600];
        assert!(
            plan(&[Track {
                pcm: &silence,
                start_ms: 0
            }])
            .is_empty()
        );
    }
}
