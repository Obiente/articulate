//! Separate decoded participant PCM. The mixed output device is never opened here.
use super::{REFRESH_SECONDS, Request, Row, Update, quiet_tail};
use crate::{
    call_capture::Track, discord::pcm::Frame, discord_attribution::Attribution, engine::Engine,
};
use anyhow::{Result, ensure};
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

const MAX_PARTICIPANTS: usize = 16;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_SECONDS: f64 = 24.0;
const CLOCK_JITTER: f64 = 0.030;

struct Voice {
    native_generation: u64,
    loss_epoch: u64,
    attribution: Attribution,
    rate: u32,
    start: f64,
    end: f64,
    pcm: Vec<f32>,
}

struct Boundary {
    attribution: Attribution,
    native_generation: u64,
    end: f64,
}

fn same_person(a: &Attribution, b: &Attribution) -> bool {
    a.generation == b.generation
        && a.channel_id == b.channel_id
        && a.speakers[0].id == b.speakers[0].id
}

#[derive(Default)]
struct Window {
    start: f64,
    mic: Vec<f32>,
    voices: Vec<Voice>,
    previous: Vec<Boundary>,
    rows: Vec<Row>,
    // Only completed speech spans are reusable as new audio extends this window.
    turn_cache: HashMap<(usize, usize, usize), String>,
    // Audio delivered after its section was finalized cannot be transcribed
    // retroactively. Retain a session-wide count for the incomplete-audio notice.
    late_packets: u64,
    context_submitted: BTreeSet<(usize, usize, usize)>,
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
        let mut skip = if raw_start < origin {
            (origin.duration_since(raw_start).as_secs_f64() * f64::from(frame.rate)).ceil() as usize
        } else {
            0
        };
        let frames = frame.samples.len() / usize::from(frame.channels);
        if skip >= frames {
            return Ok(());
        }
        let raw_seconds = if raw_start >= origin {
            raw_start.duration_since(origin).as_secs_f64()
        } else {
            -origin.duration_since(raw_start).as_secs_f64()
        };
        let mut start = raw_seconds + skip as f64 / f64::from(frame.rate);
        let end = frame.at.saturating_duration_since(origin).as_secs_f64();
        let existing = self
            .voices
            .iter()
            .rposition(|voice| same_person(&voice.attribution, &frame.attribution));
        let previous = existing
            .map(|slot| {
                let voice = &self.voices[slot];
                (
                    voice.native_generation,
                    voice
                        .end
                        .max(voice.start + voice.pcm.len() as f64 / f64::from(voice.rate)),
                )
            })
            .or_else(|| {
                self.previous
                    .iter()
                    .find(|voice| same_person(&voice.attribution, &frame.attribution))
                    .map(|voice| (voice.native_generation, voice.end))
            });
        if previous.is_some_and(|(generation, _)| frame.native_generation < generation) {
            return Ok(());
        }
        let replacement =
            previous.is_some_and(|(generation, _)| frame.native_generation > generation);
        let original_skip = skip;
        // A callback can be delayed past commit without belonging to a replaced
        // connection. Drop only its finalized prefix, retaining any new tail.
        let cutoff = previous
            .and_then(|(_, end)| replacement.then_some(end.max(self.start)))
            .or_else(|| (start < self.start).then_some(self.start));
        if let Some(cutoff) = cutoff {
            skip = skip.max(
                (((cutoff - raw_seconds) * f64::from(frame.rate) - 1e-6)
                    .ceil()
                    .max(0.0)) as usize,
            );
            start = raw_seconds + skip as f64 / f64::from(frame.rate);
        }
        // A finalized boundary can trim ordinary clock jitter or digital silence.
        // Neither is evidence of missing conversation. Inspect only the discarded
        // prefix, never the retained tail, and keep replacement overlap excluded.
        let discarded = skip.min(frames).saturating_sub(original_skip);
        if !replacement
            && discarded > 1
            && (end <= self.start || discarded as f64 / f64::from(frame.rate) > CLOCK_JITTER)
            && frame.samples[original_skip * usize::from(frame.channels)
                ..skip.min(frames) * usize::from(frame.channels)]
                .iter()
                .any(|&sample| sample != 0)
        {
            self.late_packets = self.late_packets.saturating_add(1);
        }
        if skip >= frames {
            // Keep the old generation until replacement audio reaches its end.
            // The transport already discards late packets from that old stream.
            return Ok(());
        }
        let slot = existing.filter(|&slot| !replacement || self.voices[slot].rate == frame.rate);
        let slot = match slot {
            Some(slot) => slot,
            None => {
                ensure!(
                    self.voices.len() < MAX_PARTICIPANTS * 2
                        && (existing.is_some()
                            || self
                                .voices
                                .iter()
                                .map(|voice| (
                                    voice.attribution.generation,
                                    &voice.attribution.channel_id,
                                    &voice.attribution.speakers[0].id
                                ))
                                .collect::<BTreeSet<_>>()
                                .len()
                                < MAX_PARTICIPANTS),
                    "This capture reached its bounded participant stream limit"
                );
                self.voices.push(Voice {
                    native_generation: frame.native_generation,
                    loss_epoch: frame.loss_epoch,
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
        let timeline_end = if cutoff.is_some() {
            voice.start + voice.pcm.len() as f64 / f64::from(voice.rate)
        } else {
            voice.end
        };
        let gap = if start - timeline_end > CLOCK_JITTER
            || frame.loss_epoch != voice.loss_epoch
            || cutoff.is_some()
        {
            ((start - timeline_end).max(0.0) * f64::from(voice.rate)).round() as usize
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
        voice.loss_epoch = frame.loss_epoch;
        voice.native_generation = frame.native_generation;
        Ok(())
    }

    #[cfg(test)]
    fn decode(
        &mut self,
        end: f64,
        transcribe: impl FnMut(&[f32]) -> Result<String>,
    ) -> Result<Vec<Row>> {
        self.decode_context(end, transcribe, &mut None, false)
    }

    fn decode_context(
        &mut self,
        end: f64,
        mut transcribe: impl FnMut(&[f32]) -> Result<String>,
        context: &mut Option<crate::sensevoice::Worker>,
        finalizing: bool,
    ) -> Result<Vec<Row>> {
        // Locate utterances on the aligned separate tracks before recognition.
        // Whole-track ASR loses the timing of A/B/A exchanges because this model
        // returns text without word timestamps.
        let remote = self
            .voices
            .iter()
            .map(|voice| {
                let mut pcm = crate::audio::resample(&voice.pcm, voice.rate)?;
                // consume() has already admitted all audio through this boundary.
                // A callback that stopped sending is silence through that time,
                // not an indefinitely unfinished turn.
                let through = ((end - voice.start).max(0.0) * 16000.0).round() as usize;
                pcm.resize(pcm.len().max(through), 0.0);
                Ok(pcm)
            })
            .collect::<Result<Vec<_>>>()?;
        let mut tracks = vec![super::turns::Track {
            pcm: &self.mic,
            start_ms: (self.start * 1000.0) as u64,
        }];
        tracks.extend(
            remote
                .iter()
                .zip(&self.voices)
                .map(|(pcm, voice)| super::turns::Track {
                    pcm,
                    start_ms: (voice.start * 1000.0) as u64,
                }),
        );
        let mut rows = Vec::new();
        let mut next_cache = HashMap::new();
        for turn in super::turns::plan(&tracks) {
            let key = (turn.track, turn.audio_start, turn.audio_end);
            let text = if let Some(text) = self.turn_cache.get(&key) {
                text.clone()
            } else {
                transcribe(&tracks[turn.track].pcm[turn.audio_start..turn.audio_end])?
            };
            if turn.complete && next_cache.len() < 256 {
                next_cache.insert(key, text.clone());
            }
            let row = Row {
                cues: Vec::new(),
                start_ms: tracks[turn.track].start_ms + turn.start as u64 / 16,
                end_ms: (tracks[turn.track].start_ms + turn.end as u64 / 16)
                    .min((end * 1000.0) as u64),
                microphone: turn.track == 0,
                speakers: Vec::new(),
                discord: turn
                    .track
                    .checked_sub(1)
                    .map(|index| self.voices[index].attribution.clone()),
                text,
            };
            if (turn.complete || finalizing || end - self.start >= MAX_SECONDS - 0.001)
                && self.context_submitted.len() < 256
                && !self.context_submitted.contains(&key)
                && let Some(context) = context
            {
                // Cue times cover the analyzed clip, not invented event/word timestamps.
                let mut target = row.clone();
                target.start_ms = tracks[turn.track].start_ms + turn.audio_start as u64 / 16;
                target.end_ms = tracks[turn.track].start_ms + turn.audio_end as u64 / 16;
                context.submit(
                    &target,
                    &tracks[turn.track].pcm[turn.audio_start..turn.audio_end],
                );
                self.context_submitted.insert(key);
            }
            if !row.text.is_empty() {
                rows.push(row);
            }
        }
        rows.sort_by_key(|row| row.start_ms);
        self.turn_cache = next_cache;
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
        self.turn_cache.clear();
        self.context_submitted.clear();
        self.previous.clear();
        for voice in self.voices.iter().rev() {
            if !self
                .previous
                .iter()
                .any(|previous| same_person(&previous.attribution, &voice.attribution))
            {
                self.previous.push(Boundary {
                    attribution: voice.attribution.clone(),
                    native_generation: voice.native_generation,
                    end: end
                        .max(voice.end)
                        .max(voice.start + voice.pcm.len() as f64 / f64::from(voice.rate)),
                });
            }
        }
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
            native_generation: frame.native_generation,
            loss_epoch: frame.loss_epoch,
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
    let mut context = crate::sensevoice::Worker::start(request.audio_context);
    let mut received = false;
    let mut lost_packets = 0;
    let mut pending = VecDeque::new();
    let mut window = Window::default();
    let mut cursor = 0.0;
    let result = (|| -> Result<()> {
        loop {
            if let Some(context) = &mut context {
                context.poll(&mut update);
            }
            if request.control.abort.load(Ordering::Relaxed) {
                break;
            }
            let frames = capture.drain();
            let lost = capture.lost_packets().saturating_add(window.late_packets);
            if lost != lost_packets {
                lost_packets = lost;
                update(Update::AudioGap(lost));
            }
            let frames = frames?;
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
                let lost = capture.lost_packets().saturating_add(window.late_packets);
                if lost != lost_packets {
                    lost_packets = lost;
                    update(Update::AudioGap(lost));
                }
                window.mic.extend(mic.take_until(end)?);
                let rows = window.decode_context(
                    end,
                    |pcm| engine.transcribe(pcm),
                    &mut context,
                    stopping,
                )?;
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
    drop(capture);
    drop(mic);
    if let Some(context) = &mut context {
        context.finish(&mut update, &request.control.abort);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(origin: Instant, id: &str, end: f64, samples: usize, value: i16) -> Frame {
        Frame {
            native_generation: 1,
            loss_epoch: 0,
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
    fn a_loss_disables_jitter_compensation_for_each_affected_voice() {
        let origin = Instant::now();
        let mut window = Window::default();
        for id in ["Casey", "Jordan"] {
            window
                .ingest(frame(origin, id, 0.020, 320, 8192), origin)
                .unwrap();
        }
        // One lost 20ms callback belongs to an unknown participant. Preserve
        // the next real timestamp gap on both tracks, never guess its owner.
        for id in ["Casey", "Jordan"] {
            let mut next = frame(origin, id, 0.060, 320, 8192);
            next.loss_epoch = 1;
            window.ingest(next, origin).unwrap();
        }
        for voice in &window.voices {
            assert_eq!(voice.pcm.len(), 960);
            assert!(voice.pcm[320..640].iter().all(|sample| *sample == 0.0));
            assert!(voice.pcm[640..].iter().all(|sample| *sample == 0.25));
        }
    }
    #[test]
    fn replacement_trims_already_accumulated_samples_including_clock_jitter() {
        let origin = Instant::now();
        let mut window = Window::default();
        window
            .ingest(frame(origin, "Casey", 0.1, 1600, 8192), origin)
            .unwrap();
        window
            .ingest(frame(origin, "Casey", 0.18, 1600, 8192), origin)
            .unwrap();
        let mut replacement = frame(origin, "Casey", 0.25, 1600, -8192);
        replacement.native_generation = 2;
        window.ingest(replacement, origin).unwrap();
        assert_eq!(window.voices.len(), 1);
        assert_eq!(window.voices[0].pcm.len(), 4000);
        assert!(
            window.voices[0].pcm[..3200]
                .iter()
                .all(|sample| *sample == 0.25)
        );
        assert!(
            window.voices[0].pcm[3200..]
                .iter()
                .all(|sample| *sample == -0.25)
        );
    }

    #[test]
    fn replacement_preserves_short_wallclock_gaps_and_other_people_generations() {
        let origin = Instant::now();
        let mut window = Window::default();
        let mut first = frame(origin, "Casey", 0.1, 1600, 8192);
        first.native_generation = 40;
        window.ingest(first, origin).unwrap();
        let mut other = frame(origin, "Jordan", 0.1, 1600, -8192);
        other.native_generation = 2;
        window.ingest(other, origin).unwrap();
        let mut next = frame(origin, "Casey", 0.14, 320, 8192);
        next.native_generation = 41;
        window.ingest(next, origin).unwrap();
        assert_eq!(window.voices[0].pcm.len(), 2240);
        assert!(
            window.voices[0].pcm[1600..1920]
                .iter()
                .all(|sample| *sample == 0.0)
        );
        assert_eq!(window.voices[1].pcm.len(), 1600);
        assert_eq!(window.voices[1].native_generation, 2);
    }

    #[test]
    fn replacement_straddling_a_committed_section_keeps_only_new_audio() {
        let origin = Instant::now();
        let mut window = Window::default();
        window
            .ingest(frame(origin, "Casey", 0.1, 1600, 8192), origin)
            .unwrap();
        window.commit(0.1, &mut |_| {});
        let mut next = frame(origin, "Casey", 0.15, 1600, -8192);
        next.native_generation = 2;
        window.ingest(next, origin).unwrap();
        assert_eq!(window.voices[0].pcm.len(), 800);
        assert!((window.voices[0].start - 0.1).abs() < 1e-8);
    }

    #[test]
    fn replacement_format_change_splits_segments_but_same_stream_remains_strict() {
        let origin = Instant::now();
        let mut window = Window::default();
        window
            .ingest(frame(origin, "Casey", 0.1, 1600, 8192), origin)
            .unwrap();
        assert!(
            window
                .ingest(frame(origin, "Casey", 0.15, 1600, 8192), origin)
                .is_err()
        );
        let mut changed = frame(origin, "Casey", 0.2, 4800, -8192);
        changed.rate = 48000;
        assert!(window.ingest(changed, origin).is_err());
        let mut changed = frame(origin, "Casey", 0.2, 4800, -8192);
        changed.rate = 48000;
        changed.native_generation = 2;
        window.ingest(changed, origin).unwrap();
        assert_eq!(window.voices.len(), 2);
        assert_eq!(window.voices[0].pcm.len(), 1600);
        assert_eq!(window.voices[1].pcm.len(), 4800);
        assert_eq!(window.voices[1].attribution.speakers[0].id, "Casey");
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
    fn silence_keeps_context_participants_stay_bounded_and_late_audio_is_counted() {
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
        window
            .ingest(frame(origin, "late", 2.0, 16000, 8192), origin)
            .unwrap();
        assert_eq!(window.late_packets, 1);
        assert_eq!(window.voices.len(), 16);
    }

    #[test]
    fn silent_late_audio_and_small_boundary_overlap_do_not_report_loss() {
        let origin = Instant::now();
        let mut window = Window {
            start: 1.0,
            ..Window::default()
        };
        window
            .ingest(frame(origin, "Casey", 0.95, 1600, 0), origin)
            .unwrap();
        assert_eq!(window.late_packets, 0);
        // A silent discarded prefix must not inherit sound from the retained tail.
        let mut crossing = frame(origin, "Casey", 1.05, 1600, 0);
        crossing.samples[800..].fill(8192);
        window.ingest(crossing, origin).unwrap();
        assert_eq!(window.late_packets, 0);
        assert_eq!(window.voices[0].pcm.len(), 800);
        let mut jitter = Window {
            start: 1.0,
            ..Window::default()
        };
        jitter
            .ingest(frame(origin, "Casey", 1.01, 320, 8192), origin)
            .unwrap();
        assert_eq!(jitter.late_packets, 0);
        // A fully discarded audible packet remains a genuine loss signal.
        jitter
            .ingest(frame(origin, "Casey", 0.95, 320, 8192), origin)
            .unwrap();
        assert_eq!(jitter.late_packets, 1);
    }

    #[test]
    fn delayed_packets_skip_finalized_audio_and_keep_a_straddling_tail() {
        let origin = Instant::now();
        let mut window = Window::default();
        window
            .ingest(frame(origin, "Casey", 1.0, 16000, 8192), origin)
            .unwrap();
        window.rows = window
            .decode(1.0, |_| Ok("Finalized words.".into()))
            .unwrap();
        let mut saved = Vec::new();
        window.commit(1.0, &mut |update| {
            if let Update::Rows(rows) = update {
                saved.extend(rows)
            }
        });
        window
            .ingest(frame(origin, "Casey", 0.95, 320, 16384), origin)
            .unwrap();
        assert!(window.voices.is_empty());
        window
            .ingest(frame(origin, "Casey", 1.05, 1600, -8192), origin)
            .unwrap();
        assert_eq!(window.voices.len(), 1);
        assert_eq!(window.voices[0].pcm.len(), 800);
        assert!((window.voices[0].start - 1.0).abs() < 1e-8);
        assert!(window.voices[0].pcm.iter().all(|sample| *sample == -0.25));
        assert_eq!(window.late_packets, 2);
        window
            .ingest(frame(origin, "Casey", 1.07, 320, -8192), origin)
            .unwrap();
        assert_eq!(window.voices[0].pcm.len(), 1120);
        window.commit(1.07, &mut |_| {});
        assert_eq!(window.late_packets, 2, "The warning survives commit");
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].text, "Finalized words.");
    }

    #[test]
    fn alternating_tracks_emit_a_b_a_and_cache_completed_turns() {
        let origin = Instant::now();
        let mut window = Window::default();
        let mut a = frame(origin, "Casey", 4.0, 64000, 0);
        a.samples[..9600].fill(8192);
        a.samples[32000..41600].fill(16384);
        window.ingest(a, origin).unwrap();
        let mut b = frame(origin, "Jordan", 4.0, 64000, 0);
        b.samples[12800..22400].fill(-8192);
        window.ingest(b, origin).unwrap();
        let mut decoded = 0;
        let rows = window
            .decode(4.0, |pcm| {
                decoded += 1;
                let sample = pcm
                    .iter()
                    .copied()
                    .find(|sample| sample.abs() > 0.01)
                    .unwrap();
                Ok(if sample < 0.0 {
                    "Reply."
                } else if sample > 0.3 {
                    "Follow-up."
                } else {
                    "Question."
                }
                .into())
            })
            .unwrap();
        assert_eq!(
            rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>(),
            ["Question.", "Reply.", "Follow-up."]
        );
        assert_eq!(
            rows.iter()
                .map(|row| row.discord.as_ref().unwrap().speakers[0].id.as_str())
                .collect::<Vec<_>>(),
            ["Casey", "Jordan", "Casey"]
        );
        assert_eq!(decoded, 3);
        assert!(rows[0].end_ms <= rows[1].start_ms && rows[1].end_ms <= rows[2].start_ms);
        window
            .decode(4.2, |_| anyhow::bail!("Completed turns must be reused"))
            .unwrap();
        let mut committed = Vec::new();
        super::super::append_rows(&mut committed, rows);
        assert_eq!(committed.len(), 3);
        window.commit(4.2, &mut |_| {});
        assert!(window.turn_cache.is_empty());
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
        assert_eq!(lengths, [16000]);
        assert!(window.endpoint(24.0));
        let mut huge = frame(origin, "Casey", 27.0, 26 * 48000, 8192);
        huge.rate = 48000;
        assert!(window.ingest(huge, origin).is_err());
    }
}
