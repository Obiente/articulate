use super::*;
use crate::history::{Event as HistoryEvent, Kind, Session, Summary, Worker};
use theme::{INK, MUTED};

#[derive(Default)]
pub(super) struct State {
    pub worker: Option<Worker>,
    pub items: Vec<Summary>,
    pub selected: Option<Session>,
    pub selected_dirty: Option<Instant>,
    pub open_requested: Option<String>,
    pub notetaker_search: String,
    pub notetaker_filter: usize,
    pub notetaker_retry: bool,
    pub notetaker_tab: usize,
    pub notetaker_assort: assort_ui::State,
    pub dictation: Option<Session>,
    pub call: Option<Session>,
    pub dictation_dirty: Option<Instant>,
    pub call_dirty: Option<Instant>,
    pub dictation_deleted: bool,
    pub call_deleted: bool,
    pub search: String,
    pub kind_filter: usize,
    pub detail_search: String,
    pub error: Option<String>,
    pub notice: String,
    pub rename: String,
    pub renaming: bool,
    pub confirm_delete: bool,
    pub detail_tab: usize,
    pub loading: bool,
}

impl State {
    pub fn start() -> Self {
        Self {
            worker: Some(Worker::start()),
            loading: true,
            ..Default::default()
        }
    }

    fn deleted(&mut self, id: &str) {
        self.items.retain(|item| item.id != id);
        if self
            .selected
            .as_ref()
            .is_some_and(|session| session.id == id)
        {
            self.selected = None;
        }
        if self
            .dictation
            .as_ref()
            .is_some_and(|session| session.id == id)
        {
            self.dictation = None;
            self.dictation_deleted = true;
        }
        if self.call.as_ref().is_some_and(|session| session.id == id) {
            self.call = None;
            self.call_deleted = true;
        }
        self.confirm_delete = false;
        self.notice = "Deleted from this device".into();
    }
}

impl App {
    pub(super) fn history_dictation_changed(&mut self) {
        self.history
            .dictation_dirty
            .get_or_insert_with(Instant::now);
    }

    pub(super) fn history_call_changed(&mut self) {
        self.history.call_dirty.get_or_insert_with(Instant::now);
    }

    pub(super) fn history_save_dictation(&mut self) {
        self.history.dictation_dirty = None;
        if self.history.dictation_deleted {
            return;
        }
        if self.text.trim().is_empty() && self.history.dictation.is_none() {
            return;
        }
        let session = self
            .history
            .dictation
            .get_or_insert_with(|| Session::new(Kind::Dictation));
        if session.title.is_empty() {
            session.title = title(&self.text, "Dictation");
        }
        session.text.clone_from(&self.text);
        session.original.clone_from(&self.raw);
        if let Some(worker) = &self.history.worker {
            worker.save(session.clone());
        }
    }

    pub(super) fn history_save_call(&mut self) {
        self.history.call_dirty = None;
        if self.history.call_deleted {
            return;
        }
        if self.call_rows.is_empty() && self.history.call.is_none() {
            return;
        }
        let session = self
            .history
            .call
            .get_or_insert_with(|| Session::new(Kind::Call));
        if session.title.is_empty() {
            session.title = title(
                self.call_rows.first().map_or("", |row| row.text.as_str()),
                "Conversation",
            );
        }
        session.rows.clone_from(&self.call_rows);
        session.speaker_names.clone_from(&self.speaker_names);
        session.notes.clone_from(&self.call_notes);
        session.text = calls::text(&self.call_rows, &self.speaker_names);
        if let Some(worker) = &self.history.worker {
            worker.save(session.clone());
        }
    }

    pub(super) fn history_poll(&mut self) {
        let events = self
            .history
            .worker
            .as_ref()
            .map(Worker::drain)
            .unwrap_or_default();
        for event in events {
            match event {
                HistoryEvent::Listed(items) => {
                    self.history.items = items;
                    self.history.loading = false;
                }
                HistoryEvent::Loaded(mut session) => {
                    if self.history.open_requested.as_deref() != Some(session.id.as_str()) {
                        continue;
                    }
                    self.history.open_requested = None;
                    if let Some(current) = &self.history.call
                        && current.id == session.id
                    {
                        *session = current.clone();
                    }
                    self.history.notetaker_tab = 0;
                    self.history.notetaker_assort =
                        assort_ui::State::configured(self.settings.assort.clone());
                    self.history.rename.clone_from(&session.title);
                    self.history.selected = Some(*session);
                    self.history.renaming = false;
                    self.history.confirm_delete = false;
                    self.history.detail_tab = 0;
                    self.history.loading = false;
                }
                HistoryEvent::Saved { id, updated_ms } => {
                    for session in [
                        &mut self.history.dictation,
                        &mut self.history.call,
                        &mut self.history.selected,
                    ]
                    .into_iter()
                    .flatten()
                    {
                        if session.id == id {
                            session.updated_ms = updated_ms;
                        }
                    }
                    if let Some(worker) = &self.history.worker {
                        worker.list();
                    }
                }
                HistoryEvent::Deleted { id } => {
                    self.history.deleted(&id);
                }
                HistoryEvent::Failed(error) => {
                    self.history.error = Some(error);
                    self.history.loading = false;
                }
            }
        }
        if self.history.notetaker_retry {
            self.history.notetaker_retry = false;
            self.history_retry();
        }
        if self
            .history
            .selected_dirty
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(700))
        {
            self.history_save_personal_notes();
        }
        if self
            .history
            .dictation_dirty
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(800))
        {
            self.history_save_dictation();
        }
        if self
            .history
            .call_dirty
            .is_some_and(|at| at.elapsed() >= Duration::from_secs(1))
        {
            self.history_save_call();
        }
    }

    pub(super) fn history_retry(&mut self) {
        self.history.error = None;
        self.history_save_personal_notes();
        if self.history.dictation.is_some() {
            self.history_save_dictation();
        }
        if self.history.call.is_some() {
            self.history_save_call();
        }
        if let Some(worker) = &self.history.worker {
            worker.retry();
            if let Some(id) = &self.history.open_requested {
                worker.load(id.clone());
            }
            worker.list();
        }
    }

    fn history_rename(&mut self, session: &mut Session, title: String) {
        session.title = title.clone();
        if let Some(current) = &mut self.history.call
            && current.id == session.id
        {
            current.title = title;
            self.history_save_call();
            session.clone_from(self.history.call.as_ref().unwrap());
        } else if let Some(current) = &mut self.history.dictation
            && current.id == session.id
        {
            current.title = title;
            self.history_save_dictation();
            session.clone_from(self.history.dictation.as_ref().unwrap());
        } else if let Some(worker) = &self.history.worker {
            worker.save(session.clone());
        }
    }

    pub(super) fn history_ui(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing = egui::vec2(10.0, 8.0);
        ui.spacing_mut().button_padding = egui::vec2(12.0, 6.0);
        ui.spacing_mut().interact_size.y = 38.0;
        if let Some(error) = self.history.error.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Color32::LIGHT_YELLOW, "Could not save or open history.");
                if ui.button("Retry").clicked() {
                    self.history_retry();
                }
                ui.label(
                    RichText::new("Your current transcript is still here.")
                        .small()
                        .color(MUTED),
                )
                .on_hover_text(error);
            });
        }
        if self.call.is_some() || self.recording.is_some() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Recording continues").color(ACCENT));
                if ui.small_button("Return to recording").clicked() {
                    self.page = if self.call.is_some() { 3 } else { 0 };
                }
            });
        }
        if self
            .history
            .selected
            .as_ref()
            .is_some_and(|session| session.kind != Kind::Dictation)
        {
            self.notetaker_detail_ui(ui);
            return;
        }
        if self.history.selected.is_some() {
            self.history_detail_ui(ui);
            return;
        }
        ui.horizontal(|ui| {
            theme::page_title(ui, "History");
            ui.label(
                RichText::new(format!("{} saved", self.history.items.len()))
                    .small()
                    .color(MUTED),
            );
            if ui.small_button("Refresh").clicked()
                && let Some(worker) = &self.history.worker
            {
                worker.list();
            }
        });
        ui.label(
            RichText::new("Transcripts and notes, saved on this device.")
                .small()
                .color(MUTED),
        );
        ui.add(
            egui::TextEdit::singleline(&mut self.history.search)
                .hint_text("Search titles or previews")
                .margin(egui::vec2(14.0, 12.0))
                .desired_width(f32::INFINITY),
        );
        if !self.history.notice.is_empty() {
            ui.label(RichText::new(&self.history.notice).small().color(ACCENT));
        }
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.history.kind_filter, 0, "All");
            ui.selectable_value(&mut self.history.kind_filter, 1, "Dictations");
            ui.selectable_value(&mut self.history.kind_filter, 2, "Calls");
        });
        ui.add_space(14.0);
        let query = self.history.search.trim().to_lowercase();
        let mut open = None;
        let mut count = 0;
        egui::ScrollArea::vertical()
            .id_salt("history_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut previous_date = String::new();
                for item in &self.history.items {
                    if (self.history.kind_filter == 1 && !matches!(item.kind, Kind::Dictation))
                        || (self.history.kind_filter == 2 && !matches!(item.kind, Kind::Call))
                    {
                        continue;
                    }
                    if !query.is_empty()
                        && !format!("{} {}", item.title, item.preview)
                            .to_lowercase()
                            .contains(&query)
                    {
                        continue;
                    }
                    count += 1;
                    let day = date(item.created_ms);
                    if previous_date != day {
                        ui.add_space(12.0);
                        ui.label(RichText::new(&day).size(13.0).color(MUTED));
                        ui.add_space(6.0);
                        previous_date = day;
                    }
                    egui::Frame::new()
                        .fill(theme::SURFACE)
                        .corner_radius(10)
                        .inner_margin(14.0)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal_top(|ui| {
                                ui.add(match item.kind {
                                    Kind::Call => theme::Icon::Phone.image(22.0, MUTED),
                                    Kind::Note => theme::Icon::Book.image(22.0, MUTED),
                                    Kind::Dictation => theme::Icon::Mic.image(22.0, MUTED),
                                });
                                ui.vertical(|ui| {
                                    ui.set_min_width(ui.available_width());
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new(&item.title).size(17.0).color(INK),
                                            )
                                            .frame(false)
                                            .truncate(),
                                        )
                                        .on_hover_text(&item.title)
                                        .clicked()
                                    {
                                        open = Some(item.id.clone());
                                    }
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&item.preview).size(14.0).color(MUTED),
                                        )
                                        .truncate(),
                                    );
                                    ui.label(
                                        RichText::new(kind_name(&item.kind))
                                            .size(12.0)
                                            .color(MUTED),
                                    );
                                });
                            });
                        });
                    ui.separator();
                }
                if count == 0 {
                    ui.add_space(28.0);
                    ui.label(
                        RichText::new(if self.history.loading {
                            "Opening your history…"
                        } else if query.is_empty() {
                            "Your words will be here."
                        } else {
                            "No matching transcripts"
                        })
                        .size(24.0),
                    );
                    if query.is_empty() && !self.history.loading {
                        ui.label(
                            "Finished dictations, call transcripts and notes save automatically.",
                        );
                    }
                }
            });
        if let Some(id) = open {
            if self.history.dictation_dirty.is_some() {
                self.history_save_dictation();
            }
            if self.history.call_dirty.is_some() {
                self.history_save_call();
            }
            self.notetaker_open(id);
        }
    }

    fn history_detail_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut session) = self.history.selected.take() else {
            return;
        };
        let mut keep_selected = true;
        let ongoing = (self.call.is_some()
            && self
                .history
                .call
                .as_ref()
                .is_some_and(|s| s.id == session.id))
            || ((self.recording.is_some() || self.busy)
                && self
                    .history
                    .dictation
                    .as_ref()
                    .is_some_and(|s| s.id == session.id));
        let mut remove = false;
        if ui
            .add(egui::Button::new("Back to history").frame(false))
            .clicked()
        {
            keep_selected = false;
        }
        ui.add_space(8.0);
        theme::page_title(ui, &session.title);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("Copy transcript").color(Color32::from_rgb(13, 34, 30)),
                    )
                    .fill(ACCENT),
                )
                .clicked()
            {
                self.history.notice = match platform::copy(&session.text) {
                    Ok(()) => "Transcript copied".into(),
                    Err(error) => error.to_string(),
                };
            }
            ui.menu_button("Export…", |ui| {
                for (label, extension, format) in [
                    ("Plain text", "txt", crate::call_export::Format::Text),
                    ("Markdown", "md", crate::call_export::Format::Markdown),
                    ("Subtitles (SRT)", "srt", crate::call_export::Format::Srt),
                    (
                        "Subtitles (WebVTT)",
                        "vtt",
                        crate::call_export::Format::WebVtt,
                    ),
                ] {
                    if session.rows.is_empty() && extension != "txt" {
                        continue;
                    }
                    if ui.button(label).clicked() {
                        let text = if session.rows.is_empty() {
                            session.text.clone()
                        } else {
                            crate::call_export::export(
                                &session.rows,
                                &session.speaker_names,
                                format,
                            )
                        };
                        self.history.notice = match crate::export_file::save(&text, extension) {
                            Ok(true) => "Transcript exported".into(),
                            Ok(false) => "Export cancelled".into(),
                            Err(error) => error.to_string(),
                        };
                        ui.close();
                    }
                }
            });
            if ui
                .add_enabled(!ongoing, egui::Button::new("Rename"))
                .on_disabled_hover_text("Finish this recording before renaming it.")
                .clicked()
            {
                self.history.renaming = !self.history.renaming;
            }
            if ui
                .add_enabled(!ongoing, egui::Button::new("Delete"))
                .on_disabled_hover_text("Finish this recording before deleting it.")
                .clicked()
            {
                self.history.confirm_delete = true;
            }
        });

        ui.label(
            RichText::new(format!(
                "{} · {} · Saved on this device",
                kind_name(&session.kind),
                date(session.created_ms)
            ))
            .small()
            .color(MUTED),
        );
        if self.history.renaming {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.history.rename).desired_width(320.0));
                if ui
                    .add_enabled(
                        !self.history.rename.trim().is_empty(),
                        egui::Button::new("Save title"),
                    )
                    .clicked()
                {
                    let title = self
                        .history
                        .rename
                        .trim()
                        .chars()
                        .take(120)
                        .collect::<String>();
                    self.history_rename(&mut session, title);
                    self.history.renaming = false;
                }
                if ui.button("Cancel").clicked() {
                    self.history.renaming = false;
                }
            });
        }
        if self.history.confirm_delete {
            ui.horizontal_wrapped(|ui| {
                ui.label("Delete this transcript and its notes from this device?");
                if ui.button("Delete permanently").clicked() {
                    remove = true;
                }
                if ui.button("Keep it").clicked() {
                    self.history.confirm_delete = false;
                }
            });
        }
        if !self.history.notice.is_empty() {
            ui.label(RichText::new(&self.history.notice).small().color(ACCENT));
        }
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.history.detail_tab, 0, "Transcript");
            if session.notes.is_some() {
                ui.selectable_value(&mut self.history.detail_tab, 1, "Notes");
            }
            if !session.original.is_empty() && session.original != session.text {
                ui.selectable_value(&mut self.history.detail_tab, 2, "Original");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.history.detail_search)
                        .hint_text("Find words or a speaker")
                        .desired_width((ui.available_width() - 160.0).clamp(160.0, 260.0))
                        .margin(egui::vec2(10.0, 8.0)),
                );
            });
        });
        ui.separator();
        let available = ui.available_rect_before_wrap();
        let rail = available.width() > 1020.0 && !session.rows.is_empty();
        if rail {
            let rect = egui::Rect::from_min_max(
                egui::pos2(available.right() - 220.0, available.top() + 12.0),
                available.max,
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                theme::people(ui, &session.rows, &session.speaker_names, &self.avatars);
                if session.notes.is_some() && ui.button("Open notes").clicked() {
                    self.history.detail_tab = 1;
                }
            });
        }
        let width = if rail {
            available.width() - 260.0
        } else {
            available.width()
        };
        let reader =
            egui::Rect::from_min_size(available.min, egui::vec2(width, available.height()));
        ui.scope_builder(egui::UiBuilder::new().max_rect(reader), |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("history_detail", &session.id, self.history.detail_tab))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_max_width(840.0_f32.min(width));
                    ui.add_space(16.0);
                    if self.history.detail_tab == 1 {
                        if let Some(notes) = &session.notes {
                            let text = notes.text(&session.speaker_names);
                            ui.horizontal(|ui| {
                                if ui.button("Copy notes").clicked() {
                                    self.history.notice = match platform::copy(&text) {
                                        Ok(()) => "Notes copied".into(),
                                        Err(error) => error.to_string(),
                                    };
                                }
                                if ui.button("Export notes").clicked() {
                                    self.history.notice =
                                        match crate::export_file::save(&text, "txt") {
                                            Ok(true) => "Notes exported".into(),
                                            Ok(false) => "Export cancelled".into(),
                                            Err(error) => error.to_string(),
                                        };
                                }
                            });
                            ui.add(
                                egui::Label::new(RichText::new(text).size(18.0)).selectable(true),
                            );
                        }
                    } else if self.history.detail_tab == 2 {
                        ui.add(
                            egui::Label::new(RichText::new(&session.original).size(18.0))
                                .selectable(true),
                        );
                    } else if session.rows.is_empty() {
                        ui.add(
                            egui::Label::new(RichText::new(&session.text).size(19.0))
                                .selectable(true),
                        );
                    } else {
                        let query = self.history.detail_search.trim().to_lowercase();
                        let mut first = true;
                        for row in &session.rows {
                            if !query.is_empty()
                                && !row.text.to_lowercase().contains(&query)
                                && !calls::label(row, &session.speaker_names)
                                    .to_lowercase()
                                    .contains(&query)
                            {
                                continue;
                            }
                            if !first {
                                ui.add_space(24.0);
                            }
                            first = false;
                            theme::transcript_row(ui, row, &session.speaker_names, &self.avatars);
                        }
                    }
                });
        });
        if remove && let Some(worker) = &self.history.worker {
            worker.delete(session.id.clone());
            self.history.confirm_delete = false;
        }
        if keep_selected {
            self.history.selected = Some(session);
        }
    }
}

fn title(text: &str, fallback: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        fallback.into()
    } else {
        text.chars().take(72).collect()
    }
}

fn kind_name(kind: &Kind) -> &'static str {
    match kind {
        Kind::Dictation => "Dictation",
        Kind::Call => "Call",
        Kind::Note => "Personal note",
    }
}

fn date(ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let days = now.saturating_sub(ms) / 86_400_000;
    match days {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        n => format!("{n} days ago"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleared_preview_overwrites_saved_draft_without_creating_empty_sessions() {
        let (mut app, _) = super::super::tests::app();
        app.history_save_call();
        assert!(app.history.call.is_none());
        app.event_tx
            .send(Event::Call(calls::Update::Preview(vec![calls::Row {
                start_ms: 0,
                end_ms: 1000,
                microphone: false,
                speakers: vec![1],
                discord: None,
                text: "Please send the temporary draft.".into(),
            }])))
            .unwrap();
        app.receive();
        app.call_notes = Some(crate::notes::Notes::build(&app.call_rows));
        app.history_save_call();
        let original = app.history.call.clone().unwrap();
        let temp_root = std::env::temp_dir();
        let directory = temp_root.join(format!("articulate-empty-preview-test-{}", original.id));
        assert_eq!(directory.parent(), Some(temp_root.as_path()));
        let history = crate::history::History::open(directory.clone()).unwrap();
        history.save(original.clone()).unwrap();
        app.event_tx
            .send(Event::Call(calls::Update::Preview(Vec::new())))
            .unwrap();
        app.receive();
        app.history_save_call();
        let cleared = app.history.call.clone().unwrap();
        assert_eq!(cleared.id, original.id);
        assert!(cleared.rows.is_empty());
        assert!(cleared.text.is_empty());
        assert!(cleared.notes.as_ref().unwrap().actions.is_empty());
        history.save(cleared).unwrap();
        assert!(history.load(&original.id).unwrap().text.is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rename_uses_current_draft_not_stale_history_content() {
        let (mut app, _) = super::super::tests::app();
        app.text = "First version".into();
        app.raw = "The original words".into();
        app.history_save_dictation();
        let mut old = app.history.dictation.clone().unwrap();
        app.text = "The user's corrected version".into();
        app.history_rename(&mut old, "Renamed note".into());
        assert_eq!(old.text, app.text);
        assert_eq!(old.original, app.raw);
        assert_eq!(old.title, "Renamed note");
        assert_eq!(old.id, app.history.dictation.as_ref().unwrap().id);
    }

    #[test]
    fn renaming_saved_call_keeps_new_rows_names_and_notes() {
        let (mut app, _) = super::super::tests::app();
        app.call_rows.push(calls::Row {
            start_ms: 0,
            end_ms: 1000,
            microphone: false,
            speakers: vec![1],
            discord: None,
            text: "First part.".into(),
        });
        app.history_save_call();
        let mut old = app.history.call.clone().unwrap();
        app.call_rows.push(calls::Row {
            start_ms: 1000,
            end_ms: 2000,
            microphone: true,
            speakers: vec![],
            discord: None,
            text: "I will review this tomorrow.".into(),
        });
        app.speaker_names[0] = "Alex".into();
        app.call_notes = Some(crate::notes::Notes::build(&app.call_rows));
        app.history_rename(&mut old, "Review call".into());
        assert_eq!(old.rows.len(), 2);
        assert_eq!(old.speaker_names[0], "Alex");
        assert!(old.notes.is_some());
        assert_eq!(old.text, calls::text(&app.call_rows, &app.speaker_names));
        assert_eq!(old.id, app.history.call.as_ref().unwrap().id);
    }

    #[test]
    fn deletion_does_not_resurrect_visible_draft_on_later_edits() {
        let (mut app, _) = super::super::tests::app();
        app.text = "Saved words".into();
        app.history_save_dictation();
        let id = app.history.dictation.as_ref().unwrap().id.clone();
        app.history.deleted(&id);
        app.text.push_str(" with an edit");
        app.history_save_dictation();
        assert!(app.history.dictation.is_none());
        assert_eq!(app.text, "Saved words with an edit");
        app.history.dictation_deleted = false;
        app.history_save_dictation();
        assert_ne!(app.history.dictation.as_ref().unwrap().id, id);
    }

    #[test]
    fn browsing_history_keeps_live_call_and_transcript() {
        let (mut app, _) = super::super::tests::app();
        let control = call_capture::Control::new();
        app.call = Some(control.clone());
        app.text = "Current dictation".into();
        let mut old = Session::new(Kind::Dictation);
        old.text = "Older saved words".into();
        old.title = "Earlier note".into();
        app.history.selected = Some(old);
        let ctx = egui::Context::default();
        theme::configure(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.history_ui(ui));
        });
        assert!(std::sync::Arc::ptr_eq(app.call.as_ref().unwrap(), &control));
        assert_eq!(control.stop_ns.load(Ordering::SeqCst), 0);
        assert_eq!(app.text, "Current dictation");
        assert_eq!(
            app.history.selected.as_ref().unwrap().text,
            "Older saved words"
        );
    }
}
