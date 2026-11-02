use crate::classification::InstalledPackage;
use crate::classification::builtin::{self, Task as ModelTask};
use serde::{Deserialize, Serialize};

#[derive(Default)]
pub(super) struct ReviewState {
    task: Option<crate::classification::CorrectionTask>,
    review: Option<crate::correction_context::Review>,
    utterance: u64,
    raw: String,
    baseline: String,
    app: Option<String>,
    status: String,
}

impl ReviewState {
    pub(super) fn working(&self) -> bool {
        self.task.is_some()
    }
}

#[allow(dead_code)]
impl super::App {
    pub(super) fn desktop_assort_start(&mut self, app: Option<String>) -> Result<(), String> {
        if self.brain_working()
            || self.preview_inflight
            || self.integration_inflight
            || self.integration_pending.is_some()
            || self.macro_used.is_some()
            || self.recording.is_some()
            || self.call.is_some()
            || self.busy
            || self.polish_working()
        {
            return Err("Finish recording and editing before reviewing vocabulary.".into());
        }
        if self.polish.assort.task.is_some() {
            return Err("A vocabulary review is already running.".into());
        }
        let request =
            crate::correction_context::prepare(&self.raw, &self.settings.entries, app.as_deref())
                .map_err(|e| e.to_string())?;
        let package = self
            .settings
            .assort
            .package(ModelTask::Corrections)
            .map_err(|e| e.to_string())?;
        let task = crate::classification::start_corrections(package, request)
            .map_err(|e| e.to_string())?;
        self.polish.assort = ReviewState {
            task: Some(task),
            utterance: self.utterance,
            raw: self.raw.clone(),
            baseline: self.text.clone(),
            app,
            status: "Reviewing vocabulary".into(),
            review: None,
        };
        Ok(())
    }

    pub(super) fn desktop_assort_cancel(&mut self) {
        self.polish.assort = ReviewState::default();
    }

    pub(super) fn desktop_assort_status(&mut self) -> serde_json::Value {
        let state = &mut self.polish.assort;
        if state.utterance != self.utterance
            || state.raw != self.raw
            || state.baseline != self.text
            || self.recording.is_some()
            || self.call.is_some()
        {
            *state = ReviewState::default();
        }
        if let Some(result) = state.task.as_ref().and_then(|task| task.poll()) {
            state.task = None;
            match result {
                Ok(review) => {
                    state.review = Some(review);
                    state.status = "Review suggestions before applying".into();
                }
                Err(error) => state.status = error,
            }
        }
        serde_json::json!({"working":state.task.is_some(), "status":state.status,
            "candidates":state.review.as_ref().map(|r| &r.request.candidates),
            "scores":state.review.as_ref().map(|r| &r.scores)})
    }

    pub(super) fn desktop_assort_apply(
        &mut self,
        index: usize,
        saved_spelling: bool,
    ) -> Result<(), String> {
        self.desktop_assort_status();
        if self.brain_working()
            || self.preview_inflight
            || self.integration_inflight
            || self.integration_pending.is_some()
            || self.macro_used.is_some()
            || self.busy
            || self.polish_working()
        {
            return Err("Finish the current operation first.".into());
        }
        let state = &self.polish.assort;
        let review = state.review.as_ref().ok_or("Review this passage first.")?;
        let text = crate::correction_context::apply_choice(
            review,
            index,
            saved_spelling,
            crate::correction_context::Preview {
                raw: &self.raw,
                text: &self.text,
                baseline: &state.baseline,
            },
            &self.settings.entries,
            state.app.as_deref(),
        )
        .map_err(|e| e.to_string())?;
        self.text = text;
        self.edit_baseline.clone_from(&self.text);
        self.edited_at = None;
        self.history_dictation_changed();
        self.desktop_assort_cancel();
        Ok(())
    }
}

#[derive(Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Configuration {
    pub checkpoint: String,
    pub tokenizer: String,
    pub limits: String,
    pub local_preview: bool,
    pub use_imported_notes: bool,
    pub correction_checkpoint: String,
    pub correction_tokenizer: String,
    pub correction_limits: String,
    pub correction_preview: bool,
    pub use_imported_corrections: bool,
}

#[allow(
    dead_code,
    reason = "Assort model configuration retained for the renderer integration"
)]
impl Configuration {
    fn available(&self, task: ModelTask) -> bool {
        match task {
            ModelTask::Notes if self.use_imported_notes => self.local_preview,
            ModelTask::Corrections if self.use_imported_corrections => self.correction_preview,
            _ => builtin::available(task),
        }
    }

    fn package(&self, task: ModelTask) -> anyhow::Result<InstalledPackage> {
        let imported = match task {
            ModelTask::Notes => self.use_imported_notes,
            ModelTask::Corrections => self.use_imported_corrections,
        };
        if !imported {
            return builtin::package(task);
        }
        let (checkpoint, tokenizer, limits, enabled) = match task {
            ModelTask::Notes => (
                &self.checkpoint,
                &self.tokenizer,
                &self.limits,
                self.local_preview,
            ),
            ModelTask::Corrections => (
                &self.correction_checkpoint,
                &self.correction_tokenizer,
                &self.correction_limits,
                self.correction_preview,
            ),
        };
        anyhow::ensure!(enabled, "Enable suggestions for the imported model first");
        Ok(InstalledPackage {
            profile: "local-preview".into(),
            local_preview: true,
            checkpoint: checkpoint.trim().into(),
            tokenizer: tokenizer.trim().into(),
            limits: limits.trim().into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_review_applies_only_to_unchanged_text() {
        let (mut app, _) = super::super::tests::app();
        app.preview_inflight = false;
        let entry = crate::dictionary::validate("cube win", "Qwen").unwrap();
        app.settings.entries.push(entry);
        app.raw = "Load cube win".into();
        app.text = app.raw.clone();
        let request =
            crate::correction_context::prepare(&app.raw, &app.settings.entries, None).unwrap();
        app.polish.assort = ReviewState {
            review: Some(crate::correction_context::Review {
                request,
                scores: vec![],
            }),
            utterance: app.utterance,
            raw: app.raw.clone(),
            baseline: app.text.clone(),
            ..Default::default()
        };
        app.desktop_assort_apply(0, true).unwrap();
        assert_eq!(app.text, "Load Qwen");
        assert_eq!(app.raw, "Load cube win");
        assert!(app.edited_at.is_none());
        assert!(app.desktop_assort_apply(0, true).is_err());
    }

    #[test]
    fn cancel_discards_review_without_changing_text() {
        let (mut app, _) = super::super::tests::app();
        app.text = "Keep this passage".into();
        app.desktop_assort_cancel();
        assert_eq!(app.text, "Keep this passage");
        assert!(app.desktop_assort_apply(0, true).is_err());
    }

    #[test]
    fn normal_configuration_selects_bundled_tasks_without_manual_paths() {
        let config = Configuration::default();
        assert!(!config.use_imported_notes && !config.use_imported_corrections);
        assert!(config.checkpoint.is_empty() && config.correction_checkpoint.is_empty());
        assert_eq!(
            config.available(ModelTask::Notes),
            builtin::available(ModelTask::Notes)
        );
        assert_eq!(
            config.available(ModelTask::Corrections),
            builtin::available(ModelTask::Corrections)
        );
        let imported = Configuration {
            use_imported_notes: true,
            ..Default::default()
        };
        assert!(!imported.available(ModelTask::Notes));
        assert!(imported.package(ModelTask::Notes).is_err());
    }
}
