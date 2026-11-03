use super::App;
use crate::polish::{Event, Job, Preview, Request, Worker};

#[derive(Default)]
pub(super) struct State {
    pub(super) assort: super::assort_ui::ReviewState,
    worker: Worker,
    automatic: Option<std::time::Instant>,
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
    pub(super) fn desktop_cleanup_status(&self) -> CleanupStatus<'_> {
        CleanupStatus {
            installed: crate::polish::profile_installed(crate::polish::ModelProfile::Polish),
            working: self.polish.job.is_some(),
            status: &self.polish.status,
            progress: self.polish.fraction,
            preview: self.polish.preview.as_ref().map(|p| CleanupPreview {
                original: &p.original,
                source: &p.source,
                text: &p.text,
                blocked: p.blocked.as_deref(),
                cleanup_only: p.cleanup_only,
            }),
        }
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
        if self.polish.automatic.take().is_some() {
            self.target = None;
            self.status = "Cleanup cancelled. Your transcript is ready to copy.".into();
            self.overlay_message.clone_from(&self.status);
        }
        self.polish.preview = None;
        self.polish.context = None;
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
        !self.polish.assort.working()
            && self.recording.is_none()
            && self.call.is_none()
            && !self.brain_working()
            && !self.busy
            && !self.preview_inflight
            && !self.integration_inflight
            && self.integration_pending.is_none()
    }

    pub(super) fn polish_poll(&mut self) {
        if self.polish.automatic.is_some() {
            self.auto_polish_poll();
            return;
        }
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
                            if self.polish.context.is_none() {
                                self.polish.status = "Cancelled. Your text is unchanged.".into();
                                break;
                            }
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

    pub(super) fn auto_polish_start(&mut self) -> bool {
        if !self.settings.auto_polish
            || self.macro_used.is_some()
            || self.dictation_style() == crate::writing_style::WritingStyle::Verbatim
            || self.polish.job.is_some()
            || !crate::polish::profile_installed(crate::polish::ModelProfile::Polish)
        {
            return false;
        }
        let started = std::time::Instant::now();
        self.polish_begin();
        if self.polish.job.is_none() {
            return false;
        }
        self.polish.automatic = Some(started);
        self.status = "Cleaning up...".into();
        self.polish.status.clone_from(&self.status);
        self.overlay_message.clone_from(&self.status);
        self.overlay_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
        true
    }

    fn auto_polish_poll(&mut self) {
        let valid = !self.insertion_cancel.is_cancelled()
            && self.recording.is_none()
            && self
                .polish
                .context
                .as_ref()
                .is_some_and(|c| c.matches(self.utterance, &self.raw, &self.text));
        if !valid {
            self.polish.job = None;
            self.polish.context = None;
            self.polish.automatic = None;
            return;
        }
        // Four seconds includes queueing and model loading. Never prolong insertion
        // for a cold start; cancellation also releases the owned runtime.
        if self.polish.automatic.unwrap().elapsed() >= std::time::Duration::from_secs(4) {
            self.auto_polish_finish(None);
            return;
        }
        loop {
            match self.polish.job.as_ref().map(Job::try_recv) {
                Some(Ok(Event::Progress { .. })) => continue,
                Some(Ok(Event::Complete(preview))) => {
                    self.auto_polish_finish(preview.blocked.is_none().then_some(preview.text));
                }
                Some(Ok(_)) | Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) | None => {
                    self.auto_polish_finish(None)
                }
                Some(Err(std::sync::mpsc::TryRecvError::Empty)) => {}
            }
            break;
        }
    }

    fn auto_polish_finish(&mut self, text: Option<String>) {
        self.polish.job = None;
        self.polish.context = None;
        self.polish.automatic = None;
        if let Some(text) = text {
            self.text = text;
        }
        self.edit_baseline.clone_from(&self.text);
        self.edited_at = None;
        self.last_dictation_text.clone_from(&self.text);
        self.history_save_dictation();
        self.status = "Your transcript is ready".into();
        if self.insert_enabled()
            && (!self.text.is_empty() || self.live_mode)
            && let Some(target) = self.target.take()
        {
            let action = crate::integration::Action::Insert {
                id: self.utterance,
                target,
                text: self.text.clone(),
                learn: false,
                cancel: self.insertion_cancel.clone(),
            };
            if self.queue_integration(action) {
                self.busy = true;
                self.status = "Typing into your app...".into();
            } else {
                self.status =
                    "App insertion is unavailable. Your transcript is ready to copy.".into();
            }
        }
        self.target = None;
        self.overlay_message.clone_from(&self.status);
        self.overlay_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
    }

    #[allow(
        dead_code,
        reason = "Local polish controller retained for the Tauri editing flow"
    )]
    pub(super) fn polish_start(&mut self) {
        if !self.polish_idle() || self.macro_used.is_some() || self.polish.job.is_some() {
            return;
        }
        self.polish_begin();
    }

    fn polish_begin(&mut self) {
        let mut request = Request {
            original: self.raw.clone(),
            source: self.text.clone(),
            style: self.settings.polish_style,
            protected: crate::polish::vocabulary::scoped(
                &self.settings.entries,
                self.dictation_app.as_deref(),
            )
            .map(|entry| entry.wanted.clone())
            .collect(),
            vocabulary: Vec::new(),
        };
        if let Err(error) = request.validate() {
            self.polish.status = error.to_string();
            return;
        }
        request.vocabulary = crate::polish::vocabulary::select(
            &self.text,
            &self.settings.entries,
            self.dictation_app.as_deref(),
        );
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
    pub(super) fn polish_apply(&mut self) {
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

#[derive(serde::Serialize)]
pub(crate) struct CleanupStatus<'a> {
    pub installed: bool,
    pub working: bool,
    pub status: &'a str,
    pub progress: Option<f32>,
    pub preview: Option<CleanupPreview<'a>>,
}
#[derive(serde::Serialize)]
pub(crate) struct CleanupPreview<'a> {
    original: &'a str,
    source: &'a str,
    text: &'a str,
    blocked: Option<&'a str>,
    cleanup_only: bool,
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
    fn automatic_result_replaces_live_preview_without_learning() {
        let (mut app, _) = super::super::tests::app();
        prepared_preview(&mut app);
        app.settings.insert = true;
        app.settings.learn_corrections = true;
        app.live_mode = true;
        app.target = Some(crate::platform::test_target());
        app.integration_inflight = true;
        let preview = app.polish.preview.take().unwrap();
        let expected = preview.text.clone();
        let (job, tx) = crate::polish::Job::test_channel();
        app.polish.job = Some(job);
        app.polish.automatic = Some(std::time::Instant::now());
        tx.send(crate::polish::Event::Complete(preview)).unwrap();
        app.polish_poll();
        let Some(crate::integration::Action::Insert {
            id,
            text,
            learn,
            cancel,
            ..
        }) = app.integration_pending.take()
        else {
            panic!("Expected final insertion");
        };
        assert_eq!(id, app.utterance);
        assert_eq!(text, expected);
        assert!(!learn);
        assert!(!cancel.is_cancelled());
        assert!(app.busy);
    }

    #[test]
    fn automatic_completion_timeout_failure_and_cancellation() {
        for outcome in [
            "complete", "blocked", "failed", "timeout", "cancel", "new", "explicit",
        ] {
            let (mut app, _) = super::super::tests::app();
            prepared_preview(&mut app);
            app.preview_inflight = false;
            app.target = None;
            let original = app.raw.clone();
            let baseline = app.text.clone();
            let mut preview = app.polish.preview.take().unwrap();
            let expected = preview.text.clone();
            let (job, tx) = crate::polish::Job::test_channel();
            app.polish.job = Some(job);
            app.polish.automatic = Some(std::time::Instant::now());
            match outcome {
                "timeout" => {
                    app.polish.automatic =
                        Some(std::time::Instant::now() - std::time::Duration::from_secs(5))
                }
                "cancel" => app.insertion_cancel.cancel(),
                "new" => app.utterance += 1,
                "explicit" => app.desktop_cleanup_cancel(),
                "failed" => tx
                    .send(crate::polish::Event::Failed("Unavailable".into()))
                    .unwrap(),
                _ => {
                    if outcome == "blocked" {
                        preview.blocked = Some("Changed a number".into());
                    }
                    tx.send(crate::polish::Event::Complete(preview)).unwrap();
                }
            }
            app.polish_poll();
            assert_eq!(app.raw, original);
            assert_eq!(
                app.text,
                if outcome == "complete" {
                    expected
                } else {
                    baseline
                }
            );
            assert!(app.polish.automatic.is_none());
            assert!(app.remembered.is_empty());
            assert!(app.integration_pending.is_none());
            if matches!(outcome, "complete" | "blocked" | "failed" | "timeout") {
                let saved = app.history.dictation.as_ref().unwrap();
                assert_eq!(saved.original, original);
                assert_eq!(saved.text, app.text);
                assert_eq!(app.edit_baseline, app.text);
                assert!(app.edited_at.is_none());
            }
        }
    }

    #[test]
    fn preview_contract_excludes_runtime_details() {
        let (mut app, _) = super::super::tests::app();
        prepared_preview(&mut app);
        let status = serde_json::to_value(app.desktop_cleanup_status()).unwrap();
        let preview = status["preview"].as_object().unwrap();
        assert_eq!(preview.len(), 5);
        for key in ["original", "source", "text", "blocked", "cleanup_only"] {
            assert!(preview.contains_key(key));
        }
    }

    #[test]
    fn cancelling_discards_a_completed_preview() {
        let (mut app, _) = super::super::tests::app();
        prepared_preview(&mut app);
        let before = app.text.clone();
        app.desktop_cleanup_cancel();
        app.polish_apply();
        assert_eq!(app.text, before);
        assert!(app.polish.preview.is_none());
    }

    #[test]
    fn completed_job_is_reviewable_and_cancel_wins_a_queued_completion() {
        let (mut app, _) = super::super::tests::app();
        prepared_preview(&mut app);
        app.preview_inflight = false;
        let preview = app.polish.preview.take().unwrap();
        let (job, tx) = crate::polish::Job::test_channel();
        app.polish.job = Some(job);
        tx.send(crate::polish::Event::Complete(preview)).unwrap();
        app.polish_poll();
        assert!(app.desktop_cleanup_status().preview.is_some());
        app.polish_apply();
        assert!(app.text.starts_with("Hi Casey,"));

        prepared_preview(&mut app);
        let original = app.text.clone();
        let preview = app.polish.preview.take().unwrap();
        let (job, tx) = crate::polish::Job::test_channel();
        app.polish.job = Some(job);
        tx.send(crate::polish::Event::Complete(preview)).unwrap();
        app.desktop_cleanup_cancel();
        app.polish_poll();
        app.polish_apply();
        assert_eq!(app.text, original);
        assert!(app.polish.preview.is_none());
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
