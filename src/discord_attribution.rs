//! Conservative interval attribution. Activity is evidence for an existing
//! acoustic turn, never a source of word timings or a durable voiceprint.
use crate::{calls::Row, discord::Observation};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedSpeaker {
    #[serde(skip)]
    pub id: String,
    pub name: String,
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
