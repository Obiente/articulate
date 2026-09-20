//! Authenticated native receive audio. Metadata alone never enables this source.
use super::Observation;
use crate::discord_attribution::{Attribution, NamedSpeaker};
use anyhow::{Result, ensure};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const MAX_PACKET: usize = 64 + 5760 * 2 * 2;
const MAX_QUEUE_BYTES: usize = 8 * 1024 * 1024;
const LEASE: Duration = Duration::from_secs(2);

pub struct Frame {
    /// Capture callback time, mapped to the same monotonic clock as microphone capture.
    pub at: Instant,
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
    pub attribution: Attribution,
}

struct Active {
    id: u64,
    channel: String,
    observation_generation: u64,
    native_generation: Option<u64>,
    sequence: u64,
    participants: HashMap<u64, NamedSpeaker>,
    queue: VecDeque<Frame>,
    bytes: usize,
    error: Option<&'static str>,
}
#[derive(Default)]
struct State {
    polled: Option<Instant>,
    active: Option<Active>,
}
#[derive(Clone, Default)]
pub struct Hub(Arc<Mutex<State>>);
pub struct Capture {
    hub: Hub,
    id: u64,
}

fn participants(observation: &Observation) -> Result<HashMap<u64, NamedSpeaker>> {
    let mut names = HashMap::new();
    for p in observation.participants.iter().filter(|p| !p.is_self) {
        let id =
            p.id.parse::<u64>()
                .map_err(|_| anyhow::anyhow!("Invalid native participant identity"))?;
        ensure!(
            id != 0 && id.to_string() == p.id && !names.contains_key(&id),
            "Ambiguous native participant identity"
        );
        names.insert(
            id,
            NamedSpeaker {
                id: p.id.clone(),
                name: p.name.clone(),
                avatar: p.avatar.clone(),
            },
        );
    }
    Ok(names)
}
fn usable(observation: &Observation) -> bool {
    observation.valid
        && observation.at.elapsed() <= Duration::from_millis(750)
        && observation
            .channel_id
            .as_ref()
            .is_some_and(|id| !id.is_empty())
}

impl Hub {
    pub fn ready(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .polled
            .is_some_and(|at| at.elapsed() <= LEASE)
    }
    pub fn begin(&self, observation: &Observation) -> Result<Capture> {
        ensure!(
            usable(observation),
            "Discord voice membership is unavailable"
        );
        let id = u64::from_str_radix(&super::plugin::new_token()?[..16], 16)?.max(1);
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(
            state.polled.is_some_and(|at| at.elapsed() <= LEASE),
            "The native Discord audio adapter is unavailable"
        );
        ensure!(
            state.active.is_none(),
            "Discord audio is already being captured"
        );
        state.active = Some(Active {
            id,
            channel: observation.channel_id.clone().unwrap(),
            observation_generation: observation.generation,
            native_generation: None,
            sequence: 0,
            participants: participants(observation)?,
            queue: VecDeque::new(),
            bytes: 0,
            error: None,
        });
        Ok(Capture {
            hub: self.clone(),
            id,
        })
    }
    /// Only the loaded native adapter polls this authenticated control route.
    pub fn control(&self, observation: Option<&Observation>) -> serde_json::Value {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.polled = Some(Instant::now());
        let inactive = serde_json::json!({"version":1,"active":false,"capture_id":"0","channel_id":null,"participants":[]});
        let Some(active) = state.active.as_mut() else {
            return inactive;
        };
        let valid = observation.filter(|o| {
            usable(o)
                && o.generation == active.observation_generation
                && o.channel_id.as_deref() == Some(active.channel.as_str())
        });
        let Some(observation) = valid else {
            active.error =
                Some("Discord voice connection changed. Finish this capture and start a new one.");
            return inactive;
        };
        if active.error.is_some() {
            return inactive;
        }
        let Ok(names) = participants(observation) else {
            active.error =
                Some("Discord supplied an invalid participant identity. Capture stopped.");
            return inactive;
        };
        active.participants = names;
        let mut ids: Vec<_> = active.participants.keys().map(u64::to_string).collect();
        ids.sort();
        serde_json::json!({"version":1,"active":true,"capture_id":active.id.to_string(),
            "channel_id":active.channel,"participants":ids})
    }
    pub fn receive(&self, bytes: &[u8], now_us: u64, received: Instant) -> Result<()> {
        let packet = Packet::decode(bytes)?;
        ensure!(
            packet.observed <= now_us.saturating_add(50_000)
                && now_us.saturating_sub(packet.observed) <= 2_000_000,
            "Native audio packet is stale"
        );
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(
            state.polled.is_some_and(|at| at.elapsed() <= LEASE),
            "Native capture lease expired"
        );
        let active = state
            .active
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Audio capture is not armed"))?;
        ensure!(
            active.id == packet.capture,
            "Packet belongs to a different capture"
        );
        ensure!(active.error.is_none(), "Native capture has stopped");
        let speaker = active
            .participants
            .get(&packet.user)
            .ok_or_else(|| anyhow::anyhow!("Participant is not in this capture"))?
            .clone();
        if packet.sequence != active.sequence.saturating_add(1) {
            active.error = Some(
                "Discord audio packets were lost or reordered. Capture stopped to avoid an incomplete transcript.",
            );
            anyhow::bail!(active.error.unwrap());
        }
        if active
            .native_generation
            .is_some_and(|generation| generation != packet.generation)
        {
            active.error = Some(
                "Discord reconnected its audio stream. Finish this capture and start a new one.",
            );
            anyhow::bail!(active.error.unwrap());
        }
        if active.bytes + packet.pcm.len() > MAX_QUEUE_BYTES || active.queue.len() >= 4096 {
            active.error = Some(
                "Discord audio processing fell behind. Capture stopped; completed text is retained.",
            );
            anyhow::bail!(active.error.unwrap());
        }
        active.native_generation = Some(packet.generation);
        active.sequence = packet.sequence;
        active.bytes += packet.pcm.len();
        active.queue.push_back(Frame {
            at: received
                .checked_sub(Duration::from_micros(
                    now_us.saturating_sub(packet.observed),
                ))
                .unwrap_or(received),
            rate: packet.rate,
            channels: packet.channels,
            samples: packet
                .pcm
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes([b[0], b[1]]))
                .collect(),
            attribution: Attribution {
                generation: active.observation_generation,
                channel_id: active.channel.clone(),
                speakers: vec![speaker],
            },
        });
        Ok(())
    }
}
impl Capture {
    pub fn drain(&self) -> Result<Vec<Frame>> {
        let mut state = self.hub.0.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(
            state.polled.is_some_and(|at| at.elapsed() <= LEASE),
            "The native Discord audio adapter disconnected"
        );
        let active = state
            .active
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Native audio capture ended"))?;
        ensure!(active.id == self.id, "Native audio capture changed");
        if let Some(error) = active.error {
            anyhow::bail!(error);
        }
        active.bytes = 0;
        Ok(active.queue.drain(..).collect())
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        let mut state = self.hub.0.lock().unwrap_or_else(|e| e.into_inner());
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.id == self.id)
        {
            state.active = None;
        }
    }
}

struct Packet<'a> {
    generation: u64,
    sequence: u64,
    observed: u64,
    user: u64,
    rate: u32,
    channels: u16,
    capture: u64,
    pcm: &'a [u8],
}
impl<'a> Packet<'a> {
    fn decode(bytes: &'a [u8]) -> Result<Self> {
        ensure!(
            (64..=MAX_PACKET).contains(&bytes.len()),
            "Invalid native audio packet size"
        );
        let u16_at = |at| u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
        let u32_at = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let u64_at = |at| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        ensure!(
            &bytes[..4] == b"APCM"
                && u16_at(4) == 1
                && u16_at(6) == 64
                && u16_at(46) == 0
                && bytes[60..64] == [0; 4],
            "Unsupported native audio format"
        );
        let channels = u16_at(44);
        let frames = u32_at(48);
        let rate = u32_at(40);
        ensure!(
            (1..=2).contains(&channels)
                && (1..=5760).contains(&frames)
                && (8000..=96000).contains(&rate)
                && bytes.len() == 64 + frames as usize * channels as usize * 2,
            "Invalid native PCM dimensions"
        );
        Ok(Self {
            generation: u64_at(8),
            sequence: u64_at(16),
            observed: u64_at(24),
            user: u64_at(32),
            rate,
            channels,
            capture: u64_at(52),
            pcm: &bytes[64..],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn observation() -> Observation {
        Observation {
            at: Instant::now(),
            generation: 1,
            channel_id: Some("123".into()),
            valid: true,
            participants: vec![super::super::Participant {
                id: "456".into(),
                name: "Example".into(),
                avatar: None,
                speaking: true,
                is_self: false,
            }],
        }
    }
    fn packet(id: u64, user: u64, sequence: u64) -> Vec<u8> {
        let mut bytes = vec![0; 68];
        bytes[..4].copy_from_slice(b"APCM");
        for (at, value) in [(4, 1_u16), (6, 64), (44, 1)] {
            bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
        }
        for (at, value) in [(8, 1_u64), (16, sequence), (24, 1000), (32, user), (52, id)] {
            bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
        }
        bytes[40..44].copy_from_slice(&16000_u32.to_le_bytes());
        bytes[48..52].copy_from_slice(&2_u32.to_le_bytes());
        bytes[64..68].copy_from_slice(&[1, 0, 255, 255]);
        bytes
    }
    #[test]
    fn audio_requires_native_lease_explicit_capture_and_exact_participant() {
        let hub = Hub::default();
        let observation = observation();
        assert!(hub.begin(&observation).is_err());
        hub.control(Some(&observation));
        let capture = hub.begin(&observation).unwrap();
        assert!(
            hub.receive(&packet(capture.id + 1, 456, 1), 1000, Instant::now())
                .is_err()
        );
        assert!(
            hub.receive(&packet(capture.id, 789, 1), 1000, Instant::now())
                .is_err()
        );
        hub.receive(&packet(capture.id, 456, 1), 1000, Instant::now())
            .unwrap();
        let frames = capture.drain().unwrap();
        assert_eq!(frames[0].samples, [1, -1]);
        assert_eq!(frames[0].attribution.speakers[0].name, "Example");
        drop(capture);
        assert!(
            hub.receive(&packet(1, 456, 2), 1000, Instant::now())
                .is_err()
        );
    }
    #[test]
    fn loss_and_connection_changes_stop_instead_of_silently_mixing() {
        let hub = Hub::default();
        let mut observation = observation();
        hub.control(Some(&observation));
        let capture = hub.begin(&observation).unwrap();
        assert!(
            hub.receive(&packet(capture.id, 456, 2), 1000, Instant::now())
                .is_err()
        );
        assert!(capture.drain().is_err());
        drop(capture);
        let capture = hub.begin(&observation).unwrap();
        observation.channel_id = Some("789".into());
        assert_eq!(hub.control(Some(&observation))["active"], false);
        assert!(capture.drain().is_err());
    }
    #[test]
    fn binary_parser_rejects_truncation_dimensions_reserved_and_stale_audio() {
        let good = packet(1, 456, 1);
        assert!(Packet::decode(&good).is_ok());
        for end in 0..good.len() {
            assert!(Packet::decode(&good[..end]).is_err());
        }
        for offset in [0, 4, 6, 44, 46, 48, 60] {
            let mut bad = good.clone();
            bad[offset] = 255;
            assert!(Packet::decode(&bad).is_err());
        }
        assert!(
            Hub::default()
                .receive(&good, 3_000_000, Instant::now())
                .is_err()
        );
    }

    #[test]
    fn queue_overflow_is_bounded_and_stops_capture() {
        let hub = Hub::default();
        let observation = observation();
        hub.control(Some(&observation));
        let capture = hub.begin(&observation).unwrap();
        let mut bytes = packet(capture.id, 456, 1);
        bytes.resize(MAX_PACKET, 0);
        bytes[44..46].copy_from_slice(&2_u16.to_le_bytes());
        bytes[48..52].copy_from_slice(&5760_u32.to_le_bytes());
        let payload = bytes.len() - 64;
        let count = MAX_QUEUE_BYTES / payload;
        for index in 0..count {
            bytes[16..24].copy_from_slice(&(index as u64 + 1).to_le_bytes());
            hub.receive(&bytes, 1000, Instant::now()).unwrap();
        }
        bytes[16..24].copy_from_slice(&(count as u64 + 1).to_le_bytes());
        assert!(hub.receive(&bytes, 1000, Instant::now()).is_err());
        {
            let state = hub.0.lock().unwrap();
            let active = state.active.as_ref().unwrap();
            assert!(active.bytes <= MAX_QUEUE_BYTES);
            assert_eq!(active.queue.len(), count);
        }
        assert!(capture.drain().is_err());
        drop(capture);
        assert!(hub.0.lock().unwrap().active.is_none());
    }

    #[test]
    fn native_generation_and_lease_cannot_change_silently() {
        let hub = Hub::default();
        let observation = observation();
        hub.control(Some(&observation));
        let capture = hub.begin(&observation).unwrap();
        hub.receive(&packet(capture.id, 456, 1), 1000, Instant::now())
            .unwrap();
        capture.drain().unwrap();
        let mut changed = packet(capture.id, 456, 2);
        changed[8..16].copy_from_slice(&2_u64.to_le_bytes());
        assert!(hub.receive(&changed, 1000, Instant::now()).is_err());
        assert!(capture.drain().is_err());
        drop(capture);
        let capture = hub.begin(&observation).unwrap();
        hub.0.lock().unwrap().polled = Instant::now().checked_sub(LEASE + Duration::from_millis(1));
        assert!(!hub.ready());
        assert!(
            hub.receive(&packet(capture.id, 456, 1), 1000, Instant::now())
                .is_err()
        );
        assert!(capture.drain().is_err());
    }

    #[test]
    fn native_membership_rejects_noncanonical_overflow_and_duplicate_ids() {
        for id in ["0", "0456", "+456", "18446744073709551616"] {
            let hub = Hub::default();
            let mut observation = observation();
            observation.participants[0].id = id.into();
            hub.control(Some(&observation));
            assert!(
                hub.begin(&observation).is_err(),
                "Accepted invalid participant {id}"
            );
        }
        let hub = Hub::default();
        let mut observation = observation();
        hub.control(Some(&observation));
        let capture = hub.begin(&observation).unwrap();
        observation
            .participants
            .push(observation.participants[0].clone());
        assert_eq!(hub.control(Some(&observation))["active"], false);
        assert!(capture.drain().is_err());
    }

    #[test]
    fn a_new_capture_rejects_old_nonce_without_poisoning_current_sequence() {
        let hub = Hub::default();
        let observation = observation();
        hub.control(Some(&observation));
        let first = hub.begin(&observation).unwrap();
        let old_id = first.id;
        hub.receive(&packet(old_id, 456, 1), 1000, Instant::now())
            .unwrap();
        drop(first);
        let second = hub.begin(&observation).unwrap();
        assert_ne!(second.id, old_id);
        assert!(
            hub.receive(&packet(old_id, 456, 2), 1000, Instant::now())
                .is_err()
        );
        hub.receive(&packet(second.id, 456, 1), 1000, Instant::now())
            .unwrap();
        assert_eq!(second.drain().unwrap().len(), 1);
    }
}
