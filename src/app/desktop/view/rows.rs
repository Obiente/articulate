//! Transcript rows as the renderer displays them.

use crate::{calls, sensevoice::Cue};
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct RowView<'a> {
    start_ms: u64,
    end_ms: u64,
    speaker: String,
    speaker_labels: Vec<String>,
    text: &'a str,
    cues: &'a [Cue],
    provisional: bool,
}

impl<'a> RowView<'a> {
    pub(crate) fn new(row: &'a calls::Row, names: &[String], provisional: bool) -> Self {
        Self {
            start_ms: row.start_ms,
            end_ms: row.end_ms,
            speaker: calls::label(row, names),
            speaker_labels: calls::speaker_labels(row, names),
            text: &row.text,
            cues: &row.cues,
            provisional,
        }
    }

    /// Saved rows are never provisional.
    pub(crate) fn saved(rows: &'a [calls::Row], names: &[String]) -> Vec<Self> {
        rows.iter()
            .map(|row| Self::new(row, names, false))
            .collect()
    }
}
