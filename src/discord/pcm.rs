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
// Allow a stalled renderer or local transport to resume without ending the
// microphone recording. Audio remains disarmed until fresh controls arrive.
const RECOVERY: Duration = Duration::from_secs(15);
const MAX_RETIRED_PARTICIPANTS: usize = 512;

pub struct Frame {
    /// Capture callback time, mapped to the same monotonic clock as microphone capture.
    pub at: Instant,
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
    pub attribution: Attribution,
    /// Identity of the native Connect callback that supplied this person's PCM.
    pub native_generation: u64,
    /// Global loss count when this frame arrived. A change invalidates jitter
    /// compensation for every participant until their next frame.
    pub loss_epoch: u64,
}

struct RetiredParticipant {
    speaker: NamedSpeaker,
    removed: Instant,
}

struct Active {
    diagnostics: Option<Arc<Mutex<super::diagnostics::Recorder>>>,
    id: u64,
    channel: String,
    observation_generation: u64,
    native_generations: HashMap<u64, u64>,
    sequence: u64,
    lost_packets: u64,
    participants: HashMap<u64, NamedSpeaker>,
    retired: HashMap<u64, RetiredParticipant>,
    queue: VecDeque<Frame>,
    bytes: usize,
    error: Option<&'static str>,
    authorized: Option<Instant>,
    armed: bool,
    interrupted: bool,
}
#[derive(Default)]
struct State {
    diagnostics: Option<Arc<Mutex<super::diagnostics::Recorder>>>,
    polled: Option<Instant>,
    active: Option<Active>,
}
impl Active {
    fn diagnose(&self, reason: &'static str, incoming: u64) {
        if let Some(recorder) = &self.diagnostics {
            recorder.lock().unwrap_or_else(|e| e.into_inner()).failure(
                reason,
                self.sequence,
                incoming,
                self.lost_packets,
                self.queue.len(),
                self.bytes,
            );
        }
    }
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
    pub(super) fn with_diagnostics(path: std::path::PathBuf) -> Self {
        Self(Arc::new(Mutex::new(State {
            diagnostics: Some(Arc::new(Mutex::new(super::diagnostics::Recorder::new(
                path,
            )))),
            ..State::default()
        })))
    }
    pub(super) fn report_diagnostics(&self, bytes: &[u8]) -> Result<()> {
        let report = super::diagnostics::validate(bytes)?;
        let recorder = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .diagnostics
            .clone();
        if let Some(recorder) = recorder {
            recorder
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .companion(report);
        }
        Ok(())
    }
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
            diagnostics: state.diagnostics.clone(),
            id,
            channel: observation.channel_id.clone().unwrap(),
            observation_generation: observation.generation,
            native_generations: HashMap::new(),
            sequence: 0,
            lost_packets: 0,
            participants: participants(observation)?,
            retired: HashMap::new(),
            queue: VecDeque::new(),
            bytes: 0,
            error: None,
            authorized: Some(Instant::now()),
            armed: true,
            interrupted: false,
        });
        Ok(Capture {
            hub: self.clone(),
            id,
        })
    }
    /// Only the loaded native adapter polls this authenticated control route.
    pub fn control(&self, observation: Option<&Observation>) -> serde_json::Value {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let previous_poll = state.polled;
        state.polled = Some(Instant::now());
        let inactive = serde_json::json!({"version":1,"pcm_batch":true,"active":false,"capture_id":"0","channel_id":null,"participants":[]});
        let Some(active) = state.active.as_mut() else {
            return inactive;
        };
        if active.authorized.is_none_or(|at| at.elapsed() > RECOVERY) {
            active.error = Some("The native Discord audio adapter disconnected");
            active.armed = false;
            return inactive;
        }
        let valid = observation.filter(|o| {
            usable(o)
                && o.generation == active.observation_generation
                && o.channel_id.as_deref() == Some(active.channel.as_str())
        });
        let Some(observation) = valid else {
            // Missing or stale metadata is temporary. A verified different
            // channel is permanent and must never inherit this capture.
            if observation.is_some_and(usable) {
                active.error = Some(
                    "Discord voice connection changed. Finish this capture and start a new one.",
                );
            }
            active.armed = false;
            if !active.interrupted {
                active.lost_packets = active.lost_packets.saturating_add(1);
                active.interrupted = true;
            }
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
        // Metadata can observe a departure before previously captured audio has
        // crossed the transport. Keep only a short, bounded identity history so
        // those packets retain their speaker and do not break the global sequence.
        let now = Instant::now();
        active.retired.retain(|id, entry| {
            now.saturating_duration_since(entry.removed) <= LEASE && !names.contains_key(id)
        });
        for (id, speaker) in &active.participants {
            if !names.contains_key(id) {
                if active.retired.len() >= MAX_RETIRED_PARTICIPANTS {
                    active.error =
                        Some("Discord membership changed too quickly. Start a new capture.");
                    return inactive;
                }
                active.retired.insert(
                    *id,
                    RetiredParticipant {
                        speaker: speaker.clone(),
                        removed: now,
                    },
                );
            }
        }
        active
            .native_generations
            .retain(|id, _| names.contains_key(id) || active.retired.contains_key(id));
        active.participants = names;
        if previous_poll.is_some_and(|at| at.elapsed() > LEASE) && !active.interrupted {
            active.lost_packets = active.lost_packets.saturating_add(1);
        }
        active.authorized = Some(Instant::now());
        active.armed = true;
        active.interrupted = false;
        let mut ids: Vec<_> = active.participants.keys().map(u64::to_string).collect();
        ids.sort();
        serde_json::json!({"version":1,"pcm_batch":true,"active":true,"capture_id":active.id.to_string(),
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
        ensure!(
            active.armed && active.authorized.is_some_and(|at| at.elapsed() <= LEASE),
            "Native capture is awaiting fresh Discord membership"
        );
        ensure!(active.error.is_none(), "Native capture has stopped");
        let captured_at = received
            .checked_sub(Duration::from_micros(
                now_us.saturating_sub(packet.observed),
            ))
            .unwrap_or(received);
        let retired = active
            .retired
            .get(&packet.user)
            .filter(|entry| received.saturating_duration_since(entry.removed) <= LEASE);
        let speaker = active
            .participants
            .get(&packet.user)
            .or_else(|| retired.map(|entry| &entry.speaker))
            .ok_or_else(|| anyhow::anyhow!("Participant is not in this capture"))?
            .clone();
        if packet.sequence <= active.sequence {
            active.diagnose("sequence_order", packet.sequence);
            active.error = Some(
                "Discord audio arrived out of order. Capture stopped; completed text is retained.",
            );
            anyhow::bail!(active.error.unwrap());
        }
        let missing = packet.sequence - active.sequence - 1;
        if missing > 0 {
            active.lost_packets = active.lost_packets.saturating_add(missing);
            // Fresh, ordered audio from the same authorized capture means the
            // stream has resumed. Preserve the loss count and wall-clock gap;
            // stopping here would discard the rest of an otherwise usable call.
            // Replay, channel changes, expired leases and queue bounds remain
            // separate errors and cannot be bypassed by claiming a sequence gap.
            active.diagnose("sequence_loss", packet.sequence);
        }
        // The native allowlist update can race one last callback after removal.
        // Acknowledge its sequence without recording audio outside membership.
        if retired.is_some_and(|entry| captured_at > entry.removed) {
            active.sequence = packet.sequence;
            return Ok(());
        }
        // The native serial identifies one Connect callback, not a whole call.
        // Different participants can use different callbacks concurrently. Once
        // this person moves to a newer callback, acknowledge queued old packets
        // without duplicating their audio or rolling their stream back.
        if active
            .native_generations
            .get(&packet.user)
            .is_some_and(|generation| packet.generation < *generation)
        {
            active.sequence = packet.sequence;
            return Ok(());
        }
        if active.bytes + packet.pcm.len() > MAX_QUEUE_BYTES || active.queue.len() >= 4096 {
            active.diagnose("receiver_queue_full", packet.sequence);
            active.error = Some(
                "Discord audio processing fell behind. Capture stopped; completed text is retained.",
            );
            anyhow::bail!(active.error.unwrap());
        }
        active
            .native_generations
            .insert(packet.user, packet.generation);
        active.sequence = packet.sequence;
        active.bytes += packet.pcm.len();
        active.queue.push_back(Frame {
            native_generation: packet.generation,
            loss_epoch: active.lost_packets,
            at: captured_at,
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
    pub fn lost_packets(&self) -> u64 {
        self.hub
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active
            .as_ref()
            .filter(|active| active.id == self.id)
            .map_or(0, |active| active.lost_packets)
    }
    pub fn drain(&self) -> Result<Vec<Frame>> {
        let mut state = self.hub.0.lock().unwrap_or_else(|e| e.into_inner());
        let polled = state.polled;
        let active = state
            .active
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Native audio capture ended"))?;
        ensure!(active.id == self.id, "Native audio capture changed");
        if let Some(error) = active.error {
            anyhow::bail!(error);
        }
        ensure!(
            active.authorized.is_some_and(|at| at.elapsed() <= RECOVERY)
                && polled.is_some_and(|at| at.elapsed() <= RECOVERY),
            "The native Discord audio adapter disconnected"
        );
        if (!polled.is_some_and(|at| at.elapsed() <= LEASE)
            || !active.armed
            || !active.authorized.is_some_and(|at| at.elapsed() <= LEASE))
            && !active.interrupted
        {
            active.lost_packets = active.lost_packets.saturating_add(1);
            active.interrupted = true;
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
                && bytes[60..64] == [0; 4]
                && u64_at(8) != 0,
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
    fn departing_participant_does_not_interrupt_other_streams() {
        let hub = Hub::default();
        let mut observation = observation();
        let mut other = observation.participants[0].clone();
        other.id = "789".into();
        other.name = "Another speaker".into();
        observation.participants.push(other);
        hub.control(Some(&observation));
        let capture = hub.begin(&observation).unwrap();
        let before_departure = Instant::now();
        observation.participants.remove(0);
        hub.control(Some(&observation));
        // This packet was captured while the participant was still in the call.
        hub.receive(&packet(capture.id, 456, 1), 1000, before_departure)
            .unwrap();
        let frames = capture.drain().unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].attribution.speakers[0].id, "456");
        // A callback racing the native allowlist update is ignored, but its
        // sequence must still be consumed so the remaining speaker can continue.
        hub.receive(&packet(capture.id, 456, 2), 1000, Instant::now())
            .unwrap();
        hub.receive(&packet(capture.id, 789, 3), 1000, Instant::now())
            .unwrap();
        let frames = capture.drain().unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].attribution.speakers[0].id, "789");
        // Removed identities expire and cannot be reused as indefinite membership.
        hub.0
            .lock()
            .unwrap()
            .active
            .as_mut()
            .unwrap()
            .retired
            .get_mut(&456)
            .unwrap()
            .removed = Instant::now() - LEASE - Duration::from_millis(1);
        assert!(
            hub.receive(&packet(capture.id, 456, 4), 1000, Instant::now())
                .is_err()
        );
    }

    #[test]
    fn large_gap_recovers_but_channel_changes_still_stop() {
        let hub = Hub::default();
        let mut observation = observation();
        hub.control(Some(&observation));
        let capture = hub.begin(&observation).unwrap();
        let now = Instant::now();
        hub.receive(&packet(capture.id, 456, 1), 1000, now).unwrap();
        hub.receive(
            &packet(capture.id, 456, 152),
            1000,
            now + Duration::from_secs(1),
        )
        .unwrap();
        hub.receive(
            &packet(capture.id, 456, 153),
            1000,
            now + Duration::from_millis(1020),
        )
        .unwrap();
        let frames = capture.drain().unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(
            frames.iter().map(|f| f.loss_epoch).collect::<Vec<_>>(),
            [0, 150, 150]
        );
        assert_eq!(
            frames[1].at.duration_since(frames[0].at),
            Duration::from_secs(1)
        );
        assert_eq!(capture.lost_packets(), 150);
        drop(capture);
        let capture = hub.begin(&observation).unwrap();
        observation.channel_id = Some("789".into());
        assert_eq!(hub.control(Some(&observation))["active"], false);
        assert!(capture.drain().is_err());
    }
    #[test]
    fn brief_loss_is_visible_and_preserves_each_participant_and_timestamp() {
        let hub = Hub::default();
        let mut observation = observation();
        let mut other = observation.participants[0].clone();
        other.id = "789".into();
        observation.participants.push(other);
        assert_eq!(hub.control(Some(&observation))["pcm_batch"], true);
        let capture = hub.begin(&observation).unwrap();
        assert_eq!(hub.control(Some(&observation))["pcm_batch"], true);
        let now = Instant::now();
        hub.receive(&packet(capture.id, 456, 1), 1000, now).unwrap();
        hub.receive(
            &packet(capture.id, 789, 3),
            1000,
            now + Duration::from_millis(20),
        )
        .unwrap();
        hub.receive(
            &packet(capture.id, 456, 4),
            1000,
            now + Duration::from_millis(30),
        )
        .unwrap();
        let frames = capture.drain().unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(
            frames.iter().map(|f| f.loss_epoch).collect::<Vec<_>>(),
            [0, 1, 1]
        );
        assert_eq!(frames[1].attribution.speakers[0].id, "789");
        assert_eq!(frames[2].attribution.speakers[0].id, "456");
        assert_eq!(
            frames[2].at.duration_since(frames[0].at),
            Duration::from_millis(30)
        );
        assert_eq!(capture.lost_packets(), 1);
        // A real replay is not a recoverable loss and must never duplicate text.
        assert!(hub.receive(&packet(capture.id, 456, 4), 1000, now).is_err());
        assert!(capture.drain().is_err());
        drop(capture);
        assert_eq!(hub.begin(&observation).unwrap().lost_packets(), 0);
    }

    #[test]
    fn repeated_gaps_remain_visible_without_discarding_subsequent_audio() {
        let hub = Hub::default();
        let observation = observation();
        hub.control(Some(&observation));
        let capture = hub.begin(&observation).unwrap();
        let now = Instant::now();
        for index in 1..=100 {
            hub.receive(&packet(capture.id, 456, index * 2), 1000, now)
                .unwrap();
            let frames = capture.drain().unwrap();
            assert_eq!(frames.len(), 1);
            assert_eq!(frames[0].loss_epoch, index);
        }
        assert_eq!(capture.lost_packets(), 100);
    }

    #[test]
    fn remote_network_silence_and_replaced_voice_callback_keep_the_same_capture() {
        let hub = Hub::default();
        let mut observation = observation();
        hub.control(Some(&observation));
        let capture = hub.begin(&observation).unwrap();
        let start = Instant::now();
        hub.receive(&packet(capture.id, 456, 1), 1000, start)
            .unwrap();
        let before = capture.drain().unwrap().remove(0);
        // Discord remains in this channel and the local control connection is
        // healthy, but remote network audio disappears for thirty seconds.
        for _ in 0..120 {
            observation.at = Instant::now();
            let control = hub.control(Some(&observation));
            assert_eq!(control["active"], true);
            assert_eq!(control["capture_id"], capture.id.to_string());
            assert!(capture.drain().unwrap().is_empty());
        }
        let mut resumed = packet(capture.id, 456, 2);
        resumed[8..16].copy_from_slice(&2_u64.to_le_bytes());
        hub.receive(&resumed, 1000, start + Duration::from_secs(30))
            .unwrap();
        let after = capture.drain().unwrap().remove(0);
        assert_eq!(after.native_generation, 2);
        assert_eq!(after.attribution.speakers, before.attribution.speakers);
        assert_eq!(after.at.duration_since(before.at), Duration::from_secs(30));
        assert_eq!(
            capture.lost_packets(),
            0,
            "A network silence is not a missing local packet"
        );
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
    fn native_lease_expiry_disarms_packets_but_allows_recovery() {
        let hub = Hub::default();
        let observation = observation();
        hub.control(Some(&observation));
        let capture = hub.begin(&observation).unwrap();
        hub.0.lock().unwrap().polled = Instant::now().checked_sub(LEASE + Duration::from_millis(1));
        assert!(!hub.ready());
        assert!(
            hub.receive(&packet(capture.id, 456, 1), 1000, Instant::now())
                .is_err()
        );
        assert!(capture.drain().unwrap().is_empty());
        assert_eq!(capture.lost_packets(), 1);
        hub.control(Some(&observation));
        assert!(
            hub.receive(&packet(capture.id, 456, 1), 1000, Instant::now())
                .is_ok()
        );
        assert_eq!(capture.drain().unwrap().len(), 1);
    }

    #[test]
    fn missing_metadata_recovers_only_for_the_same_voice_session() {
        let hub = Hub::default();
        let observation = observation();
        hub.control(Some(&observation));
        let capture = hub.begin(&observation).unwrap();
        assert!(!hub.control(None)["active"].as_bool().unwrap());
        assert!(
            hub.receive(&packet(capture.id, 456, 1), 1000, Instant::now())
                .is_err()
        );
        assert!(capture.drain().unwrap().is_empty());
        assert!(hub.control(Some(&observation))["active"].as_bool().unwrap());
        assert!(
            hub.receive(&packet(capture.id, 456, 1), 1000, Instant::now())
                .is_ok()
        );
        let mut changed = observation.clone();
        changed.generation += 1;
        assert!(!hub.control(Some(&changed))["active"].as_bool().unwrap());
        assert!(capture.drain().is_err());
    }

    #[test]
    fn prolonged_adapter_outage_stops_capture() {
        let hub = Hub::default();
        let observation = observation();
        hub.control(Some(&observation));
        let capture = hub.begin(&observation).unwrap();
        hub.0.lock().unwrap().active.as_mut().unwrap().authorized =
            Instant::now().checked_sub(RECOVERY + Duration::from_millis(1));
        assert!(!hub.control(Some(&observation))["active"].as_bool().unwrap());
        assert!(capture.drain().is_err());
    }

    #[test]
    fn callback_generations_are_per_participant_and_retired_streams_cannot_return() {
        let hub = Hub::default();
        let mut observation = observation();
        let mut other = observation.participants[0].clone();
        other.id = "789".into();
        other.name = "Another speaker".into();
        observation.participants.push(other);
        hub.control(Some(&observation));
        let capture = hub.begin(&observation).unwrap();
        // The second person can still use an older native callback. A replacement
        // for the first person must neither stop nor retire that other stream.
        let deliveries = [
            (456, 7_u64),
            (789, 3),
            (456, 9),
            (456, 7),
            (789, 3),
            (456, 9),
        ];
        for (index, (user, generation)) in deliveries.into_iter().enumerate() {
            let mut bytes = packet(capture.id, user, index as u64 + 1);
            bytes[8..16].copy_from_slice(&generation.to_le_bytes());
            bytes[64..66].copy_from_slice(&(index as i16 + 1).to_le_bytes());
            hub.receive(&bytes, 1000, Instant::now()).unwrap();
        }
        let frames = capture.drain().unwrap();
        assert_eq!(frames.len(), 5);
        assert_eq!(
            frames
                .iter()
                .map(|frame| frame.samples[0])
                .collect::<Vec<_>>(),
            [1, 2, 3, 5, 6]
        );
        assert_eq!(
            frames
                .iter()
                .map(|frame| frame.native_generation)
                .collect::<Vec<_>>(),
            [7, 3, 9, 3, 9]
        );
        assert_eq!(
            frames
                .iter()
                .map(|frame| frame.attribution.speakers[0].id.as_str())
                .collect::<Vec<_>>(),
            ["456", "789", "456", "789", "456"]
        );
        assert_eq!(capture.lost_packets(), 0);
        // A real channel change still invalidates this capture and its identities.
        observation.channel_id = Some("999".into());
        assert_eq!(hub.control(Some(&observation))["active"], false);
        assert!(capture.drain().is_err());
    }

    #[test]
    fn zero_callback_generation_is_invalid_and_does_not_poison_capture() {
        let hub = Hub::default();
        let observation = observation();
        hub.control(Some(&observation));
        let capture = hub.begin(&observation).unwrap();
        let mut invalid = packet(capture.id, 456, 1);
        invalid[8..16].fill(0);
        assert!(hub.receive(&invalid, 1000, Instant::now()).is_err());
        hub.receive(&packet(capture.id, 456, 1), 1000, Instant::now())
            .unwrap();
        assert_eq!(capture.drain().unwrap().len(), 1);
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
