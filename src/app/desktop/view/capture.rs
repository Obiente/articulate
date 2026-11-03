//! Dictation, call and spoken-note capture.

use super::library::{SessionBody, SessionMeta};
use crate::app::{App, capture_feedback, desktop::publish::Collector, desktop::publish::RowStream};
use crate::{calls, history::Kind};
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct Capture<'a> {
    recording: bool,
    call_recording: bool,
    note_recording: bool,
    seconds: u64,
    call_status: &'a str,
    note_status: &'a str,
    call_cues_enabled: bool,
    call_cues_status: &'a str,
    capture_feedback: &'a capture_feedback::State,
}

pub(crate) fn capture(app: &App) -> Capture<'_> {
    let note_capture = app.is_note_capture();
    let cues = app.call_cues_enabled
        && app
            .history
            .call
            .as_ref()
            .is_some_and(|session| session.kind == Kind::Call);
    Capture {
        recording: app.recording.is_some(),
        call_recording: app.call.is_some() && !note_capture,
        note_recording: app.call.is_some() && note_capture,
        seconds: app
            .recording
            .as_ref()
            .map(|recording| recording.seconds() as u64)
            .or_else(|| app.call.as_ref().map(|call| call.end_seconds() as u64))
            .unwrap_or(app.last_seconds),
        call_status: &app.call_status,
        note_status: if note_capture { &app.call_status } else { "" },
        call_cues_enabled: cues,
        call_cues_status: if cues { &app.context_status } else { "" },
        capture_feedback: &app.capture_feedback,
    }
}

#[derive(Serialize)]
pub(crate) struct Dictation<'a> {
    text: &'a str,
    original: &'a str,
    dictation_interruption: Option<&'a str>,
    dictation_session: Option<SessionMeta<'a>>,
}

pub(crate) fn dictation(app: &App) -> Dictation<'_> {
    Dictation {
        text: &app.text,
        original: &app.raw,
        dictation_interruption: app.capture_interruption.as_deref(),
        dictation_session: app
            .history
            .dictation
            .as_ref()
            .filter(|_| !app.history.dictation_deleted)
            .map(SessionMeta::new),
    }
}

#[derive(Serialize)]
pub(crate) struct Bodies<'a> {
    dictation_body: Option<SessionBody<'a>>,
    call_body: Option<SessionBody<'a>>,
    note_body: Option<SessionBody<'a>>,
}
pub(crate) fn bodies(app: &App) -> Bodies<'_> {
    let current = app.history.call.as_ref();
    let note = app.is_note_capture();
    Bodies {
        dictation_body: app
            .history
            .dictation
            .as_ref()
            .filter(|_| !app.history.dictation_deleted)
            .map(SessionBody::new),
        call_body: current.filter(|_| !note).map(SessionBody::new),
        note_body: current.filter(|_| note).map(SessionBody::new),
    }
}

/// The current call or spoken note. Its transcript is in `live_rows`.
#[derive(Serialize)]
pub(crate) struct Live<'a> {
    call_session: Option<SessionMeta<'a>>,
    note_session: Option<SessionMeta<'a>>,
}

pub(crate) fn live_sessions(app: &App) -> Live<'_> {
    let note_capture = app.is_note_capture();
    let current = app.history.call.as_ref();
    Live {
        call_session: current.filter(|_| !note_capture).map(SessionMeta::new),
        note_session: current.filter(|_| note_capture).map(SessionMeta::new),
    }
}

/// Live call and spoken-note rows, sent as splices.
pub(crate) fn live_rows(app: &App, out: &mut Collector<'_>) {
    let note_capture = app.is_note_capture();
    let provisional = |index: usize| app.call_row_is_provisional(index);
    let none: &[calls::Row] = &[];
    out.rows(
        "live_rows",
        &[
            RowStream {
                key: "call_rows",
                rows: if note_capture { none } else { &app.call_rows },
                names: &app.speaker_names,
                provisional: &provisional,
            },
            RowStream {
                key: "note_rows",
                rows: if note_capture && app.history.call.is_some() {
                    &app.call_rows
                } else {
                    none
                },
                names: &app.speaker_names,
                provisional: &provisional,
            },
        ],
    );
}
