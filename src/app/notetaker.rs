use super::*;
use crate::history::{Kind, Session, Summary};
use theme::{INK, MUTED};

impl App {
    pub(super) fn notetaker_hub(&mut self) {
        self.history_save_personal_notes();
        self.history.selected = None;
        self.history.open_requested = None;
        self.history.renaming = false;
        self.history.confirm_delete = false;
        self.page = 6;
    }

    pub(super) fn notetaker_new_note(&mut self) {
        self.history_save_personal_notes();
        let mut session = Session::new(Kind::Note);
        session.title = "Untitled note".into();
        self.history.selected = Some(session);
        self.history.selected_dirty = Some(Instant::now());
        self.history.open_requested = None;
        self.history.notetaker_tab = 0;
        self.history.detail_search.clear();
        self.history.notice.clear();
        self.history.confirm_delete = false;
        self.history.notetaker_assort = assort_ui::State::configured(self.settings.assort.clone());
        self.page = 6;
        self.history_save_personal_notes();
    }

    pub(super) fn notetaker_open(&mut self, id: String) {
        self.history_save_personal_notes();
        self.history.detail_search.clear();
        self.history.notice.clear();
        self.history.confirm_delete = false;
        if self.call.is_some() && self.history.call.as_ref().is_some_and(|s| s.id == id) {
            self.notetaker_return_to_capture();
            return;
        }
        self.history.notetaker_tab = 0;
        self.history.notetaker_assort = assort_ui::State::configured(self.settings.assort.clone());
        if let Some(session) = &self.history.call
            && session.id == id
        {
            self.history.selected = Some(session.clone());
            self.history.open_requested = None;
            return;
        }
        self.history.selected = None;
        self.history.loading = true;
        self.history.open_requested = Some(id.clone());
        if let Some(worker) = &self.history.worker {
            worker.load(id);
        }
    }

    pub(super) fn notetaker_return_to_capture(&mut self) {
        self.history_save_personal_notes();
        self.history.open_requested = None;
        self.page = 3;
        self.call_tab = 3;
    }

    /// Mirror personal edits immediately, before a concurrent ASR snapshot can
    /// enqueue its next save. Transcript/highlight fields remain independent.
    fn notetaker_changed(&mut self, session: &Session) {
        if let Some(current) = &mut self.history.call
            && current.id == session.id
        {
            current.personal_notes.clone_from(&session.personal_notes);
            current
                .generated_summary
                .clone_from(&session.generated_summary);
            current.title.clone_from(&session.title);
            self.history_call_changed();
        }
        self.history.selected_dirty.get_or_insert_with(Instant::now);
    }

    pub(super) fn history_save_personal_notes(&mut self) {
        if self.history.selected_dirty.is_none() {
            return;
        }
        let Some(session) = self.history.selected.clone() else {
            self.history.selected_dirty = None;
            return;
        };
        self.history.selected_dirty = None;
        if let Some(current) = &mut self.history.call
            && current.id == session.id
        {
            current.personal_notes.clone_from(&session.personal_notes);
            current
                .generated_summary
                .clone_from(&session.generated_summary);
            current.title.clone_from(&session.title);
            self.history_save_call();
            self.history.selected = self.history.call.clone();
        } else if let Some(worker) = &self.history.worker {
            worker.save(session.clone());
        }
        self.history.items.retain(|item| item.id != session.id);
        self.history.items.insert(0, Summary::from(&session));
    }

    fn notetaker_save_status(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = self.history.error.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Color32::LIGHT_YELLOW, "Changes need attention")
                    .on_hover_text(error);
                if ui.button("Retry saving").clicked() {
                    self.history.notetaker_retry = true;
                }
            });
            return;
        }
        let settled = self
            .history
            .worker
            .as_ref()
            .map(|worker| worker.saves_settled());
        match settled {
            Some(Err(error)) => {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(Color32::LIGHT_YELLOW, "Not saved yet")
                        .on_hover_text(error);
                    if ui.button("Retry saving").clicked() {
                        self.history.notetaker_retry = true;
                    }
                });
            }
            Some(Ok(true))
                if self.history.selected_dirty.is_none() && self.history.call_dirty.is_none() =>
            {
                ui.label(RichText::new("Saved on this device").small().color(MUTED));
            }
            Some(_) => {
                ui.label(RichText::new("Saving…").small().color(MUTED));
            }
            None => {
                ui.label(
                    RichText::new(
                        "Saving is unavailable. Keep this window open or copy your notes.",
                    )
                    .small()
                    .color(MUTED),
                );
            }
        }
    }

    pub(super) fn notetaker_ui(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing = egui::vec2(10.0, 8.0);
        if self.history.selected.is_some() {
            self.notetaker_detail_ui(ui);
            return;
        }
        ui.horizontal(|ui| {
            theme::page_title(ui, "Notetaker");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(primary("New note")).clicked() {
                    self.notetaker_new_note();
                }
                if ui.button("Record a conversation").clicked() {
                    self.page = 3;
                    self.call_tab = 2;
                }
            });
        });
        ui.label(RichText::new("Your conversations and personal notes, together.").color(MUTED));
        ui.add_space(12.0);
        self.notetaker_active_card(ui);
        if self.history.error.is_some() {
            self.notetaker_save_status(ui);
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.history.notetaker_search)
                .hint_text("Search titles and previews")
                .desired_width(f32::INFINITY)
                .margin(egui::vec2(12.0, 10.0)),
        );
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.history.notetaker_filter, 0, "All notes");
            ui.selectable_value(&mut self.history.notetaker_filter, 1, "Conversations");
            ui.selectable_value(&mut self.history.notetaker_filter, 2, "Personal notes");
            if ui.small_button("Refresh").clicked()
                && let Some(worker) = &self.history.worker
            {
                worker.list();
            }
        });
        ui.add_space(12.0);
        if self.history.open_requested.is_some() && self.history.error.is_none() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Opening your note…");
            });
        }
        let query = self.history.notetaker_search.trim().to_lowercase();
        let mut items: Vec<_> = self
            .history
            .items
            .iter()
            .filter(|item| {
                item.kind != Kind::Dictation
                    && !(self.call.is_some()
                        && self.history.call.as_ref().is_some_and(|s| s.id == item.id))
                    && (self.history.notetaker_filter != 1 || item.kind == Kind::Call)
                    && (self.history.notetaker_filter != 2 || item.kind == Kind::Note)
                    && (query.is_empty()
                        || format!("{} {}", item.title, item.preview)
                            .to_lowercase()
                            .contains(&query))
            })
            .collect();
        items.sort_by(|a, b| {
            b.created_ms
                .cmp(&a.created_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        let mut open = None;
        egui::ScrollArea::vertical()
            .id_salt("notetaker_hub")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut group = String::new();
                for item in &items {
                    let day = day_label(item.created_ms);
                    if day != group {
                        ui.add_space(8.0);
                        ui.label(RichText::new(&day).small().color(MUTED));
                        ui.add_space(6.0);
                        group = day;
                    }
                    egui::Frame::new()
                        .fill(theme::SURFACE)
                        .corner_radius(12)
                        .inner_margin(16.0)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal_top(|ui| {
                                ui.add(if item.kind == Kind::Call {
                                    theme::Icon::Phone.image(22.0, ACCENT)
                                } else {
                                    theme::Icon::Book.image(22.0, ACCENT)
                                });
                                ui.vertical(|ui| {
                                    ui.set_min_width(ui.available_width());
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new(&item.title).size(18.0).color(INK),
                                            )
                                            .frame(false)
                                            .truncate(),
                                        )
                                        .on_hover_text(&item.title)
                                        .clicked()
                                    {
                                        open = Some(item.id.clone());
                                    }
                                    ui.label(
                                        RichText::new(meeting_metadata(item)).small().color(MUTED),
                                    )
                                    .on_hover_text(full_time(item.created_ms));
                                    if !item.preview.is_empty() {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&item.preview).color(MUTED),
                                            )
                                            .truncate(),
                                        );
                                    }
                                });
                            });
                        });
                    ui.add_space(6.0);
                }
                if items.is_empty() {
                    ui.add_space(32.0);
                    ui.label(
                        RichText::new(if self.history.loading {
                            "Opening your notes…"
                        } else if query.is_empty() {
                            "A place to think and remember"
                        } else {
                            "No matching notes"
                        })
                        .size(23.0),
                    );
                    ui.label(
                        RichText::new(if query.is_empty() {
                            "Start a personal note now, or record a conversation when you're ready."
                        } else {
                            "Try a different title or phrase."
                        })
                        .color(MUTED),
                    );
                }
            });
        if let Some(id) = open {
            self.notetaker_open(id);
        }
    }

    fn notetaker_active_card(&mut self, ui: &mut egui::Ui) {
        let Some(control) = &self.call else {
            return;
        };
        let seconds = control.end_seconds() as u64;
        let title = self
            .history
            .call
            .as_ref()
            .map(|s| s.title.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("Current conversation");
        let title = title.to_owned();
        egui::Frame::new()
            .fill(theme::SELECTED)
            .corner_radius(12)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - 200.0).max(120.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, 50.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.label(
                                RichText::new(format!(
                                    "Recording · {:02}:{:02}",
                                    seconds / 60,
                                    seconds % 60
                                ))
                                .small()
                                .color(ACCENT),
                            );
                            ui.add(egui::Label::new(RichText::new(title).size(19.0)).truncate());
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Open current note").clicked() {
                            self.notetaker_return_to_capture();
                        }
                    });
                });
            });
        ui.add_space(14.0);
    }

    pub(super) fn call_personal_notes_ui(&mut self, ui: &mut egui::Ui) {
        if self.history.call.is_none() && self.call.is_some() {
            self.history.call = Some(Session::new(Kind::Call));
        }
        let Some(mut session) = self.history.call.take() else {
            ui.add_space(16.0);
            ui.label("Start a conversation to write alongside the transcript, or create a personal note.");
            if ui.button("New personal note").clicked() {
                self.notetaker_new_note();
            }
            return;
        };
        self.notetaker_save_status(ui);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Your own notes").color(MUTED));
            note_copy_actions(ui, &session.personal_notes, &mut self.history.notice);
        });
        ui.add_space(8.0);
        let changed = personal_editor(ui, &mut session, "live_personal_notes");
        self.history.call = Some(session);
        if changed {
            self.history_call_changed();
        }
        if !self.history.notice.is_empty() {
            ui.label(RichText::new(&self.history.notice).small().color(MUTED));
        }
    }

    pub(super) fn notetaker_detail_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut session) = self.history.selected.take() else {
            return;
        };
        // The active call remains the authoritative transcript while personal
        // edits are mirrored synchronously by notetaker_changed.
        if let Some(current) = &self.history.call
            && current.id == session.id
        {
            session.rows.clone_from(&current.rows);
            session.text.clone_from(&current.text);
            session.notes.clone_from(&current.notes);
        }
        let mut back = false;
        let mut changed = false;
        let mut deleting = false;
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(if self.page == 5 {
                        "Back to history"
                    } else {
                        "Back to Notetaker"
                    })
                    .frame(false),
                )
                .clicked()
            {
                back = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.notetaker_save_status(ui);
            });
        });
        ui.add_space(8.0);
        changed |= ui
            .add(
                egui::TextEdit::singleline(&mut session.title)
                    .hint_text("Untitled note")
                    .font(egui::FontId::proportional(28.0))
                    .frame(false)
                    .desired_width(f32::INFINITY)
                    .char_limit(120),
            )
            .changed();
        ui.label(
            RichText::new(format!(
                "{} · {}",
                if session.kind == Kind::Call {
                    "Conversation"
                } else {
                    "Personal note"
                },
                full_time(session.created_ms)
            ))
            .small()
            .color(MUTED),
        );
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.history.notetaker_tab, 0, "My notes");
            if session.kind == Kind::Call {
                ui.selectable_value(&mut self.history.notetaker_tab, 3, "Summary");
                ui.selectable_value(&mut self.history.notetaker_tab, 1, "Highlights");
                ui.selectable_value(&mut self.history.notetaker_tab, 2, "Transcript");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("More", |ui| {
                    if ui.button("Copy full note").clicked() {
                        self.history.notice = copy_text(&full_note(&session), "Full note copied");
                        ui.close();
                    }
                    if ui.button("Export full note").clicked() {
                        self.history.notice = export_text(&full_note(&session));
                        ui.close();
                    }
                    if ui.button("Delete note").clicked() {
                        self.history.confirm_delete = true;
                        ui.close();
                    }
                });
            });
        });
        if self.history.confirm_delete {
            ui.horizontal_wrapped(|ui| {
                ui.label("Delete this note and its transcript from this device?");
                if ui.button("Delete permanently").clicked()
                    && let Some(worker) = &self.history.worker
                {
                    worker.delete(session.id.clone());
                    self.history.selected_dirty = None;
                    deleting = true;
                    back = true;
                }
                if ui.button("Keep note").clicked() {
                    self.history.confirm_delete = false;
                }
            });
        }
        if !self.history.notice.is_empty() {
            ui.label(RichText::new(&self.history.notice).small().color(ACCENT));
        }
        ui.separator();
        match self.history.notetaker_tab {
            3 if session.kind == Kind::Call => {
                changed |= self.brain_summary_ui(ui, &mut session);
            }
            1 if session.kind == Kind::Call => {
                egui::ScrollArea::vertical()
                    .id_salt(("saved_highlights", &session.id))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let input = source_transcript(&session);
                        let configuration = self.history.notetaker_assort.configuration.clone();
                        if let Some(summary) = assort_ui::show(
                            ui,
                            &mut self.history.notetaker_assort,
                            input.as_ref(),
                            true,
                        ) && let Some(input) = input.as_ref()
                        {
                            match assort_ui::to_notes(
                                &summary,
                                input,
                                &session.rows,
                                &session.speaker_names,
                            ) {
                                Ok(notes) => {
                                    session.notes = Some(notes);
                                    if self
                                        .history
                                        .call
                                        .as_ref()
                                        .is_some_and(|s| s.id == session.id)
                                    {
                                        self.call_notes.clone_from(&session.notes);
                                        if let Some(current) = &mut self.history.call {
                                            current.notes.clone_from(&session.notes);
                                        }
                                    }
                                    changed = true;
                                }
                                Err(error) => self.history.notice = error.to_string(),
                            }
                        }
                        if configuration != self.history.notetaker_assort.configuration {
                            self.settings.assort =
                                self.history.notetaker_assort.configuration.clone();
                            self.assort.configuration.clone_from(&self.settings.assort);
                            self.save();
                        }
                        if let Some(notes) = &session.notes {
                            ui.add_space(16.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Saved highlights").size(20.0));
                                if ui.button("Copy highlights").clicked() {
                                    self.history.notice = copy_text(
                                        &notes.text(&session.speaker_names),
                                        "Highlights copied",
                                    );
                                }
                            });
                            ui.label(
                                RichText::new("Exact excerpts with speaker-turn time ranges.")
                                    .small()
                                    .color(MUTED),
                            );
                            for (heading, quotes) in [
                                ("Highlights", &notes.highlights),
                                ("Possible actions", &notes.actions),
                            ] {
                                if quotes.is_empty() {
                                    continue;
                                }
                                ui.add_space(14.0);
                                ui.strong(heading);
                                for quote in quotes {
                                    ui.label(
                                        RichText::new(quote.attribution(&session.speaker_names))
                                            .small()
                                            .color(ACCENT),
                                    );
                                    ui.add(
                                        egui::Label::new(RichText::new(&quote.text).size(18.0))
                                            .selectable(true),
                                    );
                                    ui.add_space(12.0);
                                }
                            }
                        }
                    });
            }
            2 if session.kind == Kind::Call => {
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.history.detail_search)
                            .hint_text("Find words or a speaker")
                            .margin(egui::vec2(10.0, 8.0))
                            .desired_width((ui.available_width() - 300.0).clamp(140.0, 480.0)),
                    );
                    if ui.button("Copy transcript").clicked() {
                        self.history.notice = copy_text(&session.text, "Transcript copied");
                    }
                    ui.menu_button("Export", |ui| {
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
                            if ui.button(label).clicked() {
                                let text = crate::call_export::export(
                                    &session.rows,
                                    &session.speaker_names,
                                    format,
                                );
                                self.history.notice =
                                    match crate::export_file::save(&text, extension) {
                                        Ok(true) => "Transcript exported".into(),
                                        Ok(false) => "Export cancelled".into(),
                                        Err(error) => error.to_string(),
                                    };
                                ui.close();
                            }
                        }
                    });
                });
                let query = self.history.detail_search.trim().to_lowercase();
                egui::ScrollArea::vertical()
                    .id_salt(("notetaker_transcript", &session.id))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_max_width(840.0_f32.min(ui.available_width()));
                        if session.rows.is_empty() {
                            ui.label("This conversation has no transcript yet.");
                        }
                        for row in &session.rows {
                            if !query.is_empty()
                                && !row.text.to_lowercase().contains(&query)
                                && !calls::label(row, &session.speaker_names)
                                    .to_lowercase()
                                    .contains(&query)
                            {
                                continue;
                            }
                            ui.add_space(16.0);
                            theme::transcript_row(ui, row, &session.speaker_names, &self.avatars);
                        }
                    });
            }
            _ => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Write what matters to you.").color(MUTED));
                    note_copy_actions(ui, &session.personal_notes, &mut self.history.notice);
                });
                ui.add_space(8.0);
                changed |= personal_editor(ui, &mut session, "saved_personal_notes");
            }
        }
        if changed && !deleting {
            self.notetaker_changed(&session);
        }
        self.history.selected = Some(session);
        if back {
            self.history_save_personal_notes();
            self.history.selected = None;
            self.history.confirm_delete = false;
        }
    }
}

fn primary(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).color(Color32::from_rgb(13, 34, 30)))
        .fill(ACCENT)
        .min_size(egui::vec2(110.0, 38.0))
}

fn personal_editor(ui: &mut egui::Ui, session: &mut Session, salt: &str) -> bool {
    let mut changed = false;
    let height = ui.available_height().max(100.0);
    egui::ScrollArea::vertical()
        .id_salt((salt, &session.id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = ui.available_width().min(920.0);
            changed = ui
                .add_sized(
                    [width, (height - 16.0).max(100.0)],
                    egui::TextEdit::multiline(&mut session.personal_notes)
                        .id(egui::Id::new((salt, &session.id)))
                        .font(egui::FontId::proportional(19.0))
                        .desired_width(width)
                        .desired_rows(12)
                        .margin(egui::vec2(16.0, 14.0))
                        .char_limit(1_000_000)
                        .hint_text("Ideas, questions, next steps…"),
                )
                .changed();
        });
    changed
}

fn note_copy_actions(ui: &mut egui::Ui, text: &str, notice: &mut String) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui
            .add_enabled(!text.is_empty(), egui::Button::new("Export notes"))
            .clicked()
        {
            *notice = export_text(text);
        }
        if ui
            .add_enabled(!text.is_empty(), egui::Button::new("Copy notes"))
            .clicked()
        {
            *notice = copy_text(text, "Notes copied");
        }
    });
}
fn copy_text(text: &str, success: &str) -> String {
    platform::copy(text)
        .map(|()| success.into())
        .unwrap_or_else(|error| error.to_string())
}
fn export_text(text: &str) -> String {
    match crate::export_file::save(text, "md") {
        Ok(true) => "Note exported".into(),
        Ok(false) => "Export cancelled".into(),
        Err(error) => error.to_string(),
    }
}
fn full_note(session: &Session) -> String {
    let mut text = format!(
        "# {}\n\n{}\n\n## My notes\n\n{}\n",
        session.title,
        full_time(session.created_ms),
        session.personal_notes
    );
    if let Some(summary) = &session.generated_summary {
        let stale = summary.validate(&super::brain_ui::source(session)).is_err();
        text.push_str(&format!(
            "\n## Summary{}\n\n{}\n",
            if stale {
                " (earlier transcript version)"
            } else {
                ""
            },
            super::brain_ui::summary_text(summary)
        ));
    }
    if let Some(notes) = &session.notes {
        text.push_str(&format!(
            "\n## Highlights\n\n{}\n",
            notes.text(&session.speaker_names)
        ));
    }
    if !session.text.is_empty() {
        text.push_str(&format!("\n## Transcript\n\n{}\n", session.text));
    }
    text
}
fn source_transcript(session: &Session) -> Option<crate::classification::Transcript> {
    let mut segments: Vec<_> = session
        .rows
        .iter()
        .enumerate()
        .filter(|(_, row)| !row.text.trim().is_empty())
        .map(|(index, row)| crate::classification::Segment {
            id: format!("row-{index}"),
            start_ms: row.start_ms,
            end_ms: row.end_ms,
            speaker: Some(calls::label(row, &session.speaker_names)),
            text: row.text.clone(),
        })
        .collect();
    if segments.is_empty() {
        return None;
    }
    segments.sort_by_key(|segment| segment.start_ms);
    Some(crate::classification::Transcript {
        id: session.id.clone(),
        title: session.title.clone(),
        goal: "Capture final decisions, assigned actions, and important facts.".into(),
        segments,
    })
}
fn local_time(ms: u64) -> time::OffsetDateTime {
    let utc = time::OffsetDateTime::from_unix_timestamp((ms / 1000).min(i64::MAX as u64) as i64)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    utc.to_offset(time::UtcOffset::local_offset_at(utc).unwrap_or(time::UtcOffset::UTC))
}
fn full_time(ms: u64) -> String {
    let time = local_time(ms);
    format!(
        "{:04}-{:02}-{:02} · {:02}:{:02}",
        time.year(),
        u8::from(time.month()),
        time.day(),
        time.hour(),
        time.minute()
    )
}
fn day_label(ms: u64) -> String {
    let time = local_time(ms);
    let now = time::OffsetDateTime::now_utc();
    let now = now.to_offset(time::UtcOffset::local_offset_at(now).unwrap_or(time::UtcOffset::UTC));
    if time.date() == now.date() {
        "Today".into()
    } else if Some(time.date()) == now.date().previous_day() {
        "Yesterday".into()
    } else {
        format!(
            "{:04}-{:02}-{:02}",
            time.year(),
            u8::from(time.month()),
            time.day()
        )
    }
}
fn meeting_metadata(item: &Summary) -> String {
    let time = local_time(item.created_ms);
    let kind = if item.kind == Kind::Call {
        "Conversation"
    } else {
        "Personal note"
    };
    if item.duration_ms > 0 {
        format!(
            "{kind} · {:02}:{:02} · {} min",
            time.hour(),
            time.minute(),
            item.duration_ms.div_ceil(60_000)
        )
    } else {
        format!("{kind} · {:02}:{:02}", time.hour(), time.minute())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personal_note_needs_no_model_or_audio_and_does_not_touch_dictation() {
        let (mut app, commands) = super::super::tests::app();
        app.ready = false;
        app.text = "Unrelated dictation".into();
        app.notetaker_new_note();
        assert_eq!(app.page, 6);
        assert_eq!(app.history.selected.as_ref().unwrap().kind, Kind::Note);
        assert!(app.call.is_none());
        assert!(app.recording.is_none());
        assert_eq!(app.text, "Unrelated dictation");
        assert!(matches!(
            commands.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn returning_to_active_note_does_not_restart_or_stop_capture() {
        let (mut app, commands) = super::super::tests::app();
        let control = call_capture::Control::new();
        app.call = Some(control.clone());
        let mut session = Session::new(Kind::Call);
        session.personal_notes = "Keep this thought.".into();
        app.history.call = Some(session);
        app.notetaker_hub();
        app.notetaker_return_to_capture();
        assert_eq!(app.page, 3);
        assert_eq!(app.call_tab, 3);
        assert!(std::sync::Arc::ptr_eq(app.call.as_ref().unwrap(), &control));
        assert_eq!(control.stop_ns.load(Ordering::SeqCst), 0);
        assert!(matches!(
            commands.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert_eq!(
            app.history.call.as_ref().unwrap().personal_notes,
            "Keep this thought."
        );
    }

    #[test]
    fn active_capture_snapshots_preserve_new_personal_writing() {
        let (mut app, _) = super::super::tests::app();
        let mut session = Session::new(Kind::Call);
        session.personal_notes = "Old thought".into();
        app.history.call = Some(session.clone());
        session.personal_notes = "Revised personal thought".into();
        app.history.selected = Some(session.clone());
        app.notetaker_changed(&session);
        app.call_rows.push(calls::Row {
            start_ms: 0,
            end_ms: 2000,
            microphone: true,
            speakers: vec![],
            discord: None,
            text: "Words from the conversation.".into(),
        });
        app.history_save_call();
        app.history_save_personal_notes();
        let saved = app.history.call.as_ref().unwrap();
        assert_eq!(saved.personal_notes, "Revised personal thought");
        assert_eq!(saved.rows.len(), 1);
        assert!(saved.text.contains("Words from the conversation."));
        assert_eq!(
            app.history.selected.as_ref().unwrap().personal_notes,
            saved.personal_notes
        );
    }

    #[test]
    fn reviewed_summary_survives_call_snapshot_and_personal_note_save() {
        let (mut app, _) = super::super::tests::app();
        let mut session = Session::new(Kind::Call);
        session.generated_summary = Some(crate::brain::Draft {
            schema: 1,
            source_id: session.id.clone(),
            source_hash: "synthetic".into(),
            model: crate::brain::MODEL_LABEL.into(),
            sections: 0,
            elapsed_ms: 1,
            items: vec![],
        });
        let mut previous = session.clone();
        previous.generated_summary = None;
        app.history.call = Some(previous);
        app.history.selected = Some(session.clone());
        app.notetaker_changed(&session);
        app.history_save_call();
        app.history_save_personal_notes();
        assert!(
            app.history
                .call
                .as_ref()
                .unwrap()
                .generated_summary
                .is_some()
        );
        assert!(
            app.history
                .selected
                .as_ref()
                .unwrap()
                .generated_summary
                .is_some()
        );
    }

    #[test]
    fn highlight_sources_and_full_export_keep_personal_notes_separate() {
        let mut session = Session::new(Kind::Call);
        session.title = "Planning".into();
        session.personal_notes = "A private idea not spoken.".into();
        session.rows.push(calls::Row {
            start_ms: 0,
            end_ms: 2000,
            microphone: true,
            speakers: vec![],
            discord: None,
            text: "I will send the update tomorrow.".into(),
        });
        session.text = calls::text(&session.rows, &session.speaker_names);
        session.notes = Some(crate::notes::Notes::build(&session.rows));
        let input = source_transcript(&session).unwrap();
        assert_eq!(input.segments.len(), 1);
        assert!(!input.segments[0].text.contains("private idea"));
        assert_eq!(session.personal_notes, "A private idea not spoken.");
        let exported = full_note(&session);
        assert!(exported.contains("## My notes\n\nA private idea not spoken."));
        assert!(exported.contains("## Transcript"));
    }
    #[test]
    fn active_note_edits_and_later_transcript_survive_restart_in_order() {
        let (mut app, _) = super::super::tests::app();
        let mut session = Session::new(Kind::Call);
        session.personal_notes = "Original thought".into();
        let id = session.id.clone();
        let temp = std::env::temp_dir();
        let directory = temp.join(format!("articulate-notetaker-test-{id}"));
        assert_eq!(directory.parent(), Some(temp.as_path()));
        app.history.worker = Some(crate::history::Worker::test_directory(directory.clone()));
        app.history.call = Some(session.clone());
        app.history_save_call();
        session.personal_notes = "My revised thought, independent of speech.".into();
        app.history.selected = Some(session.clone());
        app.notetaker_changed(&session);
        app.call_rows.push(calls::Row {
            start_ms: 0,
            end_ms: 2000,
            microphone: true,
            speakers: vec![],
            discord: None,
            text: "New words from the call.".into(),
        });
        // The ASR autosave can run before the editor debounce expires.
        app.history_save_call();
        app.notetaker_hub();
        drop(app);
        let reopened = crate::history::History::open(directory.clone())
            .unwrap()
            .load(&id)
            .unwrap();
        assert_eq!(
            reopened.personal_notes,
            "My revised thought, independent of speech."
        );
        assert_eq!(reopened.rows[0].text, "New words from the call.");
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn typing_into_native_editor_autosaves_and_reopens_exact_text() {
        let (mut app, _) = super::super::tests::app();
        app.notetaker_new_note();
        let id = app.history.selected.as_ref().unwrap().id.clone();
        let temp = std::env::temp_dir();
        let directory = temp.join(format!("articulate-notetaker-typing-{id}"));
        assert_eq!(directory.parent(), Some(temp.as_path()));
        app.history.worker = Some(crate::history::Worker::test_directory(directory.clone()));
        let ctx = egui::Context::default();
        theme::configure(&ctx);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(850.0, 620.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input(), |ctx| app.surface(ctx));
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new(("saved_personal_notes", &id))));
        let mut raw = input();
        raw.events.push(egui::Event::Text(
            "Remember the plan.\nAsk Casey before Thursday.".into(),
        ));
        let _ = ctx.run(raw, |ctx| app.surface(ctx));
        assert_eq!(
            app.history.selected.as_ref().unwrap().personal_notes,
            "Remember the plan.\nAsk Casey before Thursday."
        );
        assert!(app.history.selected_dirty.is_some());
        app.history.selected_dirty = Some(Instant::now() - Duration::from_secs(1));
        app.history_poll();
        assert!(app.history.selected_dirty.is_none());
        drop(app);
        let reopened = crate::history::History::open(directory.clone())
            .unwrap()
            .load(&id)
            .unwrap();
        assert_eq!(
            reopened.personal_notes,
            "Remember the plan.\nAsk Casey before Thursday."
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
