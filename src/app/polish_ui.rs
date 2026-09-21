use super::App;
use crate::polish::{Event, Job, Preview, Request, Worker};

#[derive(Default)]
pub(super) struct State {
    worker: Worker,
    job: Option<Job>,
    context: Option<Context>,
    preview: Option<Preview>,
    status: String,
    fraction: Option<f32>,
}

struct Context {
    utterance: u64,
    original: String,
    source: String,
}
impl Context {
    fn matches(&self, utterance: u64, original: &str, source: &str) -> bool {
        self.utterance == utterance && self.original == original && self.source == source
    }
}

impl App {
    pub(super) fn desktop_cleanup_status(&self) -> serde_json::Value {
        serde_json::json!({"installed":crate::polish::profile_installed(crate::polish::ModelProfile::Polish),
            "working":self.polish.job.is_some(),"status":self.polish.status,"progress":self.polish.fraction})
    }

    pub(super) fn desktop_cleanup_download(&mut self) -> Result<(), String> {
        if self.polish.job.is_some() {
            return Err("The local editor is already busy.".into());
        }
        self.polish.context = None;
        self.polish.status = "Downloading the local editor…".into();
        self.polish.job = Some(crate::polish::download_profile(
            crate::polish::ModelProfile::Polish,
        ));
        Ok(())
    }

    pub(super) fn desktop_cleanup_cancel(&mut self) {
        if let Some(job) = &self.polish.job {
            job.cancel();
        }
    }
    pub(super) fn polish_working(&self) -> bool {
        self.polish.job.is_some()
    }

    pub(super) fn release_polish_runtime(&mut self) {
        if self.polish.job.is_none() {
            self.polish.worker = Worker::default();
        }
    }

    fn polish_idle(&self) -> bool {
        self.recording.is_none()
            && self.call.is_none()
            && !self.brain_working()
            && !self.busy
            && !self.preview_inflight
            && !self.integration_inflight
            && self.integration_pending.is_none()
    }

    pub(super) fn polish_poll(&mut self) {
        if self.polish.context.as_ref().is_some_and(|context| {
            !self.polish_idle() || !context.matches(self.utterance, &self.raw, &self.text)
        }) {
            self.polish.job = None;
            self.polish.context = None;
            self.polish.preview = None;
            self.polish.status =
                "Your text changed. Polish the updated text when you are ready.".into();
        }
        loop {
            let event = self.polish.job.as_ref().map(Job::try_recv);
            match event {
                Some(Ok(Event::Progress { stage, fraction })) => {
                    self.polish.status = stage;
                    self.polish.fraction = fraction;
                }
                Some(Ok(event)) => {
                    self.polish.job = None;
                    self.polish.fraction = None;
                    match event {
                        Event::Installed => {
                            self.polish.status = "Ready. Choose Polish text to try it.".into()
                        }
                        Event::Complete(preview) => {
                            self.polish.status = if preview.cleanup_only {
                                "Spoken repeats cleaned up. Other wording was kept unchanged."
                                    .into()
                            } else if let Some(reason) = &preview.blocked {
                                format!("Original kept: {reason}")
                            } else if preview.text == preview.source {
                                "Your wording already reads clearly.".into()
                            } else {
                                "Review the changes before using them.".into()
                            };
                            self.polish.preview = Some(preview);
                        }
                        Event::Failed(error) => self.polish.status = error,
                        Event::Cancelled => {
                            self.polish.status = "Cancelled. Your text is unchanged.".into()
                        }
                        Event::Progress { .. } => unreachable!(),
                    }
                    break;
                }
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                    self.polish.job = None;
                    self.polish.status = "The local editor stopped. Try again.".into();
                    break;
                }
                _ => break,
            }
        }
    }

    #[allow(
        dead_code,
        reason = "Local polish controller retained for the Tauri editing flow"
    )]
    fn polish_start(&mut self) {
        if !self.polish_idle() || self.macro_used.is_some() || self.polish.job.is_some() {
            return;
        }
        let request = Request {
            original: self.raw.clone(),
            source: self.text.clone(),
            style: self.settings.polish_style,
            protected: self
                .settings
                .entries
                .iter()
                .filter(|entry| entry.enabled)
                .map(|entry| entry.wanted.clone())
                .collect(),
        };
        match self.polish.worker.start(request) {
            Ok(job) => {
                self.polish.context = Some(Context {
                    utterance: self.utterance,
                    original: self.raw.clone(),
                    source: self.text.clone(),
                });
                self.polish.job = Some(job);
                self.polish.preview = None;
                self.polish.status = "Preparing your preview…".into();
            }
            Err(error) => self.polish.status = error.to_string(),
        }
    }

    #[allow(
        dead_code,
        reason = "Local polish controller retained for the Tauri editing flow"
    )]
    fn polish_apply(&mut self) {
        let valid = self.polish_idle()
            && self
                .polish
                .context
                .as_ref()
                .is_some_and(|context| context.matches(self.utterance, &self.raw, &self.text));
        if !valid {
            self.polish_poll();
            return;
        }
        if let Some(preview) = self
            .polish
            .preview
            .take()
            .filter(|preview| preview.blocked.is_none() || preview.cleanup_only)
        {
            self.text = preview.text;
            self.edit_baseline.clone_from(&self.text);
            self.edited_at = None;
            self.history_dictation_changed();
            self.polish.context = None;
            self.status = "Polished preview saved. Copy it when ready.".into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{App, Context, Preview};

    fn prepared_preview(app: &mut App) {
        app.raw = "um hi Casey can you review the plan before tomorrow".into();
        app.text = "Hi Casey can you review the plan before tomorrow".into();
        app.polish.context = Some(Context {
            utterance: app.utterance,
            original: app.raw.clone(),
            source: app.text.clone(),
        });
        app.polish.preview = Some(Preview {
            original: app.raw.clone(),
            source: app.text.clone(),
            text: "Hi Casey, can you review the plan before tomorrow?".into(),
            blocked: None,
            cleanup_only: false,
            elapsed_ms: 950,
        });
    }

    #[test]
    fn previews_cannot_apply_to_another_utterance_or_an_edited_transcript() {
        let context = Context {
            utterance: 7,
            original: "um hello".into(),
            source: "Hello.".into(),
        };
        assert!(context.matches(7, "um hello", "Hello."));
        assert!(!context.matches(8, "um hello", "Hello."));
        assert!(!context.matches(7, "hello", "Hello."));
        assert!(!context.matches(7, "um hello", "Hello Casey."));
    }

    #[test]
    fn applying_preview_preserves_raw_and_does_not_learn_generated_edits() {
        let (mut app, _) = super::super::tests::app();
        prepared_preview(&mut app);
        app.preview_inflight = false;
        let original = app.raw.clone();
        app.edited_at = Some(std::time::Instant::now());
        app.polish_apply();
        assert!(app.text.starts_with("Hi Casey,"));
        assert_eq!(app.raw, original);
        assert_eq!(app.edit_baseline, app.text);
        assert!(app.edited_at.is_none());
        assert!(app.remembered.is_empty());
    }

    #[test]
    fn speech_cleanup_can_be_applied_when_further_model_edits_were_rejected() {
        let (mut app, _) = super::super::tests::app();
        prepared_preview(&mut app);
        app.preview_inflight = false;
        let preview = app.polish.preview.as_mut().unwrap();
        preview.cleanup_only = true;
        preview.blocked = Some("The draft changed a number.".into());
        let expected = preview.text.clone();
        let original = app.raw.clone();
        app.polish_apply();
        assert_eq!(app.text, expected);
        assert_eq!(app.raw, original);
        assert!(app.remembered.is_empty());
    }

    #[test]
    fn apply_rechecks_text_even_before_event_polling() {
        let (mut app, _) = super::super::tests::app();
        prepared_preview(&mut app);
        app.preview_inflight = false;
        app.text = "A newer draft.".into();
        app.polish_apply();
        assert_eq!(app.text, "A newer draft.");
        assert!(app.polish.preview.is_none());
    }
}
