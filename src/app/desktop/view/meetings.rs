//! Meeting detection for Teams, Zoom and Google Meet.
//!
//! Built by the Discord module (`App::desktop_meetings_status`), which owns
//! the detector and the capture it starts.

use crate::app::App;
use crate::meetings;
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct MeetingApp {
    pub id: &'static str,
    pub label: &'static str,
    pub best_effort: bool,
}

impl From<meetings::App> for MeetingApp {
    fn from(app: meetings::App) -> Self {
        Self {
            id: app.id(),
            label: app.label(),
            best_effort: app.best_effort(),
        }
    }
}

/// A confirmed start or end, newest last.
#[derive(Serialize)]
pub(crate) struct MeetingEvent {
    pub app: &'static str,
    pub label: &'static str,
    pub kind: &'static str,
    pub at_unix_ms: u64,
}

#[derive(Serialize)]
pub(crate) struct MeetingsView<'a> {
    /// At least one application is watched.
    pub enabled: bool,
    pub settings: &'a meetings::Settings,
    pub consent_reminder: &'static str,
    /// Applications currently confirmed to be in a meeting.
    pub active: Vec<MeetingApp>,
    /// A detected meeting waiting for `meeting_respond`.
    pub prompt: Option<MeetingApp>,
    /// The meeting whose transcript this feature started and will finish.
    pub owned: Option<MeetingApp>,
    pub status: &'a str,
    pub probe_error: Option<&'a str>,
    pub events: Vec<MeetingEvent>,
}

#[derive(Serialize)]
pub(crate) struct Meetings<'a> {
    meetings: MeetingsView<'a>,
}

pub(crate) fn meetings(app: &App) -> Meetings<'_> {
    Meetings {
        meetings: app.desktop_meetings_status(),
    }
}
