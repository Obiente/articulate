use super::App;
use crate::polish::{self, Event, Job, Preview, Request, Style, Worker};
use eframe::egui::{self, RichText};

#[derive(Default)]
pub(super) struct State {
    worker: Worker,
    job: Option<Job>,
    context: Option<Context>,
    preview: Option<Preview>,
    open: bool,
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
    fn polish_idle(&self) -> bool {
        self.recording.is_none()
            && self.call.is_none()
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
                            self.polish.status = if let Some(reason) = &preview.blocked {
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
            .filter(|preview| preview.blocked.is_none())
        {
            self.text = preview.text;
            self.edit_baseline.clone_from(&self.text);
            self.edited_at = None;
            self.history_dictation_changed();
            self.polish.context = None;
            self.polish.open = false;
            self.status = "Polished preview saved. Copy it when ready.".into();
        }
    }

    pub(super) fn polish_button(&mut self, ui: &mut egui::Ui) {
        if ui.button("Polish").clicked() {
            self.polish.open = true;
        }
    }

    fn polish_controls(&mut self, ui: &mut egui::Ui) {
        if self.polish.job.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(&self.polish.status);
                if ui.button("Cancel").clicked() {
                    self.polish.job = None;
                    self.polish.status = "Cancelled. Your text is unchanged.".into();
                }
            });
            if let Some(progress) = self.polish.fraction {
                ui.add(egui::ProgressBar::new(progress).show_percentage());
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        } else if !polish::installed() {
            ui.label("Download the local editor once. Your text stays on this computer.");
            if ui
                .button(format!(
                    "Download editor · {:.0} MB",
                    polish::MODEL_BYTES as f64 / 1_000_000.0 + 19.0
                ))
                .clicked()
            {
                self.polish.context = None;
                self.polish.job = Some(polish::download());
                self.polish.status = "Starting download…".into();
            }
        } else {
            ui.label(format!("{} · ready on this device", polish::MODEL_NAME));
        }
    }

    fn polish_style(&mut self, ui: &mut egui::Ui) {
        let previous = self.settings.polish_style;
        ui.horizontal_wrapped(|ui| {
            ui.label("Writing style");
            for (style, label) in [
                (Style::Clear, "Clear"),
                (Style::Professional, "Professional"),
                (Style::Casual, "Casual"),
            ] {
                ui.selectable_value(&mut self.settings.polish_style, style, label);
            }
        });
        if previous != self.settings.polish_style {
            self.save();
        }
    }

    pub(super) fn polish_settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Advanced cleanup").strong().size(18.0));
        ui.label("Polish a finished dictation for punctuation and readability. Review every change before applying it.");
        self.polish_controls(ui);
        self.polish_style(ui);
        ui.collapsing("Download options", |ui| {
            if ui
                .add_enabled(
                    self.polish.job.is_none(),
                    egui::Button::new("Repair editor download"),
                )
                .clicked()
            {
                self.polish.worker = Worker::default();
                self.polish.context = None;
                self.polish.preview = None;
                self.polish.job = Some(polish::download());
                self.polish.status = "Checking the local editor…".into();
            }
        });
        if self.polish.job.is_none() && !self.polish.status.is_empty() {
            ui.label(&self.polish.status);
        }
    }

    pub(super) fn polish_ui(&mut self, ui: &mut egui::Ui) {
        let mut open = self.polish.open;
        let viewport = ui.ctx().content_rect();
        egui::Window::new("Polish your words").open(&mut open).collapsible(false)
            .default_pos(egui::pos2((viewport.width() - 720.0).max(32.0) * 0.5, 30.0))
            .default_width((viewport.width() - 100.0).clamp(280.0, 700.0))
            .max_height((viewport.height() - 90.0).max(240.0))
            .show(ui.ctx(), |ui| {
                ui.label("A local editing preview. Applying it updates this transcript; copy it to use elsewhere.");
                ui.small("Best for short passages: up to 200 words. Longer or non-Latin passages may reach the size limit sooner.");
                if self.polish.preview.is_none() || self.polish.job.is_some() { self.polish_controls(ui); }
                self.polish_style(ui);
                let can_start = self.polish_idle() && self.macro_used.is_none()
                    && !self.text.is_empty() && self.polish.job.is_none() && polish::installed();
                ui.horizontal(|ui| {
                    if ui.add_enabled(can_start, egui::Button::new("Polish text")).clicked() { self.polish_start(); }
                    if self.macro_used.is_some() { ui.label("Expanded shortcuts keep their exact wording."); }
                });
                if self.polish.job.is_none() { ui.label(&self.polish.status); }
                ui.separator();
                egui::ScrollArea::vertical().id_salt("polish_comparison")
                    .max_height((viewport.bottom() - ui.next_widget_position().y - 90.0).clamp(80.0, 500.0))
                    .min_scrolled_height(80.0).show(ui, |ui| {
                    ui.label(RichText::new("Before").strong());
                    ui.add(egui::Label::new(self.polish.preview.as_ref().map_or(self.text.as_str(), |preview| preview.source.as_str())).wrap().selectable(true));
                    if let Some(preview) = &self.polish.preview {
                        ui.add_space(16.0);
                        ui.label(RichText::new("After").strong());
                        ui.add(egui::Label::new(&preview.text).wrap().selectable(true));
                        ui.small(format!("Prepared in {:.1} s", preview.elapsed_ms as f64 / 1000.0));
                        ui.collapsing("Original transcription", |ui| { ui.add(egui::Label::new(&preview.original).wrap().selectable(true)); });
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    let apply = self.polish.job.is_none() && self.polish.preview.as_ref()
                        .is_some_and(|preview| preview.blocked.is_none() && preview.text != preview.source);
                    if ui.add_enabled(apply, egui::Button::new(RichText::new("Apply changes").color(egui::Color32::from_rgb(13, 34, 30))).fill(super::ACCENT)).clicked() { self.polish_apply(); }
                    if ui.button("Keep current text").clicked() {
                        self.polish.job = None; self.polish.preview = None;
                        self.polish.context = None; self.polish.open = false;
                    }
                });
            });
        self.polish.open &= open;
    }
}

#[cfg(test)]
impl App {
    pub(super) fn prepare_polish_capture(&mut self) {
        self.polish.open = true;
        self.raw = "um hi Casey can you review the plan before tomorrow I moved the check in to Thursday at three".into();
        self.text = "Hi Casey can you review the plan before tomorrow I moved the check in to Thursday at three".into();
        self.polish.context = Some(Context {
            utterance: self.utterance,
            original: self.raw.clone(),
            source: self.text.clone(),
        });
        self.polish.preview = Some(Preview { original: self.raw.clone(), source: self.text.clone(), text: "Hi Casey, can you review the plan before tomorrow? I moved the check in to Thursday at three.".into(), blocked: None, elapsed_ms: 950 });
        self.polish.status = "Review the changes before using them.".into();
    }
}

#[cfg(test)]
mod tests {
    use super::Context;
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
        app.prepare_polish_capture();
        app.preview_inflight = false;
        let original = app.raw.clone();
        app.edited_at = Some(std::time::Instant::now());
        app.polish_apply();
        assert!(app.text.starts_with("Hi Casey,"));
        assert_eq!(app.raw, original);
        assert_eq!(app.edit_baseline, app.text);
        assert!(app.edited_at.is_none());
        assert!(app.remembered.is_empty());
        assert!(!app.polish.open);
    }

    #[test]
    fn apply_rechecks_text_even_before_event_polling() {
        let (mut app, _) = super::super::tests::app();
        app.prepare_polish_capture();
        app.preview_inflight = false;
        app.text = "A newer draft.".into();
        app.polish_apply();
        assert_eq!(app.text, "A newer draft.");
        assert!(app.polish.preview.is_none());
    }
}
