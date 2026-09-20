use super::{App, theme};
use crate::{
    brain,
    classification::{Segment, Transcript},
    history::Session,
    polish::{self, ModelProfile},
};
use eframe::egui::{self, RichText};

#[derive(Default)]
pub(super) struct State {
    job: Option<brain::Job>,
    download: Option<polish::Job>,
    source_id: Option<String>,
    draft: Option<brain::Draft>,
    pending_save: Option<String>,
    status: String,
    progress: Option<f32>,
}

impl App {
    pub(super) fn brain_saved(&mut self, id: &str) {
        if self.brain.pending_save.as_deref() == Some(id)
            && self
                .history
                .worker
                .as_ref()
                .is_some_and(|worker| worker.saves_settled() == Ok(true))
            && self.brain.draft.is_none()
            && self.brain.job.is_none()
            && self.brain.download.is_none()
        {
            self.brain.pending_save = None;
            self.brain.status = "Summary saved on this device.".into();
        }
    }
    pub(super) fn brain_working(&self) -> bool {
        self.brain.job.is_some()
    }
    pub(super) fn brain_poll(&mut self) {
        if self.brain.job.is_some()
            && (self.recording.is_some() || self.call.is_some() || self.loading)
        {
            self.brain.job = None;
            self.brain.status = "Summary cancelled to keep speech recognition responsive. Generate it again when recording and model setup have finished.".into();
        }
        loop {
            let event = self.brain.job.as_ref().map(brain::Job::try_recv);
            match event {
                Some(Ok(brain::Event::Progress { completed, total })) => {
                    self.brain.status =
                        format!("Summarizing on this device · {completed} of {total} sections");
                    self.brain.progress = Some(completed as f32 / total.max(1) as f32);
                }
                Some(Ok(event)) => {
                    self.brain.job = None;
                    self.brain.progress = None;
                    match event {
                        brain::Event::Complete(draft) => {
                            self.brain.draft = Some(draft);
                            self.brain.status =
                                "Review the summary and its sources before saving.".into();
                        }
                        brain::Event::Failed(error) => self.brain.status = error,
                        brain::Event::Cancelled => {
                            self.brain.status =
                                "Summary cancelled. Your notes are unchanged.".into()
                        }
                        brain::Event::Progress { .. } => unreachable!(),
                    }
                    break;
                }
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                    self.brain.job = None;
                    self.brain.progress = None;
                    self.brain.status = "The summary worker stopped. Try again.".into();
                    break;
                }
                _ => break,
            }
        }
        loop {
            let event = self.brain.download.as_ref().map(polish::Job::try_recv);
            match event {
                Some(Ok(polish::Event::Progress { stage, fraction })) => {
                    self.brain.status = stage;
                    self.brain.progress = fraction;
                }
                Some(Ok(event)) => {
                    self.brain.download = None;
                    self.brain.progress = None;
                    self.brain.status = match event {
                        polish::Event::Installed => "Summary model ready on this device.".into(),
                        polish::Event::Failed(error) => error,
                        polish::Event::Cancelled => {
                            "Download paused. Choose Download to resume.".into()
                        }
                        _ => "Summary model setup finished.".into(),
                    };
                    break;
                }
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                    self.brain.download = None;
                    self.brain.progress = None;
                    self.brain.status = "Download stopped. Choose Download to resume.".into();
                    break;
                }
                _ => break,
            }
        }
    }

    fn brain_model_ui(&mut self, ui: &mut egui::Ui) {
        if self.brain.download.is_some() {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label(&self.brain.status);
                if ui.button("Pause download").clicked() {
                    self.brain.download = None;
                    self.brain.progress = None;
                    self.brain.status = "Download paused. Choose Download to resume.".into();
                }
            });
            if let Some(progress) = self.brain.progress {
                ui.add(egui::ProgressBar::new(progress).show_percentage());
            }
        } else if !polish::profile_installed(ModelProfile::Summary) {
            ui.label("A larger local model prepares summaries, decisions and action items. Your recordings stay on this computer.");
            if ui
                .button(format!(
                    "Download summary model · {:.2} GB",
                    polish::SUMMARY_DOWNLOAD_BYTES as f64 / 1e9
                ))
                .clicked()
            {
                self.brain.download = Some(polish::download_profile(ModelProfile::Summary));
                self.brain.status = "Starting summary model download…".into();
            }
        } else {
            ui.label("Qwen3.5 4B · ready on this device");
        }
    }

    pub(super) fn brain_settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Call and dictation summaries")
                .strong()
                .size(18.0),
        );
        self.brain_model_ui(ui);
        ui.collapsing("Download options", |ui| {
            if ui
                .add_enabled(
                    self.brain.download.is_none() && self.brain.job.is_none(),
                    egui::Button::new("Repair summary model download"),
                )
                .clicked()
            {
                self.brain.download = Some(polish::download_profile(ModelProfile::Summary));
            }
        });
        if self.brain.download.is_none() && !self.brain.status.is_empty() {
            ui.label(&self.brain.status);
        }
    }

    /// Returns true only when the user saves a validated generated draft.
    pub(super) fn brain_summary_ui(&mut self, ui: &mut egui::Ui, session: &mut Session) -> bool {
        let transcript = source(session);
        let matches = self.brain.source_id.as_deref() == Some(session.id.as_str());
        let idle = self.call.is_none()
            && self.recording.is_none()
            && !self.busy
            && !self.loading
            && !self.polish_working();
        let mut saved = false;
        let mut accepted = None;
        self.brain_model_ui(ui);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    idle && self.brain.job.is_none()
                        && self.brain.download.is_none()
                        && polish::profile_installed(ModelProfile::Summary)
                        && !transcript.segments.is_empty(),
                    egui::Button::new("Generate summary"),
                )
                .clicked()
            {
                self.release_polish_runtime();
                match brain::start(transcript.clone()) {
                    Ok(job) => {
                        self.brain.job = Some(job);
                        self.brain.source_id = Some(session.id.clone());
                        self.brain.draft = None;
                        self.brain.status = "Preparing the local summary…".into();
                    }
                    Err(error) => self.brain.status = error.to_string(),
                }
            }
            if self.brain.job.is_some() {
                ui.spinner();
                if ui.button("Cancel summary").clicked() {
                    self.brain.job = None;
                    self.brain.progress = None;
                    self.brain.status = "Summary cancelled.".into();
                }
            }
        });
        if !idle {
            ui.label("Finish recording or the current text cleanup before generating a summary.");
        }
        if matches || self.brain.download.is_none() {
            ui.label(&self.brain.status);
        }
        let pending = self
            .brain
            .draft
            .as_ref()
            .filter(|draft| draft.source_id == session.id);
        let draft = pending.or(session.generated_summary.as_ref());
        if let Some(draft) = draft {
            let validation = draft.validate(&transcript);
            if let Err(error) = &validation {
                ui.label(RichText::new(error.to_string()).color(egui::Color32::LIGHT_YELLOW));
            }
            ui.label(
                RichText::new(if pending.is_some() {
                    "Generated draft · review wording, labels and sources"
                } else {
                    "Saved summary · generated locally and reviewed"
                })
                .color(theme::MUTED),
            );
            if draft.sections > 1 {
                ui.label("This is a section-by-section summary. Later sections may update an earlier decision or plan.");
            }
            ui.horizontal_wrapped(|ui| {
                if pending.is_some()
                    && ui
                        .add_enabled(
                            validation.is_ok(),
                            egui::Button::new("Save reviewed summary"),
                        )
                        .clicked()
                {
                    accepted = Some(draft.clone());
                }
                if ui.button("Copy summary").clicked() {
                    ui.ctx().copy_text(summary_text(draft));
                }
            });
            ui.separator();
            if draft.items.is_empty() {
                ui.label(
                    "No supported summary points were returned. Your transcript is unchanged.",
                );
            }
            egui::ScrollArea::vertical()
                .id_salt(("generated_summary", &session.id))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (index, item) in draft.items.iter().enumerate() {
                        ui.push_id(index, |ui| {
                            egui::Frame::new()
                                .fill(theme::SURFACE)
                                .corner_radius(12)
                                .inner_margin(14)
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(format!(
                                            "{} · section {}",
                                            kind(item.kind),
                                            item.section
                                        ))
                                        .color(super::ACCENT)
                                        .small(),
                                    );
                                    ui.add(
                                        egui::Label::new(RichText::new(&item.text).size(18.0))
                                            .wrap()
                                            .selectable(true),
                                    );
                                    ui.collapsing(
                                        format!("Sources · {}", item.sources.len()),
                                        |ui| {
                                            for citation in &item.sources {
                                                ui.label(
                                                    RichText::new(format!(
                                                        "{} · {}",
                                                        citation
                                                            .speaker
                                                            .as_deref()
                                                            .unwrap_or("Transcript"),
                                                        time(citation.start_ms)
                                                    ))
                                                    .color(theme::MUTED),
                                                );
                                                ui.add(
                                                    egui::Label::new(&citation.excerpt)
                                                        .wrap()
                                                        .selectable(true),
                                                );
                                            }
                                        },
                                    );
                                });
                            ui.add_space(10.0);
                        });
                    }
                });
        } else if self.brain.job.is_none() {
            ui.add_space(18.0);
            ui.label("Turn this transcript into a concise record of facts, decisions and follow-ups. Every point includes the passage it came from.");
        }
        if let Some(draft) = accepted {
            session.generated_summary = Some(draft);
            saved = true;
        }
        if saved {
            self.brain.pending_save = Some(session.id.clone());
            self.brain.draft = None;
            self.brain.status = "Saving your reviewed summary…".into();
        }
        saved
    }
}

pub(super) fn source(session: &Session) -> Transcript {
    let segments = if !session.rows.is_empty() {
        session
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| !row.text.trim().is_empty())
            .map(|(index, row)| Segment {
                id: format!("row-{index}"),
                start_ms: row.start_ms,
                end_ms: row.end_ms,
                speaker: Some(crate::calls::label(row, &session.speaker_names)),
                text: row.text.clone(),
            })
            .collect()
    } else if !session.text.trim().is_empty() {
        vec![Segment {
            id: "dictation".into(),
            start_ms: 0,
            end_ms: session.metrics.audio_duration_ms.unwrap_or(0),
            speaker: None,
            text: session.text.clone(),
        }]
    } else {
        Vec::new()
    };
    Transcript {
        id: session.id.clone(),
        title: session.title.clone(),
        goal: "Summarize facts, decisions and explicit actions with sources.".into(),
        segments,
    }
}
fn kind(kind: brain::Kind) -> &'static str {
    match kind {
        brain::Kind::Fact => "Key point",
        brain::Kind::Decision => "Decision",
        brain::Kind::Action => "Action",
    }
}
fn time(ms: u64) -> String {
    format!("{:02}:{:02}", ms / 60_000, (ms / 1000) % 60)
}
pub(super) fn summary_text(draft: &brain::Draft) -> String {
    let mut text = String::new();
    for item in &draft.items {
        text.push_str(&format!("{}: {}\n", kind(item.kind), item.text));
        for citation in &item.sources {
            text.push_str(&format!(
                "  [{}] {}: {}\n",
                time(citation.start_ms),
                citation.speaker.as_deref().unwrap_or("Transcript"),
                citation.excerpt
            ));
        }
        text.push('\n');
    }
    text
}

#[cfg(test)]
impl App {
    pub(super) fn prepare_summary_capture(&mut self, session: &mut Session) {
        let transcript = source(session);
        let part = &transcript.segments[2];
        self.brain.source_id = Some(session.id.clone());
        self.brain.draft = Some(brain::Draft {
            schema: 1,
            source_id: session.id.clone(),
            source_hash: brain::source_hash_for_test(&transcript),
            model: brain::MODEL_LABEL.into(),
            sections: 1,
            elapsed_ms: 4200,
            items: vec![brain::Item {
                kind: brain::Kind::Action,
                text: "Review the final details tomorrow and send an update.".into(),
                section: 1,
                sources: vec![brain::Citation {
                    source_id: part.id.clone(),
                    start_byte: 0,
                    end_byte: part.text.len(),
                    start_ms: part.start_ms,
                    end_ms: part.end_ms,
                    speaker: part.speaker.clone(),
                    excerpt: part.text.clone(),
                }],
            }],
        });
    }
}
