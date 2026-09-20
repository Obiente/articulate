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

mod native;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Row {
    pub start_ms: u64,
    pub end_ms: u64,
    pub microphone: bool,
    pub speakers: Vec<i32>,
    /// A session-local match to observed Discord activity, independent of acoustic IDs.
    pub discord: Option<crate::discord_attribution::Attribution>,
    pub text: String,
}

pub fn append_rows(transcript: &mut Vec<Row>, incoming: Vec<Row>) {
    for row in incoming {
        let known_single_speaker = row.microphone
            || row
                .discord
                .as_ref()
                .is_some_and(|named| named.speakers.len() == 1)
            || (row.speakers.len() == 1 && (1..=4).contains(&row.speakers[0]));
        if let Some(last) = transcript.last_mut()
            && known_single_speaker
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
    /// Native capture is armed; true means an authenticated PCM frame arrived.
    NativeAudio(bool),
    Levels(f32, f32),
    /// Replace the current uncommitted text as more audio context arrives.
    Preview(Vec<Row>),
    /// Append committed text and clear the current preview.
    Rows(Vec<Row>),
}
pub struct Request {
    pub microphone: Option<String>,
    pub output: Option<String>,
    pub cpu: bool,
    pub native_audio: bool,
    pub control: Arc<Control>,
}

pub fn process_window(
    engine: &mut Engine,
    tracker: &mut Tracker,
    mic: &[f32],
    remote: &[f32],
    offset_ms: u64,
) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    let text = engine.transcribe(mic)?;
    if !text.is_empty() {
        rows.push(Row {
            start_ms: offset_ms,
            end_ms: offset_ms + mic.len() as u64 / 16,
            microphone: true,
            speakers: Vec::new(),
            discord: None,
            text,
        });
    }
    let turns = tracker.identify(remote)?;
    for turn in crate::call_segments::plan(&turns, remote.len()) {
        let text = engine.transcribe(&remote[turn.audio_start..turn.audio_end])?;
        if text.is_empty() {
            continue;
        }
        rows.push(Row {
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

fn process_activity_window(
    engine: &mut Engine,
    mic: &[f32],
    remote: &[f32],
    offset_ms: u64,
    segments: &[crate::discord_attribution::ActivitySegment],
) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    let text = engine.transcribe(mic)?;
    if !text.is_empty() {
        rows.push(Row {
            start_ms: offset_ms,
            end_ms: offset_ms + mic.len() as u64 / 16,
            microphone: true,
            speakers: Vec::new(),
            discord: None,
            text,
        });
    }
    for segment in segments {
        let text = engine.transcribe(&remote[segment.audio_start..segment.audio_end])?;
        if !text.is_empty() {
            rows.push(Row {
                start_ms: offset_ms + segment.start as u64 / 16,
                end_ms: offset_ms + segment.end as u64 / 16,
                microphone: false,
                speakers: Vec::new(),
                discord: Some(segment.attribution.clone()),
                text,
            });
        }
    }
    rows.sort_by_key(|row| row.start_ms);
    Ok(rows)
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

    fn endpoint(&self) -> bool {
        self.mic.len().max(self.remote.len()) >= MAX_DRAFT_SAMPLES
            || (quiet_tail(&self.mic) && quiet_tail(&self.remote))
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
    if request.native_audio {
        return native::run(engine, request, update);
    }
    let mut tracker: Option<Tracker> = None;
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
    let mut output = Track::open(request.output.as_deref(), true, request.control.clone())?;
    update(Update::Started);
    let mut cursor = 0.0;
    let mut draft_start = 0.0;
    let mut draft = Draft::default();
    let mut checkpoint = None;
    let result = (|| -> Result<()> {
        loop {
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
                draft.remote.extend(output.take_until(end)?);
                let offset_ms = (draft_start * 1000.0) as u64;
                let discord_context = request.control.discord().zip(request.control.origin()).map(
                    |(discord, origin)| {
                        let from = origin + Duration::from_secs_f64(draft_start);
                        let to = origin + Duration::from_secs_f64(end);
                        (origin, discord.history(from, to))
                    },
                );
                let activity = discord_context.as_ref().and_then(|(origin, history)| {
                    crate::discord_attribution::plan_activity(
                        *origin,
                        offset_ms,
                        draft.remote.len(),
                        history,
                    )
                });
                let rows = if let Some(segments) = activity {
                    process_activity_window(
                        engine,
                        &draft.mic,
                        &draft.remote,
                        offset_ms,
                        &segments,
                    )?
                } else {
                    if tracker.is_none() {
                        let fallback=Tracker::new(request.cpu).map_err(|error|anyhow::anyhow!("Speaker activity is unavailable and acoustic speaker recognition could not load: {error}. Prepare speaker recognition in call Setup for offline fallback."))?;
                        checkpoint = Some(fallback.checkpoint());
                        tracker = Some(fallback);
                    }
                    let fallback = tracker.as_mut().expect("fallback was initialized");
                    fallback.restore(
                        checkpoint
                            .as_ref()
                            .expect("fallback checkpoint initialized"),
                    );
                    let mut rows =
                        process_window(engine, fallback, &draft.mic, &draft.remote, offset_ms)?;
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
                if draft.endpoint() || (stopping && cursor >= now) {
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
                    f32::from_bits(output.level.load(Ordering::Relaxed)),
                ));
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        Ok(())
    })();
    // An incomplete decode never replaces the last successful preview. Flushing
    // it also preserves text when capture fails or the user cancels mid-phrase.
    draft.commit(&mut update);
    result
}

pub fn label(row: &Row, names: &[String; 4]) -> String {
    if row.microphone {
        return "You".into();
    }
    if let Some(attribution) = &row.discord {
        let names: Vec<_> = attribution
            .speakers
            .iter()
            .map(|speaker| speaker.name.as_str())
            .collect();
        if names.len() == 1 {
            return names[0].to_owned();
        }
        if names.len() > 1 {
            return format!("Overlap: {}", names.join(" + "));
        }
    }
    let labels: Vec<_> = row
        .speakers
        .iter()
        .map(|&id| {
            if !(1..=4).contains(&id) {
                "Uncertain speaker".into()
            } else if names[id as usize - 1].trim().is_empty() {
                format!("Speaker {id}")
            } else {
                names[id as usize - 1].clone()
            }
        })
        .collect();
    if labels.len() > 1 {
        format!("Overlap: {}", labels.join(" + "))
    } else {
        labels.join("")
    }
}

pub fn text(rows: &[Row], names: &[String; 4]) -> String {
    rows.iter()
        .map(|r| {
            format!(
                "[{:02}:{:02}] {}: {}",
                r.start_ms / 60000,
                r.start_ms / 1000 % 60,
                label(r, names),
                r.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!draft.endpoint());
        draft.mic.extend(vec![0.0; 4 * 16_000]);
        draft.remote.extend(vec![0.1; 4 * 16_000]);
        assert!(!draft.endpoint());
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
        assert!(!draft.endpoint());
        draft.remote.fill(0.0);
        assert!(draft.endpoint());
        draft.remote.fill(f32::NAN);
        assert!(!draft.endpoint());
        draft.remote.resize(MAX_DRAFT_SAMPLES, 0.1);
        assert!(draft.endpoint());
        assert_eq!(draft.remaining_seconds(), 0.0);
        draft.commit(&mut |_| {});
        assert_eq!(draft.remaining_seconds(), 24.0);
    }

    fn row(start: u64, speakers: &[i32], microphone: bool, text: &str) -> Row {
        Row {
            start_ms: start,
            end_ms: start + 8000,
            microphone,
            speakers: speakers.to_vec(),
            discord: None,
            text: text.into(),
        }
    }

    #[test]
    fn consecutive_windows_share_one_label_and_keep_all_text() {
        let mut rows = vec![row(0, &[1], false, "First sentence.")];
        append_rows(&mut rows, vec![row(8000, &[1], false, "Second sentence.")]);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].start_ms, rows[0].end_ms), (0, 16000));
        assert_eq!(
            text(&rows, &Default::default()),
            "[00:00] Speaker 1: First sentence. Second sentence."
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
        assert_eq!(label(rows.last().unwrap(), &Default::default()), "You");
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
        assert_eq!(label(&rows[0], &Default::default()), "Zoë 李");
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
