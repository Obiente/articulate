//! Separate decoded participant PCM. The mixed output device is never opened here.
use super::{REFRESH_SECONDS, Request, Row, Update, quiet_tail};
use crate::{
    call_capture::Track, discord::pcm::Frame, discord_attribution::Attribution, engine::Engine,
};
use anyhow::{Result, ensure};
use std::{
    collections::VecDeque,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

const MAX_PARTICIPANTS: usize = 16;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_SECONDS: f64 = 24.0;
const CLOCK_JITTER: f64 = 0.030;

struct Voice {
    attribution: Attribution,
    rate: u32,
    start: f64,
    end: f64,
    pcm: Vec<f32>,
}

#[derive(Default)]
struct Window {
    start: f64,
    mic: Vec<f32>,
    voices: Vec<Voice>,
    rows: Vec<Row>,
}

fn frame_start(frame: &Frame) -> Instant {
    frame.at
        - Duration::from_secs_f64(
            frame.samples.len() as f64 / f64::from(frame.channels) / f64::from(frame.rate),
        )
}

impl Window {
    fn bytes(&self) -> usize {
        (self.mic.len()
            + self
                .voices
                .iter()
                .map(|voice| voice.pcm.len())
                .sum::<usize>())
            * 4
    }

    fn ingest(&mut self, frame: Frame, origin: Instant) -> Result<()> {
        ensure!(
            frame.attribution.speakers.len() == 1 && !frame.attribution.speakers[0].id.is_empty(),
            "Native audio has no unique participant identity"
        );
        ensure!(
            (1..=2).contains(&frame.channels)
                && (8000..=96000).contains(&frame.rate)
                && !frame.samples.is_empty()
                && frame
                    .samples
                    .len()
                    .is_multiple_of(usize::from(frame.channels)),
            "Invalid participant audio format"
        );
        let raw_start = frame_start(&frame);
        let skip = if raw_start < origin {
            (origin.duration_since(raw_start).as_secs_f64() * f64::from(frame.rate)).ceil() as usize
        } else {
            0
        };
        let frames = frame.samples.len() / usize::from(frame.channels);
        if skip >= frames {
            return Ok(());
        }
        let start = raw_start.saturating_duration_since(origin).as_secs_f64();
        let end = frame.at.saturating_duration_since(origin).as_secs_f64();
        ensure!(
            start + 0.001 >= self.start,
            "Participant audio arrived after its transcript section was committed. Capture stopped to preserve the existing transcript."
        );
        let slot = self.voices.iter().position(|voice| {
            voice.attribution.generation == frame.attribution.generation
                && voice.attribution.channel_id == frame.attribution.channel_id
                && voice.attribution.speakers[0].id == frame.attribution.speakers[0].id
        });
        let slot = match slot {
            Some(slot) => slot,
            None => {
                ensure!(
                    self.voices.len() < MAX_PARTICIPANTS,
                    "This capture reached its limit of 16 simultaneous participant streams"
                );
                self.voices.push(Voice {
                    attribution: frame.attribution.clone(),
                    rate: frame.rate,
                    start: start.max(self.start),
                    end: start.max(self.start),
                    pcm: Vec::new(),
                });
                self.voices.len() - 1
            }
        };
        let current_bytes = self.bytes();
        let voice = &mut self.voices[slot];
        ensure!(
            voice.rate == frame.rate,
            "A participant changed audio format. Finish this capture and start another."
        );
        // Callback clock jitter must not discard or overwrite decoded samples.
        // A real silence remains a gap, preserving each participant's timeline.
        ensure!(
            start + CLOCK_JITTER >= voice.end,
            "Participant audio timestamps moved backwards"
        );
        let gap = if start - voice.end > CLOCK_JITTER {
            ((start - voice.end) * f64::from(voice.rate)).round() as usize
        } else {
            0
        };
        let additional = gap
            .checked_add(frames - skip)
            .ok_or_else(|| anyhow::anyhow!("Participant audio exceeded its memory limit"))?;
        ensure!(
            voice.pcm.len() + additional <= frame.rate as usize * 25
                && current_bytes + additional * 4 <= MAX_BYTES,
            "Participant audio exceeded its bounded capture buffer. The latest transcript is retained."
        );
        voice.pcm.resize(voice.pcm.len() + gap, 0.0);
        voice.pcm.extend(
            frame.samples[skip * usize::from(frame.channels)..]
                .chunks_exact(usize::from(frame.channels))
                .map(|channels| {
                    channels
                        .iter()
                        .map(|&sample| f32::from(sample) / 32768.0)
                        .sum::<f32>()
                        / f32::from(frame.channels)
                }),
        );
        voice.end = end;
        Ok(())
    }

    fn decode(
        &self,
        end: f64,
        mut transcribe: impl FnMut(&[f32]) -> Result<String>,
    ) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        let text = transcribe(&self.mic)?;
        if !text.is_empty() {
            rows.push(Row {
                start_ms: (self.start * 1000.0) as u64,
                end_ms: (end * 1000.0) as u64,
                microphone: true,
                speakers: Vec::new(),
                discord: None,
                text,
            });
        }
        for voice in &self.voices {
            // Resample contiguous accumulated speech, never individual 20 ms packets.
            let pcm = crate::audio::resample(&voice.pcm, voice.rate)?;
            let text = transcribe(&pcm)?;
            if !text.is_empty() {
                rows.push(Row {
                    start_ms: (voice.start * 1000.0) as u64,
                    end_ms: (voice.end * 1000.0) as u64,
                    microphone: false,
                    speakers: Vec::new(),
                    discord: Some(voice.attribution.clone()),
                    text,
                });
            }
        }
        rows.sort_by_key(|row| row.start_ms);
        Ok(rows)
    }

    fn endpoint(&self, end: f64) -> bool {
        end - self.start >= MAX_SECONDS - 0.001
            || (quiet_tail(&self.mic)
                && self.voices.iter().all(|voice| {
                    end - voice.end >= 0.6 || {
                        let tail = (voice.rate as usize * 6) / 10;
                        voice.pcm.len() >= tail
                            && voice.pcm[voice.pcm.len() - tail..]
                                .iter()
                                .all(|sample| sample.abs() < 0.003)
                    }
                }))
    }

    fn commit(&mut self, end: f64, update: &mut impl FnMut(Update)) {
        update(Update::Rows(std::mem::take(&mut self.rows)));
        self.mic.clear();
        self.voices.clear();
        self.start = end;
    }
}

/// Consume through a shared boundary, splitting a straddling packet exactly once.
fn consume(
    pending: &mut VecDeque<Frame>,
    until: Instant,
    origin: Instant,
    window: &mut Window,
) -> Result<()> {
    for _ in 0..pending.len() {
        let mut frame = pending.pop_front().expect("bounded queue iteration");
        if frame.at <= until {
            window.ingest(frame, origin)?;
            continue;
        }
        let start = frame_start(&frame);
        if start >= until {
            pending.push_back(frame);
            continue;
        }
        let count =
            (until.duration_since(start).as_secs_f64() * f64::from(frame.rate)).floor() as usize;
        let cut = count * usize::from(frame.channels);
        if cut == 0 {
            pending.push_back(frame);
            continue;
        }
        let remaining = frame.samples.split_off(cut);
        let boundary = start + Duration::from_secs_f64(count as f64 / f64::from(frame.rate));
        let head = Frame {
            at: boundary,
            rate: frame.rate,
            channels: frame.channels,
            samples: frame.samples,
            attribution: frame.attribution.clone(),
        };
        frame.samples = remaining;
        window.ingest(head, origin)?;
        if !frame.samples.is_empty() {
            pending.push_back(frame);
        }
    }
    Ok(())
}

pub(super) fn run(
    engine: &mut Engine,
    request: Request,
    mut update: impl FnMut(Update),
) -> Result<()> {
    if request.control.abort.load(Ordering::Relaxed)
        || request.control.stop_ns.load(Ordering::SeqCst) != 0
    {
        return Ok(());
    }
    let connection = request
        .control
        .discord()
        .ok_or_else(|| anyhow::anyhow!("Discord is not connected"))?;
    let mut mic = Track::open(
        request.microphone.as_deref(),
        false,
        request.control.clone(),
    )?;
    let capture = connection.start_native_audio()?;
    let origin = request
        .control
        .origin()
        .expect("microphone establishes capture clock");
    update(Update::Started);
    update(Update::NativeAudio(false));
    let mut received = false;
    let mut pending = VecDeque::new();
    let mut window = Window::default();
    let mut cursor = 0.0;
    let result = (|| -> Result<()> {
        loop {
            if request.control.abort.load(Ordering::Relaxed) {
                break;
            }
            let frames = capture.drain()?;
            if !received && !frames.is_empty() {
                received = true;
                update(Update::NativeAudio(true));
            }
            pending.extend(frames);
            let queued_bytes = pending
                .iter()
                .map(|frame| frame.samples.len() * 2)
                .sum::<usize>();
            ensure!(
                queued_bytes + window.bytes() <= MAX_BYTES && pending.len() <= 8192,
                "Participant capture fell behind; the latest transcript is retained"
            );
            let stopping = request.control.stop_ns.load(Ordering::SeqCst) != 0;
            let now = request.control.end_seconds();
            // Give already captured packets the same transport allowance as live
            // revisions before finalizing the user's stop boundary.
            if stopping && origin.elapsed().as_secs_f64() < now + 0.25 {
                std::thread::sleep(Duration::from_millis(25));
                continue;
            }
            ensure!(
                now - cursor < 40.0,
                "Call transcription fell more than 40 seconds behind. The latest transcript is retained."
            );
            let ready = if stopping { now } else { (now - 0.25).max(0.0) };
            let remaining = (MAX_SECONDS - (cursor - window.start)).max(0.0);
            if stopping || ready >= cursor + REFRESH_SECONDS.min(remaining) {
                let end = ready.min(cursor + remaining);
                if end <= cursor {
                    break;
                }
                consume(
                    &mut pending,
                    origin + Duration::from_secs_f64(end),
                    origin,
                    &mut window,
                )?;
                window.mic.extend(mic.take_until(end)?);
                let rows = window.decode(end, |pcm| engine.transcribe(pcm))?;
                window.rows = rows;
                cursor = end;
                if window.endpoint(end) || (stopping && cursor >= now) {
                    window.commit(end, &mut update);
                } else {
                    update(Update::Preview(window.rows.clone()));
                }
                if stopping && cursor >= now {
                    break;
                }
            } else {
                let level = window
                    .voices
                    .iter()
                    .filter_map(|voice| voice.pcm.last())
                    .map(|sample| sample.abs())
                    .fold(0.0, f32::max);
                update(Update::Levels(
                    f32::from_bits(mic.level.load(Ordering::Relaxed)),
                    level,
                ));
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        Ok(())
    })();
    if !window.rows.is_empty() {
        window.commit(cursor, &mut update);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(origin: Instant, id: &str, end: f64, samples: usize, value: i16) -> Frame {
        Frame {
            at: origin + Duration::from_secs_f64(end),
            rate: 16000,
            channels: 1,
            samples: vec![value; samples],
            attribution: Attribution {
                generation: 1,
                channel_id: "test-channel".into(),
                speakers: vec![crate::discord_attribution::NamedSpeaker {
                    id: id.into(),
                    name: id.into(),
                    avatar: None,
                }],
            },
        }
    }
    #[test]
    fn overlapping_people_are_decoded_separately_with_exact_identity() {
        let origin = Instant::now();
        let mut window = Window::default();
        window
            .ingest(frame(origin, "Casey", 1.0, 16000, 8192), origin)
            .unwrap();
        window
            .ingest(frame(origin, "Jordan", 1.0, 16000, -8192), origin)
            .unwrap();
        let rows = window
            .decode(1.0, |pcm| {
                Ok(if pcm.is_empty() {
                    ""
                } else if pcm[0] > 0.0 {
                    "Casey sentence."
                } else {
                    "Jordan sentence."
                }
                .into())
            })
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].text, "Casey sentence.");
        assert_eq!(rows[1].text, "Jordan sentence.");
        assert_eq!(rows[0].discord.as_ref().unwrap().speakers[0].id, "Casey");
        assert_eq!(rows[1].discord.as_ref().unwrap().speakers[0].id, "Jordan");
        assert_eq!((rows[0].start_ms, rows[1].start_ms), (0, 0));
        let mut transcript = Vec::new();
        super::super::append_rows(&mut transcript, rows);
        assert_eq!(transcript.len(), 2);
        let names = Default::default();
        assert_eq!(super::super::label(&transcript[0], &names), "Casey");
        assert_eq!(super::super::label(&transcript[1], &names), "Jordan");
    }

    #[test]
    fn native_frames_never_accept_an_overlap_or_missing_participant_identity() {
        let origin = Instant::now();
        let mut window = Window::default();
        let mut input = frame(origin, "Casey", 1.0, 16000, 8192);
        input
            .attribution
            .speakers
            .push(input.attribution.speakers[0].clone());
        assert!(window.ingest(input, origin).is_err());
        let mut input = frame(origin, "Casey", 1.0, 16000, 8192);
        input.attribution.speakers.clear();
        assert!(window.ingest(input, origin).is_err());
        assert!(window.voices.is_empty());
    }
    #[test]
    fn packet_boundary_preserves_every_sample_without_duplication() {
        let origin = Instant::now();
        let mut window = Window::default();
        let mut packet = frame(origin, "Casey", 1.0, 16000, 0);
        packet.samples = (0..16000).map(|x| x as i16).collect();
        let mut queue = VecDeque::from([packet]);
        consume(
            &mut queue,
            origin + Duration::from_millis(500),
            origin,
            &mut window,
        )
        .unwrap();
        assert_eq!(window.voices[0].pcm.len(), 8000);
        assert_eq!(queue[0].samples.len(), 8000);
        consume(
            &mut queue,
            origin + Duration::from_secs(1),
            origin,
            &mut window,
        )
        .unwrap();
        assert_eq!(window.voices[0].pcm.len(), 16000);
        assert!(queue.is_empty());
        for (i, sample) in window.voices[0].pcm.iter().enumerate() {
            assert_eq!(*sample, i as f32 / 32768.0);
        }
    }
    #[test]
    fn failed_revision_retains_previous_words_and_commit_clears_once() {
        let origin = Instant::now();
        let mut window = Window::default();
        window
            .ingest(frame(origin, "Casey", 1.0, 16000, 8192), origin)
            .unwrap();
        window.rows = window
            .decode(1.0, |pcm| {
                Ok(if pcm.is_empty() { "" } else { "Keep this." }.into())
            })
            .unwrap();
        assert!(window.decode(2.0, |_| anyhow::bail!("cancelled")).is_err());
        assert_eq!(window.rows[0].text, "Keep this.");
        let mut committed = Vec::new();
        window.commit(1.0, &mut |update| {
            if let Update::Rows(rows) = update {
                committed.extend(rows)
            }
        });
        assert_eq!(committed.len(), 1);
        assert!(window.rows.is_empty());
        assert!(window.voices.is_empty());
    }
    #[test]
    fn silence_keeps_context_and_late_audio_or_excess_participants_fail() {
        let origin = Instant::now();
        let mut window = Window::default();
        window
            .ingest(frame(origin, "Casey", 1.0, 16000, 8192), origin)
            .unwrap();
        window
            .ingest(frame(origin, "Casey", 2.0, 8000, 8192), origin)
            .unwrap();
        assert_eq!(window.voices[0].pcm.len(), 32000);
        assert!(window.voices[0].pcm[16000..24000].iter().all(|&x| x == 0.0));
        for i in 1..16 {
            window
                .ingest(
                    frame(origin, &format!("person-{i}"), 1.0, 16000, 8192),
                    origin,
                )
                .unwrap();
        }
        assert!(
            window
                .ingest(frame(origin, "overflow", 1.0, 16000, 8192), origin)
                .is_err()
        );
        window.start = 3.0;
        assert!(
            window
                .ingest(frame(origin, "late", 2.0, 16000, 8192), origin)
                .is_err()
        );
    }

    #[test]
    fn small_packets_resample_as_one_draft_and_duration_is_bounded() {
        let origin = Instant::now();
        let mut window = Window::default();
        for packet in 1..=50 {
            let mut input = frame(origin, "Casey", packet as f64 * 0.02, 960, 8192);
            input.rate = 48000;
            window.ingest(input, origin).unwrap();
        }
        assert_eq!(window.voices[0].pcm.len(), 48000);
        let mut lengths = Vec::new();
        window
            .decode(1.0, |pcm| {
                lengths.push(pcm.len());
                Ok(String::new())
            })
            .unwrap();
        assert_eq!(lengths, [0, 16000]);
        assert!(window.endpoint(24.0));
        let mut huge = frame(origin, "Casey", 27.0, 26 * 48000, 8192);
        huge.rate = 48000;
        assert!(window.ingest(huge, origin).is_err());
    }
}
