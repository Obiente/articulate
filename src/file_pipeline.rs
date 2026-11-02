//! Transcribing recorded audio: files given to the command line, imported
//! recordings and retained call audio.
//!
//! `--transcribe`, `--call-file`, audio import and re-transcription all use
//! these transcribers, so a recording is processed the same way wherever it
//! comes from. Audio arrives in chunks and is processed in bounded windows cut
//! at quiet points, so memory does not grow with the recording.
use crate::{
    audio,
    calls::{self, Row},
    engine::Engine,
    speakers::Tracker,
};
use anyhow::Result;

/// Longest window given to the diarizer and recognizer at once.
const CALL_WINDOW: usize = 24 * 16_000;
/// Windows end at the quietest point of their last two seconds.
const SEARCH: usize = 2 * 16_000;

/// The decoded mono 16 kHz audio of a file in any supported format.
pub fn read_mono(path: &std::path::Path) -> Result<Vec<f32>> {
    let mut decoded = crate::media::decode_file(
        path,
        crate::media::Layout::Mono,
        &std::sync::atomic::AtomicBool::new(false),
        |_| {},
    )?;
    Ok(decoded.channels.pop().unwrap_or_default())
}

/// Window end within `pcm[..limit]`, cut at a quiet point.
fn cut(pcm: &[f32], limit: usize) -> usize {
    if pcm.len() <= limit {
        return pcm.len();
    }
    audio::quiet_boundary(pcm, 16_000, limit).max(limit - SEARCH)
}

/// A dictation: one track, recognized in quiet-bounded windows.
pub struct Dictation {
    pending: Vec<f32>,
    pieces: Vec<String>,
    verify: bool,
    /// Samples recognized so far.
    pub processed: usize,
}

impl Dictation {
    /// `verify` adds the second speech check to short windows, as finished
    /// dictations do.
    pub fn new(verify: bool) -> Self {
        Self {
            pending: Vec::new(),
            pieces: Vec::new(),
            verify,
            processed: 0,
        }
    }

    pub fn push(&mut self, engine: &mut Engine, pcm: &[f32]) -> Result<()> {
        let window = audio::WINDOW_SECONDS * 16_000;
        self.pending.extend_from_slice(pcm);
        while self.pending.len() >= window + SEARCH {
            let end = cut(&self.pending, window);
            self.recognize(engine, end)?;
        }
        Ok(())
    }

    fn recognize(&mut self, engine: &mut Engine, end: usize) -> Result<()> {
        let window: Vec<f32> = self.pending.drain(..end).collect();
        let text = if self.verify {
            engine.transcribe_final(&window)?
        } else {
            engine.transcribe(&window)?
        };
        if !text.is_empty() {
            self.pieces.push(text);
        }
        self.processed += end;
        Ok(())
    }

    pub fn finish(mut self, engine: &mut Engine) -> Result<String> {
        while !self.pending.is_empty() {
            let end = cut(&self.pending, audio::WINDOW_SECONDS * 16_000);
            self.recognize(engine, end)?;
        }
        Ok(self.pieces.join(" "))
    }
}

/// A conversation: an optional microphone track labelled You and the remote
/// track, attributed to speakers when the speaker model is available.
pub struct Call {
    tracker: Option<Tracker>,
    microphone: Vec<f32>,
    remote: Vec<f32>,
    /// Samples already transcribed.
    pub processed: usize,
}

impl Call {
    /// Start with a speaker tracker, or `None` to keep remote speech unattributed.
    pub fn new(tracker: Option<Tracker>) -> Self {
        Self {
            tracker,
            microphone: Vec::new(),
            remote: Vec::new(),
            processed: 0,
        }
    }

    /// Load the speaker model; `None` (with the reason) when it is unavailable.
    pub fn speakers(cpu: bool) -> (Option<Tracker>, Option<String>) {
        if !crate::speakers::path().is_file() {
            return (None, Some("The speaker model is not installed".into()));
        }
        match Tracker::new(cpu) {
            Ok(tracker) => (Some(tracker), None),
            Err(error) => (None, Some(format!("{error:#}"))),
        }
    }

    /// Add aligned audio. Either track may be empty when the source has none;
    /// it is treated as silence. Returns the rows of completed windows.
    pub fn push(
        &mut self,
        engine: &mut Engine,
        microphone: &[f32],
        remote: &[f32],
    ) -> Result<Vec<Row>> {
        self.microphone.extend_from_slice(microphone);
        self.remote.extend_from_slice(remote);
        let mut rows = Vec::new();
        while self.microphone.len().max(self.remote.len()) >= CALL_WINDOW + SEARCH {
            rows.extend(self.window(engine, false)?);
        }
        Ok(rows)
    }

    fn window(&mut self, engine: &mut Engine, last: bool) -> Result<Vec<Row>> {
        let available = self.microphone.len().max(self.remote.len());
        let guide = if self.remote.len() >= self.microphone.len() {
            &self.remote
        } else {
            &self.microphone
        };
        let end = if last && available <= CALL_WINDOW {
            available
        } else {
            cut(guide, CALL_WINDOW).min(available)
        };
        let take = |track: &mut Vec<f32>| {
            let count = end.min(track.len());
            let mut window: Vec<f32> = track.drain(..count).collect();
            window.resize(end, 0.0);
            window
        };
        let microphone = take(&mut self.microphone);
        let remote = take(&mut self.remote);
        let offset_ms = self.processed as u64 / 16;
        self.processed += end;
        match self.tracker.as_mut() {
            Some(tracker) => {
                calls::process_window(engine, tracker, &microphone, &remote, offset_ms, true)
            }
            None => {
                calls::process_unattributed_window(engine, &microphone, &remote, offset_ms, true)
            }
        }
    }

    /// Transcribe what remains. Returns the rows of the final windows.
    pub fn finish(mut self, engine: &mut Engine) -> Result<Vec<Row>> {
        let mut rows = Vec::new();
        while self.microphone.len().max(self.remote.len()) > 0 {
            rows.extend(self.window(engine, true)?);
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_end_at_a_quiet_point_within_the_search_range() {
        let mut pcm = vec![0.5f32; CALL_WINDOW + 4 * 16_000];
        for sample in &mut pcm[CALL_WINDOW - 16_000..CALL_WINDOW - 15_000] {
            *sample = 0.0;
        }
        let end = cut(&pcm, CALL_WINDOW);
        assert!(
            (CALL_WINDOW - 16_000..=CALL_WINDOW - 15_000).contains(&end),
            "{end}"
        );
        assert_eq!(cut(&pcm[..1000], CALL_WINDOW), 1000);
        // Without a quiet point the window still ends inside the search range.
        let loud = vec![0.5f32; CALL_WINDOW * 2];
        assert!(cut(&loud, CALL_WINDOW) >= CALL_WINDOW - SEARCH);
    }
}
