//! Application status, controller health and the dictation indicator.

use crate::app::App;
use serde::Serialize;
use std::{sync::atomic::Ordering, time::Instant};

#[derive(Serialize)]
pub(crate) struct Shell<'a> {
    ready: bool,
    loading: bool,
    busy: bool,
    status: &'a str,
    saving: bool,
    history_loading: bool,
    history_error: Option<&'a str>,
    filing_status: &'a str,
    filing_working: bool,
    source_move_working: bool,
    summary_working: bool,
}

pub(crate) fn shell(app: &App) -> Shell<'_> {
    Shell {
        ready: app.ready,
        loading: app.loading,
        busy: app.busy,
        status: &app.status,
        saving: app.history.selected_dirty.is_some()
            || app.history.call_dirty.is_some()
            || app.history.dictation_dirty.is_some()
            || app
                .history
                .worker
                .as_ref()
                .is_some_and(|worker| worker.saves_settled() == Ok(false)),
        history_loading: app.history.loading,
        history_error: app.history.error.as_deref(),
        filing_status: &app.notetaker.status,
        filing_working: app.notetaker_routing_busy(),
        source_move_working: app.desktop_editors_locked(),
        summary_working: app.brain_working(),
    }
}

/// Whether the native controller is available to the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ControllerState {
    Starting,
    Running,
    /// The controller stopped after an error. Saves cannot be verified.
    Failed,
    /// The controller did not answer a close request in time.
    Unresponsive,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ControllerHealth {
    pub state: ControllerState,
    pub message: Option<String>,
    /// Closing without verified saves is allowed.
    pub can_close_anyway: bool,
}

impl ControllerHealth {
    pub(crate) fn new(state: ControllerState, message: Option<String>) -> Self {
        Self {
            can_close_anyway: matches!(
                state,
                ControllerState::Failed | ControllerState::Unresponsive
            ),
            state,
            message,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct ControllerSection<'a> {
    pub controller: &'a ControllerHealth,
}

/// The shell shown before the controller publishes its first state, or after
/// startup failed.
pub(crate) fn starting(status: &str, loading: bool) -> Shell<'_> {
    Shell {
        ready: false,
        loading,
        busy: false,
        status,
        saving: false,
        history_loading: true,
        history_error: None,
        filing_status: "",
        filing_working: false,
        source_move_working: false,
        summary_working: false,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct Overlay {
    pub visible: bool,
    pub recording: bool,
    pub finishing: bool,
    pub message: String,
}

#[derive(Serialize)]
pub(crate) struct OverlaySection {
    overlay: Overlay,
}

pub(crate) fn overlay_section(app: &App) -> OverlaySection {
    OverlaySection {
        overlay: overlay(app),
    }
}

/// Indicator state. It never includes transcript text.
pub(crate) fn overlay(app: &App) -> Overlay {
    let note_recording = app.is_note_capture() && app.call.is_some();
    let note_stopping = note_recording
        && app
            .call
            .as_ref()
            .is_some_and(|call| call.stop_ns.load(Ordering::SeqCst) != 0);
    let note_listening = note_recording && !note_stopping && app.capture_feedback.is_recording();
    let recording = (app.target.is_some() && app.recording.is_some()) || note_listening;
    let finishing = (app.target.is_some() && app.busy) || note_stopping;
    let visible = recording
        || finishing
        || app
            .overlay_until
            .is_some_and(|until| until > Instant::now());
    let message = if note_listening {
        format!(
            "Recording a note · {} to finish",
            app.settings.quick_note_hotkey.label()
        )
    } else if recording {
        app.shortcut_instruction()
    } else if finishing {
        if note_stopping {
            "Recording stopped. Finishing your spoken note…"
        } else {
            "Finishing your transcript…"
        }
        .into()
    } else {
        app.overlay_message.clone()
    };
    Overlay {
        visible,
        recording,
        finishing,
        message,
    }
}
