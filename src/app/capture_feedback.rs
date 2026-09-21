use super::App;
use crate::audio_cues::Cue;
use serde::Serialize;
use std::sync::atomic::Ordering;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    Call,
    Note,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Event {
    Started,
    Stopped,
    Failed,
}

#[derive(Default, Serialize)]
pub(super) struct State {
    pub sequence: u64,
    pub event: Option<Event>,
    pub kind: Option<Kind>,
    pub session_id: Option<String>,
    pub status: String,
    #[serde(skip)]
    lifecycle: Option<Lifecycle>,
}

struct Lifecycle {
    kind: Kind,
    id: Option<String>,
    started: bool,
    stopped: bool,
}

impl State {
    pub(super) fn is_recording(&self) -> bool {
        self.lifecycle
            .as_ref()
            .is_some_and(|lifecycle| lifecycle.started && !lifecycle.stopped)
    }

    pub(super) fn prepare(&mut self, kind: Kind, id: Option<String>) {
        self.lifecycle = Some(Lifecycle {
            kind,
            id,
            started: false,
            stopped: false,
        });
    }

    fn publish(&mut self, event: Event, status: String) {
        let Some(lifecycle) = &self.lifecycle else {
            return;
        };
        self.sequence = self.sequence.saturating_add(1);
        self.event = Some(event);
        self.kind = Some(lifecycle.kind);
        self.session_id.clone_from(&lifecycle.id);
        self.status = status;
    }

    fn started(&mut self, stopping: bool) -> Option<Cue> {
        let lifecycle = self.lifecycle.as_mut()?;
        if lifecycle.started {
            return None;
        }
        lifecycle.started = true;
        if stopping {
            return self.stopped();
        }
        let status = match lifecycle.kind {
            Kind::Call => "Call recording started",
            Kind::Note => "Spoken note recording started",
        };
        self.publish(Event::Started, status.into());
        Some(Cue::Started)
    }

    fn stopped(&mut self) -> Option<Cue> {
        let lifecycle = self.lifecycle.as_mut()?;
        if !lifecycle.started || lifecycle.stopped {
            return None;
        }
        lifecycle.stopped = true;
        let status = match lifecycle.kind {
            Kind::Call => "Recording stopped. Finishing your call transcript…",
            Kind::Note => "Recording stopped. Finishing your spoken note…",
        };
        self.publish(Event::Stopped, status.into());
        Some(Cue::Stopped)
    }

    fn finished(&mut self, error: Option<&str>) -> Option<Cue> {
        let cue = if let Some(error) = error {
            let cue = self.stopped();
            self.publish(Event::Failed, error.into());
            cue
        } else {
            self.stopped()
        };
        self.lifecycle = None;
        cue
    }
}

impl App {
    fn capture_feedback_notice(&mut self, previous_sequence: u64) {
        if self.capture_feedback.sequence != previous_sequence {
            self.overlay_message
                .clone_from(&self.capture_feedback.status);
            self.overlay_until = Some(
                std::time::Instant::now()
                    + std::time::Duration::from_secs(
                        if self.capture_feedback.event == Some(Event::Failed) {
                            6
                        } else {
                            3
                        },
                    ),
            );
        }
    }

    pub(super) fn capture_started_feedback(&mut self) {
        let Some(control) = &self.call else {
            return;
        };
        if self.capture_feedback.lifecycle.is_none() {
            let kind = if self.is_note_capture() {
                Kind::Note
            } else {
                Kind::Call
            };
            self.capture_feedback.prepare(
                kind,
                self.history.call.as_ref().map(|session| session.id.clone()),
            );
        }
        let stopping = control.stop_ns.load(Ordering::SeqCst) != 0;
        let previous_sequence = self.capture_feedback.sequence;
        if let Some(cue) = self.capture_feedback.started(stopping)
            && self.settings.audio_feedback
        {
            crate::audio_cues::play(cue);
        }
        self.capture_feedback_notice(previous_sequence);
    }

    pub(super) fn poll_capture_stop_feedback(&mut self) {
        let previous_sequence = self.capture_feedback.sequence;
        if self
            .call
            .as_ref()
            .is_some_and(|control| control.stop_ns.load(Ordering::SeqCst) != 0)
            && let Some(cue) = self.capture_feedback.stopped()
            && self.settings.audio_feedback
        {
            crate::audio_cues::play(cue);
        }
        self.capture_feedback_notice(previous_sequence);
    }

    pub(super) fn capture_finished_feedback(&mut self, error: Option<&str>) {
        let previous_sequence = self.capture_feedback.sequence;
        if let Some(cue) = self.capture_feedback.finished(error)
            && self.settings.audio_feedback
        {
            crate::audio_cues::play(cue);
        }
        self.capture_feedback_notice(previous_sequence);
    }

    pub(super) fn capture_failed_feedback(&mut self, note: bool, error: &str) {
        self.capture_feedback
            .prepare(if note { Kind::Note } else { Kind::Call }, None);
        self.capture_finished_feedback(Some(error));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparing_does_not_claim_success_and_failed_start_has_no_success_cue() {
        let mut state = State::default();
        state.prepare(Kind::Call, Some("synthetic-call".into()));
        assert_eq!(state.sequence, 0);
        assert!(state.finished(Some("Microphone unavailable")).is_none());
        assert_eq!(state.event, Some(Event::Failed));
        assert_eq!(state.sequence, 1);
        assert_eq!(state.session_id.as_deref(), Some("synthetic-call"));
    }

    #[test]
    fn stop_feedback_precedes_final_asr_and_is_only_emitted_once() {
        let mut state = State::default();
        state.prepare(Kind::Note, Some("synthetic-note".into()));
        assert!(matches!(state.started(false), Some(Cue::Started)));
        assert!(state.started(false).is_none());
        assert!(matches!(state.stopped(), Some(Cue::Stopped)));
        assert_eq!(state.event, Some(Event::Stopped));
        assert_eq!(state.sequence, 2);
        assert!(state.stopped().is_none());
        assert!(state.finished(None).is_none());
        assert_eq!(state.sequence, 2);
    }

    #[test]
    fn stop_before_start_event_does_not_play_a_delayed_start_sound() {
        let mut state = State::default();
        state.prepare(Kind::Call, Some("short-call".into()));
        assert!(state.stopped().is_none());
        assert!(matches!(state.started(true), Some(Cue::Stopped)));
        assert_eq!(state.event, Some(Event::Stopped));
        assert_eq!(state.sequence, 1);
    }

    #[test]
    fn unexpected_capture_failure_still_announces_actual_stop_once() {
        let mut state = State::default();
        state.prepare(Kind::Call, Some("interrupted-call".into()));
        assert!(!state.is_recording());
        assert!(matches!(state.started(false), Some(Cue::Started)));
        assert!(state.is_recording());
        assert!(matches!(
            state.finished(Some("Audio device disconnected")),
            Some(Cue::Stopped)
        ));
        assert!(!state.is_recording());
        assert_eq!(state.event, Some(Event::Failed));
        assert_eq!(state.sequence, 3);
        assert!(state.finished(Some("Audio device disconnected")).is_none());
        assert_eq!(state.sequence, 3);
    }

    #[test]
    fn muted_audio_still_publishes_actual_capture_lifecycle() {
        let (mut app, _) = super::super::tests::app();
        app.settings.audio_feedback = false;
        app.call = Some(crate::call_capture::Control::new());
        let session = crate::history::Session::new(crate::history::Kind::Note);
        let id = session.id.clone();
        app.history.call = Some(session);
        app.capture_started_feedback();
        assert_eq!(app.capture_feedback.event, Some(Event::Started));
        assert_eq!(app.capture_feedback.kind, Some(Kind::Note));
        assert_eq!(
            app.capture_feedback.session_id.as_deref(),
            Some(id.as_str())
        );
        app.call.as_ref().unwrap().stop();
        app.poll_capture_stop_feedback();
        assert_eq!(app.capture_feedback.event, Some(Event::Stopped));
        app.capture_finished_feedback(None);
        assert_eq!(app.capture_feedback.sequence, 2);
    }
}
