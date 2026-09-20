//! Conservative interval attribution and activity-defined audio windows.
//! Activity never supplies word timings or a durable voiceprint.
use crate::{calls::Row, discord::Observation};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedSpeaker {
    #[serde(skip)]
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<crate::discord::avatar::Avatar>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attribution {
    #[serde(skip)]
    pub generation: u64,
    #[serde(skip)]
    pub channel_id: String,
    pub speakers: Vec<NamedSpeaker>,
}

const FRESH_FOR: Duration = Duration::from_millis(750);
const MAX_GAP: Duration = Duration::from_millis(150);

/// No fixed audio/event latency is assumed. A turn needs a stable remote
/// speaking set over at least 85% of its full acoustic interval. Conflicting
/// identities, disconnected states and channel changes reject the entire row;
/// ASR text is never split heuristically between participants.
pub fn identify(row: &Row, origin: Instant, history: &[Observation]) -> Option<Attribution> {
    if row.microphone
        || row.end_ms <= row.start_ms
        || row.speakers.is_empty()
        || row.speakers.iter().any(|id| !(1..=4).contains(id))
    {
        return None;
    }
    let start = origin.checked_add(Duration::from_millis(row.start_ms))?;
    let end = origin.checked_add(Duration::from_millis(row.end_ms))?;
    let total = end.duration_since(start).as_secs_f64();
    let mut covered = 0.0;
    let mut speaking = 0.0;
    let mut coverage_end = start;
    let mut session: Option<(u64, &str)> = None;
    let mut matched: Option<Attribution> = None;
    for (index, observation) in history.iter().enumerate() {
        if observation.at >= end {
            break;
        }
        let next = history.get(index + 1).map_or(end, |next| next.at);
        // Histories must be chronological. A clock reset cannot supply evidence.
        if next < observation.at {
            return None;
        }
        let a = start.max(observation.at);
        let b = end.min(next);
        if b <= a {
            continue;
        }
        if !observation.valid {
            return None;
        }
        let channel = observation.channel_id.as_deref()?;
        if channel.is_empty() {
            return None;
        }
        let current = (observation.generation, channel);
        if session.is_some_and(|session| session != current) {
            return None;
        }
        session = Some(current);
        let fresh_end = b.min(observation.at.checked_add(FRESH_FOR)?);
        if fresh_end <= a {
            continue;
        }
        if a.saturating_duration_since(coverage_end) > MAX_GAP {
            return None;
        }
        coverage_end = fresh_end;
        let duration = fresh_end.duration_since(a).as_secs_f64();
        covered += duration;
        let mut participants: Vec<_> = observation
            .participants
            .iter()
            .filter(|participant| participant.speaking && !participant.is_self)
            .collect();
        participants.sort_by(|a, b| a.id.cmp(&b.id));
        if participants.is_empty() {
            continue;
        }
        // Mixed loopback overlap is named only when both acoustic diarization
        // and runtime activity agree on its cardinality throughout the turn.
        if participants.len() != row.speakers.len()
            || participants
                .iter()
                .any(|p| p.id.is_empty() || p.name.trim().is_empty())
            || participants.windows(2).any(|pair| pair[0].id == pair[1].id)
        {
            return None;
        }
        let speakers: Vec<_> = participants
            .into_iter()
            .map(|p| NamedSpeaker {
                id: p.id.clone(),
                name: p.name.clone(),
                avatar: p.avatar.clone(),
            })
            .collect();
        if let Some(previous) = &matched
            && previous
                .speakers
                .iter()
                .map(|p| &p.id)
                .ne(speakers.iter().map(|p| &p.id))
        {
            return None;
        }
        matched = Some(Attribution {
            generation: observation.generation,
            channel_id: channel.to_owned(),
            speakers,
        });
        speaking += duration;
    }
    if end.saturating_duration_since(coverage_end) > MAX_GAP
        || covered / total < 0.90
        || speaking / total < 0.85
    {
        return None;
    }
    matched
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::Participant;

    fn row(start_ms: u64, end_ms: u64, speakers: &[i32]) -> Row {
        Row {
            start_ms,
            end_ms,
            speakers: speakers.to_vec(),
            microphone: false,
            discord: None,
            text: "Transcript.".into(),
        }
    }

    fn observation(origin: Instant, ms: u64, names: &[(&str, &str)]) -> Observation {
        Observation {
            at: origin + Duration::from_millis(ms),
            generation: 1,
            channel_id: Some("synthetic-channel".into()),
            valid: true,
            participants: names
                .iter()
                .map(|(id, name)| Participant {
                    id: (*id).into(),
                    name: (*name).into(),
                    avatar: None,
                    speaking: true,
                    is_self: false,
                })
                .collect(),
        }
    }

    fn history(origin: Instant, names: &[(&str, &str)]) -> Vec<Observation> {
        (0..=8)
            .map(|i| observation(origin, i * 250, names))
            .collect()
    }

    #[test]
    fn timed_turns_use_the_audio_origin_without_learning_cluster_identity() {
        let origin = Instant::now();
        let mut observations = history(origin, &[("a", "Zoë")]);
        for event in &mut observations[4..] {
            event.participants[0].id = "b".into();
            event.participants[0].name = "李".into();
        }
        assert_eq!(
            identify(&row(0, 1000, &[1]), origin, &observations)
                .unwrap()
                .speakers[0]
                .name,
            "Zoë"
        );
        assert_eq!(
            identify(&row(1000, 2000, &[1]), origin, &observations)
                .unwrap()
                .speakers[0]
                .name,
            "李"
        );
        assert!(identify(&row(0, 2000, &[1]), origin, &observations).is_none());
    }

    #[test]
    fn stale_missing_disconnected_and_cross_session_intervals_stay_anonymous() {
        let origin = Instant::now();
        let events = history(origin, &[("a", "Ada")]);
        assert!(identify(&row(0, 2000, &[1]), origin, &events[..1]).is_none());
        assert!(identify(&row(0, 2000, &[1]), origin, &events[2..]).is_none());
        let mut gap = events.clone();
        gap.drain(2..7);
        assert!(identify(&row(0, 2000, &[1]), origin, &gap).is_none());
        for kind in 0..3 {
            let mut altered = events.clone();
            match kind {
                0 => altered[4].valid = false,
                1 => altered[4].generation += 1,
                _ => altered[4].channel_id = Some("another-channel".into()),
            }
            assert!(identify(&row(0, 2000, &[1]), origin, &altered).is_none());
        }
    }

    #[test]
    fn overlap_requires_agreement_and_never_assigns_local_voice_to_output() {
        let origin = Instant::now();
        let mut events = history(origin, &[("b", "Bea"), ("a", "Ada")]);
        let overlap = identify(&row(0, 2000, &[1, 2]), origin, &events).unwrap();
        assert_eq!(
            overlap
                .speakers
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["Ada", "Bea"]
        );
        assert!(identify(&row(0, 2000, &[1]), origin, &events).is_none());
        for event in &mut events {
            event.participants[0].is_self = true;
        }
        let remote = identify(&row(0, 2000, &[1]), origin, &events).unwrap();
        assert_eq!(remote.speakers[0].name, "Ada");
        assert!(identify(&row(0, 2000, &[1, 2]), origin, &events).is_none());
        let mut microphone = row(0, 2000, &[]);
        microphone.microphone = true;
        assert!(identify(&microphone, origin, &events).is_none());
        assert!(identify(&row(0, 2000, &[0]), origin, &events).is_none());
    }

    #[test]
    fn predecessor_covers_start_but_boundary_activity_is_not_extended_backwards() {
        let origin = Instant::now();
        let events = history(origin, &[("a", "Ada")]);
        assert!(identify(&row(100, 1900, &[1]), origin, &events).is_some());
        assert!(identify(&row(0, 2000, &[1]), origin, &events[1..]).is_none());
        let mut rapid = events;
        rapid[3].participants[0].id = "b".into();
        assert!(identify(&row(0, 2000, &[1]), origin, &rapid).is_none());
    }

    #[test]
    fn local_only_missing_names_and_reordered_events_cannot_supply_names() {
        let origin = Instant::now();
        let events = history(origin, &[("a", "Ada")]);
        for kind in 0..4 {
            let mut altered = events.clone();
            match kind {
                0 => {
                    for event in &mut altered {
                        event.participants[0].is_self = true;
                    }
                }
                1 => altered[4].participants[0].name.clear(),
                2 => altered.swap(3, 4),
                _ => {
                    let duplicate = altered[4].participants[0].clone();
                    altered[4].participants.push(duplicate);
                }
            }
            assert!(identify(&row(0, 2000, &[1]), origin, &altered).is_none());
        }
    }
}
/// A contiguous slice of mixed call audio described by Discord's voice activity.
/// This is participant timing, not an isolated audio stream or word alignment.
#[derive(Clone, Debug)]
pub struct ActivitySegment {
    pub start: usize,
    pub end: usize,
    pub audio_start: usize,
    pub audio_end: usize,
    pub attribution: Attribution,
}

/// Only a fully covered, fresh, single-channel observation window may replace
/// acoustic diarization. Missing/stale metadata falls back for the whole draft.
pub fn plan_activity(
    origin: Instant,
    offset_ms: u64,
    samples: usize,
    history: &[Observation],
) -> Option<Vec<ActivitySegment>> {
    if samples == 0 {
        return Some(Vec::new());
    }
    let start = origin.checked_add(Duration::from_millis(offset_ms))?;
    let end = start.checked_add(Duration::from_secs_f64(samples as f64 / 16_000.0))?;
    let mut cursor = start;
    let mut session: Option<(u64, &str)> = None;
    let mut segments: Vec<ActivitySegment> = Vec::new();
    for (index, observation) in history.iter().enumerate() {
        let next = history.get(index + 1).map_or(end, |item| item.at);
        if next < observation.at {
            return None;
        }
        if observation.at >= end {
            break;
        }
        let a = start.max(observation.at);
        let b = end.min(next);
        if b <= a {
            continue;
        }
        if !observation.valid || a > cursor || observation.at.checked_add(FRESH_FOR)? < b {
            return None;
        }
        let channel = observation
            .channel_id
            .as_deref()
            .filter(|channel| !channel.is_empty())?;
        let current = (observation.generation, channel);
        if session.is_some_and(|previous| previous != current) {
            return None;
        }
        session = Some(current);
        cursor = b;
        let mut active: Vec<_> = observation
            .participants
            .iter()
            .filter(|p| p.speaking && !p.is_self)
            .collect();
        active.sort_by(|a, b| a.id.cmp(&b.id));
        if active.is_empty() {
            continue;
        }
        if active
            .iter()
            .any(|p| p.id.is_empty() || p.name.trim().is_empty())
            || active.windows(2).any(|pair| pair[0].id == pair[1].id)
        {
            return None;
        }
        let speakers: Vec<_> = active
            .iter()
            .map(|p| NamedSpeaker {
                id: p.id.clone(),
                name: p.name.clone(),
                avatar: p.avatar.clone(),
            })
            .collect();
        let first = ((a.duration_since(start).as_nanos() * 16_000) / 1_000_000_000) as usize;
        let last =
            (((b.duration_since(start).as_nanos() * 16_000) / 1_000_000_000) as usize).min(samples);
        if last <= first {
            continue;
        }
        if let Some(previous) = segments.last_mut()
            && first.saturating_sub(previous.end) <= 9_600
            && previous
                .attribution
                .speakers
                .iter()
                .map(|p| &p.id)
                .eq(speakers.iter().map(|p| &p.id))
        {
            previous.end = last;
            previous.audio_end = last;
        } else {
            segments.push(ActivitySegment {
                start: first,
                end: last,
                audio_start: first,
                audio_end: last,
                attribution: Attribution {
                    generation: current.0,
                    channel_id: channel.into(),
                    speakers,
                },
            });
        }
    }
    if cursor < end || session.is_none() {
        return None;
    }
    for index in 0..segments.len() {
        let first = segments[index].start;
        let last = segments[index].end;
        let before = if index == 0 {
            0
        } else {
            let end = segments[index - 1].end;
            end + (first.saturating_sub(end)) / 2
        };
        let after = if index + 1 == segments.len() {
            samples
        } else {
            last + (segments[index + 1].start.saturating_sub(last)) / 2
        };
        segments[index].audio_start = first.saturating_sub(4_000).max(before);
        segments[index].audio_end = last.saturating_add(4_000).min(after);
    }
    Some(segments)
}

#[cfg(test)]
mod activity_tests {
    use super::*;
    use crate::discord::Participant;

    fn history(
        origin: Instant,
        duration_ms: u64,
        active: impl Fn(u64) -> Vec<&'static str>,
    ) -> Vec<Observation> {
        (0..=duration_ms / 100)
            .map(|index| {
                let ms = index * 100;
                Observation {
                    at: origin + Duration::from_millis(ms),
                    generation: 1,
                    channel_id: Some("synthetic-room".into()),
                    valid: true,
                    participants: active(ms)
                        .into_iter()
                        .map(|name| Participant {
                            id: name.into(),
                            name: name.into(),
                            avatar: None,
                            speaking: true,
                            is_self: false,
                        })
                        .collect(),
                }
            })
            .collect()
    }

    #[test]
    fn activity_preserves_names_across_a_short_pause_without_acoustic_ids() {
        let origin = Instant::now();
        let observed = history(origin, 3000, |ms| {
            if (1200..1500).contains(&ms) {
                vec![]
            } else {
                vec!["Casey"]
            }
        });
        let result = plan_activity(origin, 0, 48_000, &observed).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!((result[0].start, result[0].end), (0, 48_000));
        assert_eq!(result[0].attribution.speakers[0].name, "Casey");
    }

    #[test]
    fn activity_changes_and_actual_overlap_remain_distinct_nonoverlapping_audio() {
        let origin = Instant::now();
        let observed = history(origin, 4000, |ms| match ms {
            0..=1499 => vec!["Casey"],
            1500..=1699 => vec!["Casey", "Jordan"],
            _ => vec!["Jordan"],
        });
        let result = plan_activity(origin, 0, 64_000, &observed).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(
            result
                .iter()
                .map(|s| s.attribution.speakers.len())
                .collect::<Vec<_>>(),
            [1, 2, 1]
        );
        assert_eq!(result[0].audio_end, result[1].audio_start);
        assert_eq!(result[1].audio_end, result[2].audio_start);
        assert_eq!(result[1].end - result[1].start, 3200);
        assert!(result.iter().all(|segment| segment.audio_end <= 64_000));
    }

    #[test]
    fn silent_valid_activity_is_not_a_request_for_acoustic_classification() {
        let origin = Instant::now();
        assert!(
            plan_activity(origin, 0, 32_000, &history(origin, 2000, |_| vec![]))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn missing_stale_disconnected_reordered_or_changed_channel_activity_falls_back() {
        let origin = Instant::now();
        assert!(plan_activity(origin, 0, 32_000, &[]).is_none());
        let good = history(origin, 2000, |_| vec!["Casey"]);
        assert!(plan_activity(origin, 0, 32_000, &good[..1]).is_none());
        let mut bad = good.clone();
        bad[5].valid = false;
        assert!(plan_activity(origin, 0, 32_000, &bad).is_none());
        let mut bad = good.clone();
        bad.swap(5, 6);
        assert!(plan_activity(origin, 0, 32_000, &bad).is_none());
        let mut bad = good.clone();
        bad[5].channel_id = Some("another-room".into());
        assert!(plan_activity(origin, 0, 32_000, &bad).is_none());
        let mut bad = good.clone();
        bad[5].generation = 2;
        assert!(plan_activity(origin, 0, 32_000, &bad).is_none());
        assert!(plan_activity(origin, 0, 32_000, &good[1..]).is_none());
    }
}
