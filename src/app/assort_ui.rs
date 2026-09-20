use crate::classification::builtin::{self, Task as ModelTask};
use crate::classification::{self, InstalledPackage, Summary, Transcript};
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

#[derive(Default)]
pub(super) struct State {
    pub configuration: Configuration,
    task: Option<classification::Task>,
    summary: Option<Summary>,
    note_selection: Vec<bool>,
    input_hash: Option<[u8; 32]>,
    error: Option<String>,
    correction_task: Option<classification::CorrectionTask>,
    correction_review: Option<crate::correction_context::Review>,
    correction_baseline: String,
    correction_error: Option<String>,
    correction_done: std::collections::HashSet<usize>,
    correction_open: bool,
}

impl State {
    pub fn configured(configuration: Configuration) -> Self {
        Self {
            configuration,
            ..Default::default()
        }
    }
    pub fn busy(&self) -> bool {
        self.task.is_some() || self.correction_task.is_some()
    }

    fn take_selected_notes(&mut self) -> Option<Summary> {
        let summary = self.summary.as_ref()?;
        if self.note_selection.len() != summary.highlights.len()
            || !self.note_selection.iter().any(|selected| *selected)
        {
            return None;
        }
        let mut summary = self.summary.take()?;
        summary.highlights = summary
            .highlights
            .into_iter()
            .zip(self.note_selection.drain(..))
            .filter_map(|(highlight, selected)| selected.then_some(highlight))
            .collect();
        summary.word_count = summary
            .highlights
            .iter()
            .map(|highlight| highlight.source.text.split_whitespace().count())
            .sum();
        Some(summary)
    }
}

impl super::App {
    pub(super) fn assort_correction_button(&mut self, ui: &mut egui::Ui) {
        if ui.button("Review vocabulary").clicked() {
            self.assort.correction_open = true;
        }
    }

    pub(super) fn assort_settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Assort").strong().size(18.0));
        ui.label(
            "Pretrained models for notes and vocabulary review. Everything runs on this device.",
        );
        builtin_status(ui);
        let previous = self.assort.configuration.clone();
        ui.collapsing("Advanced: import a model", |ui| {
            setup(ui, &mut self.assort);
            ui.separator();
            correction_setup(ui, &mut self.assort);
        });
        if previous != self.assort.configuration {
            self.settings.assort = self.assort.configuration.clone();
            self.save();
        }
        if let Some(error) = &self.assort.error {
            ui.label(RichText::new(error).color(egui::Color32::from_rgb(245, 179, 158)));
        }
    }

    pub(super) fn assort_correction_ui(&mut self, ui: &mut egui::Ui) {
        let finished = self.recording.is_none()
            && !self.busy
            && !self.integration_inflight
            && self.integration_pending.is_none()
            && self.call.is_none();
        if self.text.is_empty() || !finished {
            return;
        }
        if let Some(result) = self
            .assort
            .correction_task
            .as_ref()
            .and_then(|task| task.poll())
        {
            self.assort.correction_task = None;
            match result {
                Ok(review) => self.assort.correction_review = Some(review),
                Err(error) => self.assort.correction_error = Some(error),
            }
        }
        let mut open = self.assort.correction_open;
        let viewport = ui.ctx().content_rect();
        egui::Window::new("Review vocabulary in context")
            .open(&mut open)
            .default_pos(egui::pos2((viewport.width() - 680.0).max(32.0) * 0.5, 80.0))
            .default_width((viewport.width() - 100.0).clamp(280.0, 640.0))
            .max_height((viewport.height() - 120.0).max(180.0))
            .vscroll(true)
            .collapsible(false)
            .show(ui.ctx(), |ui| {
            ui.label("Assort can review saved spellings against the surrounding sentence and app. You choose whether to change the finished preview.");
            let previous = self.assort.configuration.clone();
            ui.collapsing("Advanced model options", |ui| correction_setup(ui, &mut self.assort));
            if !self.assort.configuration.available(ModelTask::Corrections) {
                ui.small("This build does not include the vocabulary model. An imported model is optional under Advanced model options.");
            }
            if previous != self.assort.configuration {
                self.settings.assort = self.assort.configuration.clone(); self.save();
            }
            if self.assort.correction_task.is_some() {
                ui.horizontal(|ui| { ui.spinner(); ui.label("Reviewing vocabulary choices…");
                    if ui.button("Cancel review").clicked() { self.assort.correction_task = None; }
                });
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            } else if ui.add_enabled(!self.assort.busy() && self.assort.configuration.available(ModelTask::Corrections),
                egui::Button::new("Check saved spellings")).clicked() {
                let raw = if self.raw.is_empty() { &self.text } else { &self.raw };
                let result = crate::correction_context::prepare(raw, &self.settings.entries, self.dictation_app.as_deref()).and_then(|request| {
                    classification::start_corrections(self.assort.configuration.package(ModelTask::Corrections)?, request)
                });
            self.assort.correction_review = None;
                self.assort.correction_done.clear();
                self.assort.correction_error = None;
                self.assort.correction_baseline.clone_from(&self.text);
                match result { Ok(task) => self.assort.correction_task = Some(task), Err(error) => self.assort.correction_error = Some(format!("{error:#}")) }
            }
            let mut choice = None;
            if let Some(review) = &self.assort.correction_review {
                for score in &review.scores {
                    if self.assort.correction_done.contains(&score.candidate) { continue; }
                    let candidate = &review.request.candidates[score.candidate];
                    ui.separator();
                    ui.label(RichText::new(format!("{} → {}", candidate.entry.heard, candidate.entry.wanted)).strong());
                    ui.label(if crate::correction_context::literal_context(review,score.candidate) { "Literal wording may be intended. Choose the spelling to keep." }
                        else if score.replace_score >= 0.65 { "Suggested: saved spelling" }
                        else if score.replace_score <= 0.35 { "Suggested: original wording" }
                        else { "No clear suggestion. Choose the wording you intended." });
                    ui.label(crate::correction_context::excerpt(&review.request.original,&candidate.entry.heard,candidate.entry.ignore_case));
                    ui.label(RichText::new(crate::correction_context::excerpt(&candidate.proposed,&candidate.entry.wanted,false)).color(super::ACCENT));
                    if let Some(app)=&review.request.app { ui.small(format!("App: {app}")); }
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Keep original wording").clicked() { choice = Some((score.candidate, false)); }
                        if ui.button("Use saved spelling").clicked() { choice = Some((score.candidate, true)); }
                    });
                }
                ui.small("Review each choice before using it. Changes affect this preview only; your saved vocabulary stays the same.");
            }
            if let Some((index, saved)) = choice
                && let Some(review) = &self.assort.correction_review {
                let raw = if self.raw.is_empty() { &review.request.original } else { &self.raw };
                match crate::correction_context::apply_choice(review, index, saved, crate::correction_context::Preview { raw, text: &self.text, baseline: &self.assort.correction_baseline }, &self.settings.entries, self.dictation_app.as_deref()) {
                    Ok(text) => {
                        self.text = text; self.edit_baseline.clone_from(&self.text); self.edited_at = None;
                        self.assort.correction_baseline.clone_from(&self.text);
                        self.assort.correction_done.insert(index);
                        self.assort.correction_error = None;
                        if self.assort.correction_review.as_ref().is_some_and(|review|self.assort.correction_done.len()==review.scores.len()) {
                            self.assort.correction_review = None;
                        }
                        self.history_save_dictation(); self.status = "Vocabulary choice applied to the preview.".into();
                    }
                    Err(error) => self.assort.correction_error = Some(format!("{error:#}")),
                }
            }
            if let Some(error) = &self.assort.correction_error { ui.label(RichText::new(error).color(egui::Color32::from_rgb(245,179,158))); }
        });
        self.assort.correction_open = open;
    }
}

#[cfg(test)]
impl super::App {
    pub(super) fn prepare_assort_review_capture(&mut self, notes: bool) {
        if notes {
            let input = self.assort_transcript().unwrap();
            self.assort.input_hash =
                Some(Sha256::digest(serde_json::to_vec(&input).unwrap()).into());
            self.assort.summary = Some(Summary {
                transcript_id: input.id,
                title: input.title,
                goal: input.goal,
                source_segments: input.segments.len(),
                word_count: input
                    .segments
                    .iter()
                    .map(|s| s.text.split_whitespace().count())
                    .sum(),
                highlights: input
                    .segments
                    .into_iter()
                    .enumerate()
                    .map(|(i, source)| classification::Highlight {
                        source,
                        importance: 0.9,
                        kind: if i == 1 {
                            classification::Kind::KeyFact
                        } else {
                            classification::Kind::Action
                        },
                    })
                    .collect(),
            });
            self.assort.note_selection = vec![true, false, true];
        } else {
            self.assort.correction_open = true;
            self.text = "Could you ask at Rowan to review the draft?".into();
            self.raw.clone_from(&self.text);
            self.settings.entries =
                vec![crate::dictionary::validate("at Rowan", "@Rowan").unwrap()];
            let request =
                crate::correction_context::prepare(&self.raw, &self.settings.entries, None)
                    .unwrap();
            self.assort.correction_review = Some(crate::correction_context::Review {
                request,
                scores: vec![crate::correction_context::Score {
                    candidate: 0,
                    replace_score: 0.8,
                }],
            });
            self.assort.correction_baseline.clone_from(&self.text);
        }
    }
}

fn correction_setup(ui: &mut egui::Ui, state: &mut State) {
    ui.add_enabled_ui(!state.busy(), |ui| {
        ui.checkbox(&mut state.configuration.use_imported_corrections, "Use an imported vocabulary model");
        if !state.configuration.use_imported_corrections { ui.small("Uses the pretrained model included with Articulate."); return; }
        for (label, text, folder) in [("Correction checkpoint", &mut state.configuration.correction_checkpoint, true), ("Correction tokenizer JSON", &mut state.configuration.correction_tokenizer, false), ("Correction limits JSON", &mut state.configuration.correction_limits, false)] {
            ui.label(label);
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(text).desired_width((ui.available_width()-100.0).max(120.0)));
                if ui.button("Browse…").clicked() {
                    match super::file_picker::open(if folder {"Choose the correction manifest.json"} else {label}, "json") {
                        Ok(Some(path)) => { let path = if folder { path.parent().unwrap_or(&path).to_path_buf() } else { path }; *text = path.to_string_lossy().into_owned(); }
                        Ok(None) => {}, Err(error) => state.correction_error = Some(format!("{error:#}")),
                    }
                }
            });
        }
        ui.checkbox(&mut state.configuration.correction_preview, "Enable local correction review");
        ui.small("Use a correction checkpoint from Articulate's training recipe. Meeting-note models cannot score vocabulary choices.");
    });
}

fn setup(ui: &mut egui::Ui, state: &mut State) {
    ui.add_enabled_ui(!state.busy(), |ui| {
        ui.checkbox(&mut state.configuration.use_imported_notes, "Use an imported notes model");
        if !state.configuration.use_imported_notes { ui.small("Uses the pretrained model included with Articulate."); return; }
        for (label, text, folder) in [("Checkpoint folder", &mut state.configuration.checkpoint,true), ("Tokenizer JSON", &mut state.configuration.tokenizer,false), ("Preprocessing limits JSON", &mut state.configuration.limits,false)] {
            ui.label(label);
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(text).desired_width((ui.available_width()-100.0).max(120.0)));
                if ui.button("Browse…").clicked() {
                    match super::file_picker::open(if folder {"Choose the checkpoint manifest.json"} else {label},"json") {
                        Ok(Some(path))=> {let path=if folder {path.parent().unwrap_or(&path).to_path_buf()} else {path}; *text=path.to_string_lossy().into_owned();}
                        Ok(None)=>{}, Err(error)=>state.error=Some(format!("{error:#}")),
                    }
                }
            });
        }
        ui.checkbox(&mut state.configuration.local_preview,"Enable preview suggestions with this local model");
        ui.small("Assort runs inside Articulate. Choose your trained model files; Articulate verifies the weights and tokenizer before using them.");
    });
}

fn builtin_status(ui: &mut egui::Ui) {
    for (label, task) in [
        ("Meeting notes", ModelTask::Notes),
        ("Vocabulary review", ModelTask::Corrections),
    ] {
        ui.horizontal(|ui| {
            ui.label(label);
            ui.label(
                RichText::new(if builtin::available(task) {
                    "Included"
                } else {
                    "Not included in this build"
                })
                .small()
                .color(super::theme::MUTED),
            );
        });
    }
}

pub(super) fn to_notes(
    summary: &Summary,
    input: &Transcript,
    rows: &[crate::calls::Row],
    names: &[String; 4],
) -> anyhow::Result<crate::notes::Notes> {
    anyhow::ensure!(summary.transcript_id == input.id, "The transcript changed.");
    let mut notes = crate::notes::Notes::build(rows);
    notes.highlights.clear();
    notes.actions.clear();
    for highlight in &summary.highlights {
        let source = &highlight.source;
        let (parent_index, _) = classification::passages::locate(input, source)?;
        let parent = &input.segments[parent_index];
        let index: usize = parent
            .id
            .strip_prefix("row-")
            .ok_or_else(|| anyhow::anyhow!("Unknown source passage."))?
            .parse()?;
        let row = rows
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("The source passage is no longer available."))?;
        anyhow::ensure!(
            row.text == parent.text
                && row.start_ms == source.start_ms
                && row.end_ms == source.end_ms
                && source.speaker.as_deref() == Some(crate::calls::label(row, names).as_str()),
            "The source passage changed."
        );
        let quote = crate::notes::Quote {
            row: index,
            start_ms: row.start_ms,
            end_ms: row.end_ms,
            microphone: row.microphone,
            speakers: row.speakers.clone(),
            discord: row.discord.clone(),
            text: source.text.clone(),
        };
        if highlight.kind == classification::Kind::Action {
            notes.actions.push(quote);
        } else {
            notes.highlights.push(quote);
        }
    }
    Ok(notes)
}

/// Offers suggestions for an immutable transcript snapshot. Accept returns exact
/// source passages; caller maps segment IDs to its own rows before saving notes.
pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut State,
    input: Option<&Transcript>,
    can_start: bool,
) -> Option<Summary> {
    if let Some(result) = state.task.as_ref().and_then(classification::Task::poll) {
        state.task = None;
        match result {
            Ok(summary) => {
                state.note_selection = vec![true; summary.highlights.len()];
                state.summary = Some(summary);
                state.error = None;
            }
            Err(error) => state.error = Some(error),
        }
    }
    let current_hash = input
        .and_then(|input| serde_json::to_vec(input).ok())
        .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)));
    let unchanged = current_hash.is_some() && current_hash == state.input_hash;
    let mut accepted = None;
    egui::Frame::new().fill(super::theme::SURFACE)
        .stroke(egui::Stroke::new(1.0_f32, super::theme::LINE)).corner_radius(18).inner_margin(16.0)
        .show(ui, |ui| {
            if state.summary.is_none() {
                ui.label("Find decisions, actions and key facts. Review the passages before saving them.");
            }
            if !state.configuration.available(ModelTask::Notes) {
                ui.small("This build does not include the notes model. An imported model is optional under Advanced model options.");
            }
            if state.busy() {
                ui.horizontal(|ui| { ui.spinner(); ui.label("Finding useful passages…"); if ui.button("Cancel").clicked() { state.task = None; } });
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            } else {
                let create = ui.horizontal_wrapped(|ui| {
                    let create = ui.add_enabled(can_start && input.is_some() && state.configuration.available(ModelTask::Notes), egui::Button::new(if state.summary.is_some() {"Review again"} else {"Create notes"})).clicked();
                    if state.summary.is_some() {
                        let count=state.note_selection.iter().filter(|selected|**selected).count();
                        if ui.add_enabled(unchanged && count>0 && can_start, egui::Button::new(format!("Save {count} selected {}",if count==1 {"passage"} else {"passages"}))).clicked() { accepted = state.take_selected_notes(); }
                    }
                    create
                }).inner;
                if create && let Some(input) = input {
                    state.error = None;
                    state.summary = None;
                    state.note_selection.clear();
                    state.input_hash = current_hash;
                    match state.configuration.package(ModelTask::Notes).and_then(|package|classification::start(package, input.clone())) {
                        Ok(task) => state.task = Some(task),
                        Err(error) => state.error = Some(format!("{error:#}")),
                    }
                }
            }
            if !can_start { ui.small("Finish the recording before classifying notes."); }
            if let Some(error) = &state.error { ui.label(RichText::new(error).color(egui::Color32::from_rgb(245,179,158))); }
            if let Some(summary) = &state.summary {
                if summary.highlights.is_empty() { ui.label("No suggestions this time. Your transcript is unchanged."); }
                else { ui.small("Choose passages to save. Times refer to the speaker's full turn."); }
                for (index,highlight) in summary.highlights.iter().enumerate() {
                    let kind = match highlight.kind { classification::Kind::Decision => "Decision", classification::Kind::Action => "Action", classification::Kind::KeyFact => "Key fact", classification::Kind::Background => "Background" };
                    let source = &highlight.source;
                    ui.separator();
                    if let Some(selected)=state.note_selection.get_mut(index) {
                        ui.checkbox(selected,RichText::new(format!("{kind} · {:02}:{:02} · {}", source.start_ms/60_000, source.start_ms/1000%60, source.speaker.as_deref().unwrap_or("Uncertain speaker"))).color(super::ACCENT));
                    }
                    ui.label(&source.text);
                }
                if !unchanged { ui.small("The transcript changed. Classify it again before saving these suggestions."); }
                let count=state.note_selection.iter().filter(|selected|**selected).count();
                if count==0 && !summary.highlights.is_empty() { ui.small("Select at least one passage to save."); }
            }
            ui.collapsing("Advanced model options", |ui| setup(ui,state));
        });
    accepted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection_state() -> State {
        let (mut app, _) = super::super::tests::app();
        for (index, text) in ["We agreed to ship Friday.", "Casey will send the report."]
            .iter()
            .enumerate()
        {
            app.call_rows.push(crate::calls::Row {
                start_ms: index as u64 * 4000,
                end_ms: index as u64 * 4000 + 3000,
                microphone: true,
                speakers: vec![],
                discord: None,
                text: (*text).into(),
            });
        }
        let input = app.assort_transcript().unwrap();
        State {
            summary: Some(Summary {
                transcript_id: input.id,
                title: input.title,
                goal: input.goal,
                word_count: 10,
                source_segments: 2,
                highlights: input
                    .segments
                    .into_iter()
                    .map(|source| classification::Highlight {
                        kind: classification::Kind::Action,
                        importance: 0.9,
                        source,
                    })
                    .collect(),
            }),
            note_selection: vec![true, true],
            ..Default::default()
        }
    }

    #[test]
    fn save_selected_notes_preserves_exact_source_and_counts_selected_words() {
        let mut state = selection_state();
        let original = state.summary.as_ref().unwrap();
        let source = serde_json::to_value(&original.highlights[1].source).unwrap();
        let id = original.transcript_id.clone();
        state.note_selection[0] = false;
        let selected = state.take_selected_notes().unwrap();
        assert_eq!(selected.transcript_id, id);
        assert_eq!(selected.highlights.len(), 1);
        assert_eq!(
            serde_json::to_value(&selected.highlights[0].source).unwrap(),
            source
        );
        assert_eq!(selected.word_count, 5);
        assert_eq!(selected.source_segments, 2);
        assert!(state.summary.is_none());
    }

    #[test]
    fn empty_or_mismatched_selection_does_not_consume_review() {
        let mut state = selection_state();
        state.note_selection.fill(false);
        assert!(state.take_selected_notes().is_none());
        assert_eq!(state.summary.as_ref().unwrap().highlights.len(), 2);
        state.note_selection = vec![true];
        assert!(state.take_selected_notes().is_none());
        assert!(state.summary.is_some());
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
    #[test]
    fn reviewed_notes_preserve_row_identity_and_freshness() {
        let (mut app, _) = super::super::tests::app();
        app.call_rows.push(crate::calls::Row {
            start_ms: 1000,
            end_ms: 3000,
            microphone: true,
            speakers: vec![],
            discord: None,
            text: "I will send the report.".into(),
        });
        let input = app.assort_transcript().unwrap();
        let summary = Summary {
            transcript_id: input.id.clone(),
            title: input.title.clone(),
            goal: input.goal.clone(),
            word_count: 6,
            source_segments: 1,
            highlights: vec![classification::Highlight {
                kind: classification::Kind::Action,
                importance: 0.9,
                source: input.segments[0].clone(),
            }],
        };
        let notes = to_notes(&summary, &input, &app.call_rows, &app.speaker_names).unwrap();
        assert!(notes.is_current(&app.call_rows));
        assert_eq!(notes.actions.len(), 1);
        assert_eq!(notes.actions[0].text, app.call_rows[0].text);
        assert!(notes.highlights.is_empty());
        app.call_rows[0].text = "Changed transcript".into();
        assert!(to_notes(&summary, &input, &app.call_rows, &app.speaker_names).is_err());
    }
    #[test]
    fn reviewed_excerpts_save_only_the_quote_with_the_full_turn_time_range() {
        let (mut app, _) = super::super::tests::app();
        app.call_rows.push(crate::calls::Row {
            start_ms: 1000,
            end_ms: 30_000,
            microphone: true,
            speakers: vec![],
            discord: None,
            text: "Welcome everyone. I will send the report.".into(),
        });
        let input = app.assort_transcript().unwrap();
        let mut source = input.segments[0].clone();
        source.id = "excerpt:0:18:41".into();
        source.text = "I will send the report.".into();
        let summary = Summary {
            transcript_id: input.id.clone(),
            title: input.title.clone(),
            goal: input.goal.clone(),
            word_count: 5,
            source_segments: 1,
            highlights: vec![classification::Highlight {
                kind: classification::Kind::Action,
                importance: 0.9,
                source,
            }],
        };
        let notes = to_notes(&summary, &input, &app.call_rows, &app.speaker_names).unwrap();
        assert_eq!(notes.actions[0].text, "I will send the report.");
        assert_eq!(
            (notes.actions[0].start_ms, notes.actions[0].end_ms),
            (1000, 30_000)
        );
        app.call_rows[0].text = "Changed first sentence. I will send the report.".into();
        assert!(to_notes(&summary, &input, &app.call_rows, &app.speaker_names).is_err());
    }
    #[test]
    fn rendering_preview_does_not_launch_or_apply_a_model() {
        let (mut app, _) = super::super::tests::app();
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.assort_notes_ui(ui));
        });
        assert!(!app.assort.busy());
        assert!(!app.settings.assort.local_preview);
        assert!(app.call_notes.is_none());
    }
}
