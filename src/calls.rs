use crate::{
    call_capture::{Control, Track},
    engine::Engine,
    speakers::Tracker,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

mod activity;
mod native;
mod turns;

/// A requested companion capture never silently becomes mixed output capture.
pub fn select_native_source(companion_selected: bool, native_ready: bool) -> Result<bool> {
    anyhow::ensure!(
        !companion_selected || native_ready,
        "Separate Discord audio is not connected yet. Connect the companion audio adapter before capturing this call."
    );
    Ok(companion_selected)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Row {
    pub start_ms: u64,
    pub end_ms: u64,
    pub microphone: bool,
    pub speakers: Vec<i32>,
    /// A session-local match to observed Discord activity, independent of acoustic IDs.
    pub discord: Option<crate::discord_attribution::Attribution>,
    pub text: String,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::sensevoice::deserialize_cues"
    )]
    pub cues: Vec<crate::sensevoice::Cue>,
}

// Bundle natural continuations without hiding a later turn's own timestamp.
const ROW_CONTINUATION_GAP_MS: u64 = 1200;

pub fn append_rows(transcript: &mut Vec<Row>, mut incoming: Vec<Row>) {
    incoming.sort_by_key(|row| row.start_ms);
    for row in incoming {
        if transcript
            .last()
            .is_some_and(|last| row.start_ms < last.start_ms)
        {
            // A late result retains its timestamp and text. We cannot split an
            // already committed paragraph without word-level ASR timestamps.
            let position = transcript.partition_point(|existing| existing.start_ms <= row.start_ms);
            transcript.insert(position, row);
            continue;
        }
        let known_single_speaker = row.microphone
            || row
                .discord
                .as_ref()
                .is_some_and(|named| named.speakers.len() == 1)
            || (row.speakers.len() == 1 && (1..=8).contains(&row.speakers[0]));
        if let Some(last) = transcript.last_mut()
            && known_single_speaker
            && row.start_ms.saturating_sub(last.end_ms) <= ROW_CONTINUATION_GAP_MS
            && last.microphone == row.microphone
            && match (&last.discord, &row.discord) {
                (Some(previous), Some(next)) => previous == next && next.speakers.len() == 1,
                (None, None) => last.speakers == row.speakers,
                _ => false,
            }
        {
            if !last.text.is_empty() && !row.text.is_empty() {
                last.text.push(' ');
            }
            last.text.push_str(&row.text);
            for cue in row.cues {
                if !last.cues.contains(&cue) {
                    last.cues.push(cue);
                }
            }
            last.end_ms = last.end_ms.max(row.end_ms);
            for id in row.speakers {
                if !last.speakers.contains(&id) {
                    last.speakers.push(id);
                }
            }
            last.speakers.sort_unstable();
        } else {
            transcript.push(row);
        }
    }
}

pub enum Update {
    Started,
    /// Mixed output is still captured, but individual remote speakers cannot be distinguished.
    SpeakerFallback,
    /// Native capture is armed; true means an authenticated PCM frame arrived.
    NativeAudio(bool),
    /// Cumulative missing native packets, retained with this recording.
    AudioGap(u64),
    AudioContext(Row),
    AudioContextStatus(String),
    Levels(f32, f32),
    /// Replace the current uncommitted text as more audio context arrives.
    Preview(Vec<Row>),
    /// Append committed text and clear the current preview.
    Rows(Vec<Row>),
}
pub struct Request {
    pub audio_context: bool,
    pub language_preference: Option<String>,
    pub microphone: Option<String>,
    pub output: Option<String>,
    pub cpu: bool,
    pub native_audio: bool,
    /// Spoken notes capture only the microphone, even if Discord is connected.
    pub microphone_only: bool,
    pub control: Arc<Control>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Microphone,
    Native,
    Mixed,
}

impl Request {
    fn source(&self) -> Source {
        if self.microphone_only {
            Source::Microphone
        } else if self.native_audio {
            Source::Native
        } else {
            Source::Mixed
        }
    }
}

fn microphone_rows(text: String, samples: usize, offset_ms: u64) -> Vec<Row> {
    if text.is_empty() {
        return Vec::new();
    }
    vec![Row {
        cues: Vec::new(),
        start_ms: offset_ms,
        end_ms: offset_ms + samples as u64 / 16,
        microphone: true,
        speakers: Vec::new(),
        discord: None,
        text,
    }]
}

pub fn process_window(
    engine: &mut Engine,
    tracker: &mut Tracker,
    mic: &[f32],
    remote: &[f32],
    offset_ms: u64,
    final_pass: bool,
) -> Result<Vec<Row>> {
    let mut rows = microphone_rows(
        call_transcribe(engine, mic, final_pass)?,
        mic.len(),
        offset_ms,
    );
    let turns = tracker.identify(remote)?;
    for turn in crate::call_segments::plan(&turns, remote.len()) {
        let text = call_transcribe(
            engine,
            &remote[turn.audio_start..turn.audio_end],
            final_pass,
        )?;
        if text.is_empty() {
            continue;
        }
        rows.push(Row {
            cues: Vec::new(),
            start_ms: offset_ms + turn.start as u64 / 16,
            end_ms: offset_ms + turn.end as u64 / 16,
            microphone: false,
            speakers: turn.speakers,
            discord: None,
            text,
        });
    }
    rows.sort_by_key(|r| r.start_ms);
    Ok(rows)
}

fn process_unattributed_window(
    engine: &mut Engine,
    mic: &[f32],
    remote: &[f32],
    offset_ms: u64,
    final_pass: bool,
) -> Result<Vec<Row>> {
    let mut rows = microphone_rows(
        call_transcribe(engine, mic, final_pass)?,
        mic.len(),
        offset_ms,
    );
    let text = call_transcribe(engine, remote, final_pass)?;
    if !text.is_empty() {
        rows.push(Row {
            cues: Vec::new(),
            start_ms: offset_ms,
            end_ms: offset_ms + remote.len() as u64 / 16,
            microphone: false,
            speakers: vec![0],
            discord: None,
            text,
        });
    }
    Ok(rows)
}

fn process_activity_window(
    engine: &mut Engine,
    mic: &[f32],
    remote: &[f32],
    offset_ms: u64,
    segments: &[crate::discord_attribution::ActivitySegment],
    final_pass: bool,
) -> Result<Vec<Row>> {
    let mut rows = microphone_rows(
        call_transcribe(engine, mic, final_pass)?,
        mic.len(),
        offset_ms,
    );
    for segment in activity::plan(segments, remote.len()) {
        let text = call_transcribe(
            engine,
            &remote[segment.audio_start..segment.audio_end],
            final_pass,
        )?;
        if !text.is_empty() {
            rows.push(Row {
                cues: Vec::new(),
                start_ms: offset_ms + segment.start as u64 / 16,
                end_ms: offset_ms + segment.end as u64 / 16,
                microphone: false,
                speakers: if segment.attribution.is_some() {
                    Vec::new()
                } else {
                    vec![0]
                },
                discord: segment.attribution,
                text,
            });
        }
    }
    rows.sort_by_key(|row| row.start_ms);
    Ok(rows)
}

fn call_transcribe(engine: &mut Engine, pcm: &[f32], final_pass: bool) -> Result<String> {
    if final_pass {
        engine.transcribe_final(pcm)
    } else {
        engine.transcribe_preview(pcm)
    }
}

// Native ASR has no word timestamps. Revisions replace an entire bounded draft
// rather than guessing word boundaries or deduplicating recognized text.
const REFRESH_SECONDS: f64 = 2.0;
const MAX_DRAFT_SAMPLES: usize = 24 * 16_000;
const ENDPOINT_SAMPLES: usize = 600 * 16;

#[derive(Default)]
struct Draft {
    mic: Vec<f32>,
    remote: Vec<f32>,
    rows: Vec<Row>,
}

impl Draft {
    fn remaining_seconds(&self) -> f64 {
        MAX_DRAFT_SAMPLES.saturating_sub(self.mic.len().max(self.remote.len())) as f64 / 16_000.0
    }

    fn endpoint(&self, microphone_only: bool) -> bool {
        self.mic.len().max(self.remote.len()) >= MAX_DRAFT_SAMPLES
            || (quiet_tail(&self.mic) && (microphone_only || quiet_tail(&self.remote)))
    }

    fn commit(&mut self, update: &mut impl FnMut(Update)) {
        // Error/cancel publishes the last successful preview exactly once.
        if !self.rows.is_empty() {
            update(Update::Rows(std::mem::take(&mut self.rows)));
        }
        self.mic.clear();
        self.remote.clear();
    }
}

fn quiet_tail(pcm: &[f32]) -> bool {
    pcm.len() >= ENDPOINT_SAMPLES
        && pcm[pcm.len() - ENDPOINT_SAMPLES..]
            .iter()
            .all(|sample| sample.is_finite() && sample.abs() < 0.003)
}

pub fn run(engine: &mut Engine, request: Request, mut update: impl FnMut(Update)) -> Result<()> {
    let source = request.source();
    if source == Source::Native {
        return native::run(engine, request, update);
    }
    let microphone_only = source == Source::Microphone;
    let mut tracker: Option<Tracker> = None;
    let mut tracker_unavailable = false;
    if request.control.abort.load(Ordering::Relaxed)
        || request.control.stop_ns.load(Ordering::SeqCst) != 0
    {
        return Ok(());
    }
    let mut microphone = Track::open(
        request.microphone.as_deref(),
        false,
        request.control.clone(),
    )?;
    let mut output = if microphone_only {
        None
    } else {
        Some(Track::open(
            request.output.as_deref(),
            true,
            request.control.clone(),
        )?)
    };
    update(Update::Started);
    let mut context = crate::sensevoice::Worker::start(request.audio_context);
    let mut cursor = 0.0;
    let mut draft_start = 0.0;
    let mut draft = Draft::default();
    let mut checkpoint = None;
    let result = (|| -> Result<()> {
        loop {
            if let Some(context) = &mut context {
                context.poll(&mut update);
            }
            if request.control.abort.load(Ordering::Relaxed) {
                return Ok(());
            }
            let stopping = request.control.stop_ns.load(Ordering::SeqCst) != 0;
            let now = request.control.end_seconds();
            anyhow::ensure!(
                now - cursor < 40.0,
                "Call transcription fell more than 40 seconds behind. Capture stopped; the latest transcript is retained."
            );
            let ready = if stopping { now } else { (now - 0.25).max(0.0) };
            let remaining = draft.remaining_seconds();
            if stopping || ready >= cursor + REFRESH_SECONDS.min(remaining) {
                // Slow CPUs consume available audio in one revision instead of
                // queueing every missed refresh, with a fixed PCM memory bound.
                let end = ready.min(cursor + remaining);
                if end <= cursor {
                    break;
                }
                draft.mic.extend(microphone.take_until(end)?);
                if let Some(output) = &mut output {
                    draft.remote.extend(output.take_until(end)?);
                }
                let final_pass = draft.endpoint(microphone_only) || (stopping && end >= now);
                let offset_ms = (draft_start * 1000.0) as u64;
                let discord = if microphone_only {
                    None
                } else {
                    request.control.discord()
                };
                let discord_context =
                    discord
                        .zip(request.control.origin())
                        .map(|(discord, origin)| {
                            let from = origin + Duration::from_secs_f64(draft_start);
                            let to = origin + Duration::from_secs_f64(end);
                            (origin, discord.history(from, to))
                        });
                let activity = discord_context.as_ref().and_then(|(origin, history)| {
                    crate::discord_attribution::plan_activity(
                        *origin,
                        offset_ms,
                        draft.remote.len(),
                        history,
                    )
                });
                let rows = if microphone_only {
                    microphone_rows(
                        call_transcribe(engine, &draft.mic, final_pass)?,
                        draft.mic.len(),
                        offset_ms,
                    )
                } else if let Some(segments) = activity {
                    process_activity_window(
                        engine,
                        &draft.mic,
                        &draft.remote,
                        offset_ms,
                        &segments,
                        final_pass,
                    )?
                } else {
                    if tracker.is_none() && !tracker_unavailable {
                        match Tracker::new(request.cpu) {
                            Ok(fallback) => {
                                checkpoint = Some(fallback.checkpoint());
                                tracker = Some(fallback);
                            }
                            Err(error) => {
                                eprintln!(
                                    "Speaker recognition unavailable; using mixed call audio: {error}"
                                );
                                tracker_unavailable = true;
                                update(Update::SpeakerFallback);
                            }
                        }
                    }
                    let mut rows = if let Some(fallback) = tracker.as_mut() {
                        fallback.restore(
                            checkpoint
                                .as_ref()
                                .expect("fallback checkpoint initialized"),
                        );
                        process_window(
                            engine,
                            fallback,
                            &draft.mic,
                            &draft.remote,
                            offset_ms,
                            final_pass,
                        )?
                    } else {
                        process_unattributed_window(
                            engine,
                            &draft.mic,
                            &draft.remote,
                            offset_ms,
                            final_pass,
                        )?
                    };
                    if let Some((origin, history)) = &discord_context {
                        for row in &mut rows {
                            row.discord =
                                crate::discord_attribution::identify(row, *origin, history);
                        }
                    }
                    rows
                };
                draft.rows = rows;
                cursor = end;
                if final_pass {
                    if let Some(context) = &mut context {
                        for row in &draft.rows {
                            if !row.microphone
                                && (row.speakers.len() != 1 || row.speakers[0] == 0)
                                && !row.discord.as_ref().is_some_and(|d| d.speakers.len() == 1)
                            {
                                continue;
                            }
                            let pcm = if row.microphone {
                                &draft.mic
                            } else {
                                &draft.remote
                            };
                            let from = (row.start_ms.saturating_sub(offset_ms) as usize * 16)
                                .min(pcm.len());
                            let to =
                                (row.end_ms.saturating_sub(offset_ms) as usize * 16).min(pcm.len());
                            if to > from {
                                context.submit(row, &pcm[from..to]);
                            }
                        }
                    }
                    // Empty successful revisions must clear previous previews.
                    update(Update::Rows(std::mem::take(&mut draft.rows)));
                    draft.mic.clear();
                    draft.remote.clear();
                    checkpoint = tracker.as_ref().map(Tracker::checkpoint);
                    draft_start = cursor;
                } else {
                    update(Update::Preview(draft.rows.clone()));
                }
                if stopping && cursor >= now {
                    break;
                }
            } else {
                update(Update::Levels(
                    f32::from_bits(microphone.level.load(Ordering::Relaxed)),
                    output.as_ref().map_or(0.0, |track| {
                        f32::from_bits(track.level.load(Ordering::Relaxed))
                    }),
                ));
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        Ok(())
    })();
    // An incomplete decode never replaces the last successful preview. Flushing
    // it also preserves text when capture fails or the user cancels mid-phrase.
    draft.commit(&mut update);
    drop(microphone);
    drop(output);
    if let Some(context) = &mut context {
        context.finish(&mut update, &request.control.abort);
    }
    result
}

pub fn label(row: &Row, names: &[String]) -> String {
    let labels = speaker_labels(row, names);
    if labels.len() > 1 {
        format!("Overlap: {}", labels.join(" + "))
    } else {
        labels.join("")
    }
}

pub fn empty_speaker_names() -> Vec<String> {
    vec![String::new(); 8]
}

pub fn speaker_labels(row: &Row, names: &[String]) -> Vec<String> {
    if row.microphone {
        return vec!["You".into()];
    }
    if let Some(attribution) = &row.discord {
        let names: Vec<_> = attribution
            .speakers
            .iter()
            .map(|speaker| speaker.name.clone())
            .collect();
        if !names.is_empty() {
            return names;
        }
    }
    let labels: Vec<_> = row
        .speakers
        .iter()
        .map(|&id| {
            if id == 0 {
                "Other audio".into()
            } else if !(1..=8).contains(&id) {
                "Uncertain speaker".into()
            } else {
                names
                    .get(id as usize - 1)
                    .filter(|name| !name.trim().is_empty())
                    .cloned()
                    .unwrap_or_else(|| format!("Speaker {id}"))
            }
        })
        .collect();
    labels
}

pub fn text(rows: &[Row], names: &[String]) -> String {
    rows.iter()
        .map(|r| {
            format!(
                "[{:02}:{:02}] {}: {}",
                r.start_ms / 60000,
                r.start_ms / 1000 % 60,
                label(r, names),
                display_text(r)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn display_text(row: &Row) -> String {
    let mut text = row.text.clone();
    for cue in &row.cues {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&format!(
            "[Audio cue, approximate {:02}:{:02}-{:02}:{:02}: {}]",
            cue.start_ms / 60000,
            cue.start_ms / 1000 % 60,
            cue.end_ms / 60000,
            cue.end_ms / 1000 % 60,
            cue.label
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spoken_note_source_excludes_native_and_mixed_audio() {
        let mut request = Request {
            audio_context: false,
            language_preference: None,
            microphone: None,
            output: Some("Unused output".into()),
            cpu: false,
            native_audio: true,
            microphone_only: true,
            control: Control::new(),
        };
        assert_eq!(request.source(), Source::Microphone);
        request.native_audio = false;
        assert_eq!(request.source(), Source::Microphone);
        request.microphone_only = false;
        assert_eq!(request.source(), Source::Mixed);
        request.native_audio = true;
        assert_eq!(request.source(), Source::Native);
    }

    #[test]
    fn spoken_note_commits_on_microphone_silence_without_a_remote_track() {
        let mut draft = Draft {
            mic: vec![0.0; ENDPOINT_SAMPLES],
            ..Default::default()
        };
        assert!(draft.endpoint(true));
        assert!(!draft.endpoint(false));
        draft.mic[ENDPOINT_SAMPLES - 1] = 0.1;
        assert!(!draft.endpoint(true));
        draft.mic.resize(MAX_DRAFT_SAMPLES, 0.1);
        assert!(draft.endpoint(true));
        draft.rows = microphone_rows("Keep this idea.".into(), 32_000, 4_000);
        let mut committed = Vec::new();
        draft.commit(&mut |update| {
            if let Update::Rows(rows) = update {
                committed.extend(rows);
            }
        });
        assert_eq!(committed.len(), 1);
        assert!(committed[0].microphone);
        assert!(committed[0].discord.is_none());
        assert!(committed[0].speakers.is_empty());
        assert_eq!((committed[0].start_ms, committed[0].end_ms), (4_000, 6_000));
        assert!(microphone_rows(String::new(), 32_000, 4_000).is_empty());
    }

    #[test]
    #[ignore = "Requires downloaded models and an ARTICULATE_CALL_REPLAY WAV fixture"]
    fn replay_call_context_with_real_models() -> Result<()> {
        transcribe_cpp::init_backends_default()?;
        let fixture = std::env::var("ARTICULATE_CALL_REPLAY")?;
        let pcm = crate::audio::read_wav(&fixture)?;
        anyhow::ensure!(pcm.len() <= MAX_DRAFT_SAMPLES, "Use at most 24 seconds");
        let token = transcribe_cpp::CancelToken::new();
        let mut engine = Engine::load(&crate::model::default_path(), false, &token)?;
        let mut tracker = Tracker::new(false)?;
        let checkpoint = tracker.checkpoint();
        for samples in (32_000..pcm.len())
            .step_by(32_000)
            .chain(std::iter::once(pcm.len()))
        {
            tracker.restore(&checkpoint);
            let rows = process_window(
                &mut engine,
                &mut tracker,
                &vec![0.0; samples],
                &pcm[..samples],
                0,
                samples == pcm.len(),
            )?;
            assert!(rows.iter().all(|row| row.end_ms <= samples as u64 / 16));
            println!(
                "{}",
                serde_json::json!({"audio_ms":samples / 16,"rows":rows})
            );
        }
        Ok(())
    }

    #[test]
    fn a_sentence_crossing_eight_seconds_stays_revisable() {
        let mut draft = Draft {
            mic: vec![0.0; 8 * 16_000],
            remote: vec![0.1; 8 * 16_000],
            rows: vec![row(0, &[1], false, "Do you want?")],
        };
        assert!(!draft.endpoint(false));
        draft.mic.extend(vec![0.0; 4 * 16_000]);
        draft.remote.extend(vec![0.1; 4 * 16_000]);
        assert!(!draft.endpoint(false));
        let segments = crate::call_segments::plan(
            &[
                crate::speakers::Turn {
                    start: 0,
                    end: 8 * 16_000,
                    speakers: vec![1],
                },
                crate::speakers::Turn {
                    start: 8 * 16_000,
                    end: 12 * 16_000,
                    speakers: vec![1],
                },
            ],
            draft.remote.len(),
        );
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].audio_end, 12 * 16_000);
        draft.rows = vec![row(0, &[1], false, "Do you want to play together?")];
        let mut committed = Vec::new();
        draft.commit(&mut |update| {
            if let Update::Rows(rows) = update {
                append_rows(&mut committed, rows);
            }
        });
        assert_eq!(committed.len(), 1);
        assert_eq!(committed[0].text, "Do you want to play together?");
    }

    #[test]
    fn stopping_after_a_failed_decode_retains_last_preview_once() {
        let mut draft = Draft {
            rows: vec![row(12000, &[2], false, "The latest successful words.")],
            ..Default::default()
        };
        let mut committed = Vec::new();
        let mut flush = |update| {
            if let Update::Rows(rows) = update {
                committed.extend(rows);
            }
        };
        draft.commit(&mut flush);
        draft.commit(&mut flush);
        assert_eq!(committed.len(), 1);
        assert_eq!(committed[0].start_ms, 12000);
        assert_eq!(committed[0].text, "The latest successful words.");
    }

    #[test]
    fn endpoint_requires_both_tracks_quiet_and_context_is_bounded() {
        let mut draft = Draft {
            mic: vec![0.0; ENDPOINT_SAMPLES],
            remote: vec![0.1; ENDPOINT_SAMPLES],
            ..Default::default()
        };
        assert!(!draft.endpoint(false));
        draft.remote.fill(0.0);
        assert!(draft.endpoint(false));
        draft.remote.fill(f32::NAN);
        assert!(!draft.endpoint(false));
        draft.remote.resize(MAX_DRAFT_SAMPLES, 0.1);
        assert!(draft.endpoint(false));
        assert_eq!(draft.remaining_seconds(), 0.0);
        draft.commit(&mut |_| {});
        assert_eq!(draft.remaining_seconds(), 24.0);
    }

    fn row(start: u64, speakers: &[i32], microphone: bool, text: &str) -> Row {
        Row {
            cues: Vec::new(),
            start_ms: start,
            end_ms: start + 8000,
            microphone,
            speakers: speakers.to_vec(),
            discord: None,
            text: text.into(),
        }
    }

    #[test]
    fn eight_speaker_labels_and_overlap_remain_distinct() {
        let mut names = empty_speaker_names();
        names[7] = "Morgan".into();
        let overlapping = row(0, &[2, 8], false, "Both spoke.");
        assert_eq!(
            speaker_labels(&overlapping, &names),
            ["Speaker 2", "Morgan"]
        );
        assert_eq!(label(&overlapping, &names), "Overlap: Speaker 2 + Morgan");
        assert_eq!(
            label(&row(0, &[9], false, "Unknown."), &names),
            "Uncertain speaker"
        );
    }

    #[test]
    fn consecutive_windows_share_one_label_and_keep_all_text() {
        let mut rows = vec![row(0, &[1], false, "First sentence.")];
        append_rows(&mut rows, vec![row(8000, &[1], false, "Second sentence.")]);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].start_ms, rows[0].end_ms), (0, 16000));
        assert_eq!(
            text(&rows, &[]),
            "[00:00] Speaker 1: First sentence. Second sentence."
        );
    }

    #[test]
    fn interleaved_turns_sort_by_time_without_grouping_by_person() {
        let mut rows = Vec::new();
        // Input grouped by the ASR producer must still display A / B / A.
        append_rows(
            &mut rows,
            vec![
                row(0, &[1], false, "First speaker starts."),
                row(9000, &[1], false, "First speaker replies."),
                row(4000, &[2], false, "Second speaker interrupts."),
            ],
        );
        assert_eq!(
            rows.iter().map(|row| row.start_ms).collect::<Vec<_>>(),
            [0, 4000, 9000]
        );
        assert_eq!(
            rows.iter().map(|row| row.speakers[0]).collect::<Vec<_>>(),
            [1, 2, 1]
        );
        assert_eq!(
            rows[0].end_ms, 8000,
            "Overlapping turns keep their true bounds"
        );
        assert_eq!(rows[1].end_ms, 12000);
        assert_eq!(rows[2].text, "First speaker replies.");
    }

    #[test]
    fn short_continuations_bundle_but_long_pauses_keep_their_timestamp() {
        let mut rows = vec![row(0, &[1], false, "First thought.")];
        append_rows(&mut rows, vec![row(8500, &[1], false, "Same thought.")]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text, "First thought. Same thought.");
        append_rows(
            &mut rows,
            vec![row(30_000, &[1], false, "A later thought.")],
        );
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].start_ms, rows[0].end_ms), (0, 16_500));
        assert_eq!((rows[1].start_ms, rows[1].end_ms), (30_000, 38_000));
    }

    #[test]
    fn late_rows_keep_chronological_order_and_do_not_join_backward() {
        let mut rows = vec![
            row(0, &[1], false, "First."),
            row(20_000, &[1], false, "Last."),
        ];
        append_rows(&mut rows, vec![row(10_000, &[2], false, "Middle.")]);
        assert_eq!(
            rows.iter().map(|row| row.start_ms).collect::<Vec<_>>(),
            [0, 10_000, 20_000]
        );
        assert_eq!(
            rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>(),
            ["First.", "Middle.", "Last."]
        );
    }

    #[test]
    fn speaker_changes_overlap_and_uncertainty_keep_separate_blocks() {
        let mut rows = Vec::new();
        for (i, speakers) in [
            vec![1],
            vec![2],
            vec![1],
            vec![1, 2],
            vec![1],
            vec![0],
            vec![0],
        ]
        .iter()
        .enumerate()
        {
            append_rows(
                &mut rows,
                vec![row(i as u64 * 8000, speakers, false, "Speech.")],
            );
        }
        assert_eq!(rows.len(), 7);
        append_rows(&mut rows, vec![row(56000, &[], true, "My first sentence.")]);
        append_rows(
            &mut rows,
            vec![row(64000, &[], true, "My second sentence.")],
        );
        assert_eq!(rows.len(), 8);
        assert_eq!(label(rows.last().unwrap(), &[]), "You");
        assert_eq!(
            rows.last().unwrap().text,
            "My first sentence. My second sentence."
        );
    }

    fn named_row(start: u64, id: &str, name: &str) -> Row {
        let mut row = row(start, &[1], false, "I will review the résumé.");
        row.discord = Some(crate::discord_attribution::Attribution {
            generation: 1,
            channel_id: "synthetic-channel".into(),
            speakers: vec![crate::discord_attribution::NamedSpeaker {
                avatar: None,
                id: id.into(),
                name: name.into(),
            }],
        });
        row
    }

    #[test]
    fn named_rows_merge_only_with_the_same_identity_and_snapshot() {
        let first = named_row(0, "synthetic-user-one", "Zoë 李");
        let mut rows = vec![first];
        append_rows(
            &mut rows,
            vec![named_row(8000, "synthetic-user-one", "Zoë 李")],
        );
        assert_eq!(rows.len(), 1);
        let mut remapped = named_row(12000, "synthetic-user-one", "Zoë 李");
        remapped.speakers = vec![2];
        append_rows(&mut rows, vec![remapped]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].speakers, [1, 2]);
        assert_eq!(label(&rows[0], &[]), "Zoë 李");
        // Identical display names and acoustic clusters cannot merge people.
        append_rows(
            &mut rows,
            vec![named_row(16000, "synthetic-user-two", "Zoë 李")],
        );
        assert_eq!(rows.len(), 2);
        let mut reconnected = named_row(24000, "synthetic-user-two", "Zoë 李");
        reconnected.discord.as_mut().unwrap().generation += 1;
        append_rows(&mut rows, vec![reconnected]);
        assert_eq!(rows.len(), 3);
        append_rows(&mut rows, vec![row(32000, &[1], false, "Unknown again.")]);
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn names_reach_notes_and_unicode_exports_without_exporting_identifiers() {
        let rows = vec![named_row(0, "synthetic-private-id", "Zoë 李")];
        let names = [
            "Acoustic alias".into(),
            String::new(),
            String::new(),
            String::new(),
        ];
        assert_eq!(label(&rows[0], &names), "Zoë 李");
        let notes = crate::notes::Notes::build(&rows).text(&names);
        assert!(notes.contains("Zoë 李"));
        for format in [
            crate::call_export::Format::Text,
            crate::call_export::Format::Markdown,
            crate::call_export::Format::Srt,
            crate::call_export::Format::WebVtt,
        ] {
            let exported = crate::call_export::export(&rows, &names, format);
            assert!(exported.contains("Zoë 李"));
            assert!(exported.contains("résumé"));
            assert!(!exported.contains("synthetic-private-id"));
            assert!(!exported.contains("synthetic-channel"));
            assert!(!exported.contains("Acoustic alias"));
        }
        let json = serde_json::to_string(&rows).unwrap();
        assert!(!json.contains("synthetic-private-id"));
        assert!(!json.contains("synthetic-channel"));
    }
}
