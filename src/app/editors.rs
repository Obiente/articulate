use super::*;

#[derive(Default)]
pub(super) struct CorrectionEditor {
    open: bool,
    app: String,
    cues: String,
    ignore_case: bool,
    contexts: Vec<(String, bool)>,
    original: Option<Entry>,
    search: String,
    sample: String,
    message: String,
}

#[derive(Default)]
pub(super) struct MacroEditor {
    trigger: String,
    expansion: String,
    app: String,
    original: Option<(String, Option<String>)>,
    sample: String,
    message: String,
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(13.0)
            .color(Color32::from_rgb(155, 169, 171)),
    );
}

fn context_description(cues: &[String], ignore_case: bool) -> String {
    let context = if cues.is_empty() {
        "Any context".to_owned()
    } else {
        format!("Near {}", cues.join(", "))
    };
    format!(
        "{context} · {}",
        if ignore_case {
            "Any capitalization"
        } else {
            "Exact capitalization"
        }
    )
}

fn editor_spacing(ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing = egui::vec2(12.0, 8.0);
    ui.spacing_mut().button_padding = egui::vec2(12.0, 8.0);
}

fn editor_surface() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::from_rgb(22, 30, 32))
        .corner_radius(12.0)
        .inner_margin(18.0)
}

fn field(value: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(value)
        .margin(egui::vec2(12.0, 10.0))
        .desired_width(f32::INFINITY)
}

fn table_cell(ui: &mut egui::Ui, width: f32, text: RichText) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 30.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(width);
            ui.add(egui::Label::new(text).truncate());
        },
    );
}

fn primary(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).color(Color32::from_rgb(13, 34, 30)))
        .fill(ACCENT)
        .corner_radius(9.0)
        .min_size(egui::vec2(150.0, 40.0))
}

fn divider(ui: &mut egui::Ui) {
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);
}

fn scope_field(ui: &mut egui::Ui, value: &mut String, recent: Option<&str>) {
    ui.label("Where it works");
    ui.horizontal(|ui| {
        ui.add(
            field(value)
                .hint_text("All apps, or discord.exe")
                .desired_width(300.0),
        );
        if let Some(app) = recent
            && ui.small_button(format!("Use {app}")).clicked()
        {
            *value = app.to_owned();
        }
    });
}

impl App {
    pub(super) fn library_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(18.0);
        ui.collapsing("Back up or move your library", |ui| {
            editor_spacing(ui);
            hint(ui, "Take your vocabulary and shortcuts to another computer running Articulate.");
            if ui.button("Copy library").clicked() {
                self.status = match crate::library::export(&self.settings.entries, &self.settings.macros).and_then(|text| platform::copy(&text)) {
                    Ok(()) => "Library copied. Paste it into a file to keep a backup.".into(),
                    Err(error) => error.to_string(),
                };
            }
            ui.add(egui::TextEdit::multiline(&mut self.library_input).desired_width(f32::INFINITY).desired_rows(3).margin(egui::vec2(12.0, 10.0)).hint_text("Paste your saved library here").char_limit(1_000_000));
            hint(ui, "Import keeps other spellings and combines matching vocabulary contexts. Shortcuts with the same trigger and app are updated.");
            if ui.add_enabled(!self.library_input.trim().is_empty(), primary("Import library")).clicked() {
                match crate::library::parse(&self.library_input) {
                    Ok(library) => {
                        let count = library.corrections.len() + library.macros.len();
                        library.merge(&mut self.settings.entries, &mut self.settings.macros);
                        self.library_input.clear();
                        self.status = format!("Imported {count} library entries");
                        self.save();
                    }
                    Err(error) => self.status = error.to_string(),
                }
            }
            ui.label(&self.status);
        });
    }

    pub(super) fn vocabulary_surface(&mut self, ui: &mut egui::Ui) {
        let bounds = ui.available_rect_before_wrap();
        let editing =
            self.correction_editor.open || !self.heard.is_empty() || !self.wanted.is_empty();
        let mut body = bounds;
        if editing {
            body.max.y -= 66.0;
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("vocabulary_page")
                .auto_shrink([false, false])
                .show(ui, |ui| self.corrections_ui(ui));
        });
        if editing {
            let footer = egui::Rect::from_min_max(
                egui::pos2(bounds.left(), body.bottom() + 10.0),
                bounds.max,
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
                self.correction_save_ui(ui)
            });
        }
    }

    fn correction_save_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.add(primary("Save correction")).clicked() {
                match self.correction_draft() {
                    Ok(entry) => {
                        let original = self.correction_editor.original.take();
                        dictionary::save(&mut self.settings.entries, entry, original.as_ref());
                        self.heard.clear();
                        self.wanted.clear();
                        self.correction_editor = CorrectionEditor::default();
                        self.status = "Spelling saved".into();
                        self.save();
                        self.correction_editor.message.clone_from(&self.status);
                    }
                    Err(error) => self.correction_editor.message = error.to_string(),
                }
            }
            if self.correction_editor.original.is_some() && ui.button("Cancel edit").clicked() {
                self.heard.clear();
                self.wanted.clear();
                self.correction_editor = CorrectionEditor::default();
            }
        });
    }

    pub(super) fn corrections_ui(&mut self, ui: &mut egui::Ui) {
        editor_spacing(ui);
        let mut reveal_editor = false;
        ui.horizontal(|ui| {
            theme::page_title(ui, "Vocabulary");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(primary("Add word")).clicked() {
                    reveal_editor = true;
                    self.correction_editor.open = true;
                    self.correction_editor.original = None;
                    self.correction_editor.contexts.clear();
                    self.correction_editor.cues.clear();
                    self.correction_editor.ignore_case = false;
                    self.heard.clear();
                    self.wanted.clear();
                }
            });
        });
        hint(ui, "Saved spellings for names and everyday words.");
        ui.add_space(12.0);
        ui.add(field(&mut self.correction_editor.search).hint_text("Find a word or correction"));
        ui.label(
            RichText::new(format!("Corrections · {}", self.settings.entries.len())).color(ACCENT),
        );
        ui.spacing_mut().item_spacing.y = 6.0;
        self.learning_notice(ui);
        let query = self.correction_editor.search.to_lowercase();
        let mut visible_entries = 0;
        let mut list_shown = false;
        let mut edit = None;
        let mut remove = None;
        let mut changed = false;
        let editing =
            self.correction_editor.open || !self.heard.is_empty() || !self.wanted.is_empty();
        let list_height = if editing {
            (ui.clip_rect().height() * 0.32).clamp(150.0, 300.0)
        } else {
            f32::INFINITY
        };
        let mut entries = |ui: &mut egui::Ui| {
            list_shown = true;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let width = ui.available_width() - 8.0;
                ui.add_space(32.0);
                table_cell(
                    ui,
                    ((width - 280.0) * 0.5).max(80.0),
                    RichText::new("When you say").small().color(theme::MUTED),
                );
                table_cell(
                    ui,
                    ((width - 280.0) * 0.5).max(80.0),
                    RichText::new("Write instead").small().color(theme::MUTED),
                );
                ui.label(RichText::new("Where").small().color(theme::MUTED));
            });
            egui::ScrollArea::vertical()
                .id_salt("vocabulary_entries")
                .max_height(list_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (i, entry) in self.settings.entries.iter_mut().enumerate() {
                        if !format!(
                            "{} {} {} {}",
                            entry.heard,
                            entry.wanted,
                            entry.app.as_deref().unwrap_or("all apps"),
                            entry.all_cues().join(" ")
                        )
                        .to_lowercase()
                        .contains(&query)
                        {
                            continue;
                        }
                        visible_entries += 1;
                        ui.push_id(i, |ui| {
                            ui.separator();
                            egui::Frame::new()
                                .inner_margin(egui::Margin::symmetric(4, 6))
                                .show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    let width = ui.available_width();
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 8.0;
                                        changed |= ui
                                            .checkbox(&mut entry.enabled, "")
                                            .on_hover_text("Use this spelling")
                                            .changed();
                                        table_cell(
                                            ui,
                                            ((width - 280.0) * 0.5).max(80.0),
                                            RichText::new(&entry.heard),
                                        );
                                        table_cell(
                                            ui,
                                            ((width - 280.0) * 0.5).max(80.0),
                                            RichText::new(&entry.wanted).color(ACCENT),
                                        );
                                        ui.add_sized(
                                            [90.0, 30.0],
                                            egui::Label::new(
                                                RichText::new(
                                                    entry.app.as_deref().unwrap_or("All apps"),
                                                )
                                                .small()
                                                .color(theme::MUTED),
                                            )
                                            .truncate(),
                                        );
                                        if ui.small_button("Edit").clicked() {
                                            edit = Some(entry.clone());
                                        }
                                        ui.menu_button("…", |ui| {
                                            if ui.button("Remove correction").clicked() {
                                                remove = Some(i);
                                                ui.close();
                                            }
                                            hint(
                                                ui,
                                                &context_description(
                                                    &entry.cues,
                                                    entry.ignore_case,
                                                ),
                                            );
                                            if entry.has_alternative_contexts() { hint(ui, "Use this spelling in any matching context below."); }
                                            for context in &entry.contexts {
                                                hint(
                                                    ui,
                                                    &format!(
                                                        "Or {}",
                                                        context_description(
                                                            &context.cues,
                                                            context.ignore_case
                                                        )
                                                    ),
                                                );
                                            }
                                        });
                                    });
                                });
                        });
                    }
                });
        };
        if editing && ui.available_width() < 700.0 {
            ui.collapsing("Your vocabulary", entries);
        } else {
            entries(ui);
        }
        if let Some(entry) = edit {
            reveal_editor = true;
            self.heard = entry.heard.clone();
            self.wanted = entry.wanted.clone();
            self.correction_editor.app = entry.app.clone().unwrap_or_default();
            self.correction_editor.cues = entry.cues.join(", ");
            self.correction_editor.ignore_case = entry.ignore_case;
            self.correction_editor.contexts = entry
                .contexts
                .iter()
                .map(|context| (context.cues.join(", "), context.ignore_case))
                .collect();
            self.correction_editor.original = Some(entry);
            self.correction_editor.message.clear();
            self.correction_editor.open = true;
        }
        if let Some(i) = remove {
            self.settings.entries.remove(i);
            changed = true;
        }
        if changed {
            self.save();
        }
        if self.settings.entries.is_empty() {
            hint(
                ui,
                "Your vocabulary starts with a correction. Edit a name after dictating, or add a spelling above.",
            );
        } else if list_shown && visible_entries == 0 {
            ui.add_space(18.0);
            hint(ui, "No matches. Try another word, phrase, or app.");
        }
        if self.correction_editor.open || !self.heard.is_empty() || !self.wanted.is_empty() {
            ui.add_space(18.0);
            let editor=editor_surface().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.strong(if self.correction_editor.original.is_some() { "Edit vocabulary" } else { "Add to your vocabulary" });
            ui.columns(2, |cols| {
                cols[0].label("When you say");
                let width = (cols[0].available_width()-24.0).max(80.0);
                cols[0].add(field(&mut self.heard).desired_width(width).hint_text("at ExampleHandle"));
                cols[1].label("Write it as");
                let width = (cols[1].available_width()-24.0).max(80.0);
                cols[1].add(field(&mut self.wanted).desired_width(width).hint_text("@ExampleHandle"));
            });
            scope_field(ui, &mut self.correction_editor.app, self.dictation_app.as_deref());
            let context_header = egui::CollapsingHeader::new("Context and matching");
            #[cfg(test)]
            let context_header = context_header.default_open(std::env::var_os("ARTICULATE_UI_CONTEXTS").is_some());
            let context_response = context_header.show(ui, |ui| {
            ui.label("Context words · optional");
            ui.add(field(&mut self.correction_editor.cues).hint_text("crate, function, compiler"));
            hint(ui, "Use this spelling near any of these words in the same sentence. Leave empty for every context.");
            ui.checkbox(&mut self.correction_editor.ignore_case, "Match any capitalization");
            let mut remove_context = None;
            for (index, (cues, ignore_case)) in self.correction_editor.contexts.iter_mut().enumerate() {
                ui.push_id(index, |ui| {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Or in this context");
                        if ui.small_button("Remove").clicked() { remove_context = Some(index); }
                    });
                    ui.add(field(cues).hint_text("Context words, separated by commas"));
                    hint(ui, "Leave empty to match in any context.");
                    ui.checkbox(ignore_case, "Match any capitalization");
                });
            }
            if let Some(index) = remove_context { self.correction_editor.contexts.remove(index); }
            if ui.add_enabled(self.correction_editor.contexts.len() < 32, egui::Button::new("Add another context").small()).clicked() {
                self.correction_editor.contexts.push((String::new(), false));
            }
            });
            #[cfg(test)]
            if std::env::var_os("ARTICULATE_UI_CONTEXTS").is_some() {
                ui.scroll_to_rect(context_response.header_response.rect, Some(egui::Align::Min));
            }
            #[cfg(not(test))]
            let _ = context_response;
            ui.collapsing("Try this correction", |ui| {
                ui.add(egui::TextEdit::multiline(&mut self.correction_editor.sample).margin(egui::vec2(12.0, 10.0)).desired_rows(2).desired_width(f32::INFINITY).hint_text("Type an example sentence"));
                if !self.correction_editor.sample.is_empty() {
                    match self.correction_draft() {
                        Ok(entry) => {
                            let (text, count) = dictionary::apply_in(&self.correction_editor.sample, std::slice::from_ref(&entry), entry.app.as_deref());
                            ui.label(RichText::new(text).color(ACCENT));
                            hint(ui, &format!("{count} matches in {}", entry.app.as_deref().unwrap_or("all apps")));
                        }
                        Err(error) => { hint(ui, &error.to_string()); }
                    }
                }
            });
            if !self.correction_editor.message.is_empty() { ui.label(&self.correction_editor.message); }
        });
            if reveal_editor {
                ui.scroll_to_rect(editor.response.rect, Some(egui::Align::Max));
            }
        }
    }

    fn correction_draft(&self) -> anyhow::Result<Entry> {
        let mut entry = dictionary::validate(&self.heard, &self.wanted)?;
        entry.app = dictionary::app_scope(&self.correction_editor.app)?;
        entry.cues = dictionary::cue_words(&self.correction_editor.cues)?;
        entry.ignore_case = self.correction_editor.ignore_case;
        entry.contexts = self
            .correction_editor
            .contexts
            .iter()
            .map(|(cues, ignore_case)| {
                Ok(dictionary::Context {
                    cues: dictionary::cue_words(cues)?,
                    ignore_case: *ignore_case,
                })
            })
            .collect::<anyhow::Result<_>>()?;
        entry.enabled = self
            .correction_editor
            .original
            .as_ref()
            .is_none_or(|e| e.enabled);
        Ok(entry)
    }

    pub(super) fn macros_ui(&mut self, ui: &mut egui::Ui) {
        editor_spacing(ui);
        ui.horizontal(|ui| {
            theme::page_title(ui, "Shortcuts");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(primary("New shortcut")).clicked() {
                    self.macro_editor = MacroEditor::default();
                }
            });
        });
        hint(
            ui,
            "Say bang, then a shortcut. Your phrase or template appears when you finish dictation.",
        );
        ui.add_space(18.0);
        let wide = ui.available_width() >= 900.0;
        if wide {
            let bounds = ui.available_rect_before_wrap();
            let left_width = (bounds.width() * 0.28).min(310.0);
            let left =
                egui::Rect::from_min_size(bounds.min, egui::vec2(left_width, bounds.height()));
            let right =
                egui::Rect::from_min_max(egui::pos2(left.right() + 36.0, bounds.top()), bounds.max);
            ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("shortcut_library")
                    .show(ui, |ui| self.shortcut_list(ui));
            });
            ui.painter().line_segment(
                [
                    egui::pos2(left.right() + 18.0, bounds.top()),
                    egui::pos2(left.right() + 18.0, bounds.bottom()),
                ],
                egui::Stroke::new(1.0_f32, theme::LINE),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
                self.shortcut_editor_ui(ui)
            });
        } else {
            ui.collapsing("Your shortcuts", |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("shortcut_library_compact")
                    .max_height(160.0)
                    .show(ui, |ui| self.shortcut_list(ui));
            });
            self.shortcut_editor_ui(ui);
        }
    }

    fn shortcut_editor_ui(&mut self, ui: &mut egui::Ui) {
        let bounds = ui.available_rect_before_wrap();
        let body = egui::Rect::from_min_max(
            bounds.min,
            egui::pos2(
                bounds.right(),
                (bounds.bottom() - 66.0).max(bounds.top() + 100.0),
            ),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("shortcut_fields")
                .auto_shrink([false, false])
                .show(ui, |ui| self.shortcut_fields_ui(ui));
        });
        let footer =
            egui::Rect::from_min_max(egui::pos2(bounds.left(), body.bottom() + 10.0), bounds.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
            self.shortcut_save_ui(ui);
        });
    }

    fn shortcut_save_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.add(primary("Save shortcut")).clicked() {
                match macros::validate(
                    &self.macro_editor.trigger,
                    &self.macro_editor.expansion,
                    &self.macro_editor.app,
                ) {
                    Ok(mut m) => {
                        let original = self.macro_editor.original.take();
                        if let Some((trigger, app)) = &original
                            && let Some(old) = self
                                .settings
                                .macros
                                .iter()
                                .find(|e| &e.trigger == trigger && &e.app == app)
                        {
                            m.enabled = old.enabled;
                        }
                        self.settings.macros.retain(|e| {
                            !(e.trigger == m.trigger && e.app == m.app
                                || original.as_ref().is_some_and(|(trigger, app)| {
                                    &e.trigger == trigger && &e.app == app
                                }))
                        });
                        self.settings.macros.push(m);
                        self.macro_editor = MacroEditor::default();
                        self.status = "Shortcut saved".into();
                        self.save();
                        self.macro_editor.message.clone_from(&self.status);
                    }
                    Err(error) => self.macro_editor.message = error.to_string(),
                }
            }
            if self.macro_editor.original.is_some() {
                if ui.button("Cancel edit").clicked() {
                    self.macro_editor = MacroEditor::default();
                }
            } else if self.macro_editor.trigger.is_empty() {
                if ui.small_button("Signature example").clicked() {
                    self.macro_editor.trigger = "signature".into();
                    self.macro_editor.expansion = "Thanks,\nYour name".into();
                    self.macro_editor.sample = "bang signature".into();
                }
                if ui.small_button("Reply template").clicked() {
                    self.macro_editor.trigger = "quick reply".into();
                    self.macro_editor.expansion = "Thanks for the update. {text}".into();
                    self.macro_editor.sample = "bang quick reply I will check tomorrow.".into();
                }
            }
        });
        if !self.macro_editor.message.is_empty() {
            ui.label(&self.macro_editor.message);
        }
    }

    fn shortcut_fields_ui(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(18, 0))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.strong(if self.macro_editor.original.is_some() {
                    "Edit shortcut"
                } else {
                    "Create a shortcut"
                });
                ui.label("Spoken trigger");
                ui.horizontal(|ui| {
                    ui.label(RichText::new("bang").color(ACCENT).strong());
                    ui.add(
                        field(&mut self.macro_editor.trigger)
                            .desired_width(240.0)
                            .hint_text("signature"),
                    );
                });
                ui.label("Expansion");
                ui.add(
                    egui::TextEdit::multiline(&mut self.macro_editor.expansion)
                        .margin(egui::vec2(12.0, 10.0))
                        .desired_rows(4)
                        .desired_width(f32::INFINITY)
                        .hint_text("Thanks,\nYour name")
                        .font(egui::TextStyle::Body),
                );
                hint(
                    ui,
                    "Use {text} once to include everything you say after the trigger.",
                );
                scope_field(
                    ui,
                    &mut self.macro_editor.app,
                    self.dictation_app.as_deref(),
                );
                divider(ui);
                ui.label("Try it");
                ui.add(
                    field(&mut self.macro_editor.sample)
                        .desired_width(f32::INFINITY)
                        .hint_text("bang signature"),
                );
                if !self.macro_editor.sample.is_empty() {
                    match macros::validate(
                        &self.macro_editor.trigger,
                        &self.macro_editor.expansion,
                        &self.macro_editor.app,
                    )
                    .and_then(|m| {
                        macros::expand(
                            &self.macro_editor.sample,
                            std::slice::from_ref(&m),
                            m.app.as_deref(),
                        )
                    }) {
                        Ok(Some(expansion)) => {
                            ui.add_space(6.0);
                            ui.label(RichText::new(expansion.text).color(ACCENT));
                        }
                        Ok(None) => {
                            hint(ui, "No match. Start with bang and the exact trigger.");
                        }
                        Err(error) => {
                            hint(ui, &error.to_string());
                        }
                    }
                }
            });
    }

    fn shortcut_list(&mut self, ui: &mut egui::Ui) {
        ui.add_space(18.0);
        ui.strong(format!("Your shortcuts · {}", self.settings.macros.len()));
        let search_id = egui::Id::new("shortcut_search");
        let mut search = ui
            .ctx()
            .data_mut(|data| data.get_temp::<String>(search_id).unwrap_or_default());
        ui.add(field(&mut search).hint_text("Find a shortcut"));
        ui.ctx()
            .data_mut(|data| data.insert_temp(search_id, search.clone()));
        let mut edit = None;
        let mut remove = None;
        let mut changed = false;
        for (i, m) in self.settings.macros.iter_mut().enumerate() {
            if !m.trigger.to_lowercase().contains(&search.to_lowercase()) {
                continue;
            }
            let selected = self
                .macro_editor
                .original
                .as_ref()
                .is_some_and(|(trigger, app)| trigger == &m.trigger && app == &m.app);
            ui.push_id(i, |ui| {
                egui::Frame::new()
                    .fill(if selected {
                        Color32::from_rgb(28, 48, 44)
                    } else {
                        Color32::TRANSPARENT
                    })
                    .corner_radius(10)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            changed |= ui
                                .checkbox(&mut m.enabled, "")
                                .on_hover_text("Use this shortcut")
                                .changed();
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(&m.trigger).size(17.0).color(if selected {
                                            ACCENT
                                        } else {
                                            theme::INK
                                        }),
                                    )
                                    .frame(false),
                                )
                                .clicked()
                            {
                                edit = Some(m.clone());
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.menu_button("…", |ui| {
                                        if ui.button("Remove shortcut").clicked() {
                                            remove = Some(i);
                                            ui.close();
                                        }
                                    });
                                },
                            );
                        });
                        hint(ui, &format!("Say bang {}", m.trigger));
                    });
                ui.add_space(4.0);
            });
        }
        if let Some(m) = edit {
            self.macro_editor = MacroEditor {
                original: Some((m.trigger.clone(), m.app.clone())),
                sample: format!(
                    "bang {}{}",
                    m.trigger,
                    if m.expansion.contains("{text}") {
                        " your words here"
                    } else {
                        ""
                    }
                ),
                trigger: m.trigger,
                expansion: m.expansion,
                app: m.app.unwrap_or_default(),
                message: String::new(),
            };
        }
        if let Some(i) = remove {
            self.settings.macros.remove(i);
            changed = true;
        }
        if changed {
            self.save();
        }
        if self.settings.macros.is_empty() {
            hint(
                ui,
                "Make a signature, a familiar reply, or a template you can fill with your voice.",
            );
        }
    }
}
#[cfg(test)]
impl App {
    pub(super) fn populate_editor_capture(&mut self, macros: bool) {
        if macros {
            for (trigger, expansion) in [
                ("signature", "Thanks,\n{text}\nCasey"),
                (
                    "follow up",
                    "Thanks for joining. Here are the next steps: {text}",
                ),
                ("update", "Project update: {text}"),
            ] {
                self.settings
                    .macros
                    .push(crate::macros::validate(trigger, expansion, "").unwrap());
            }
            self.macro_editor = MacroEditor {
                trigger: "signature".into(),
                expansion: "Thanks,\n{text}\nCasey".into(),
                original: Some(("signature".into(), None)),
                sample: "bang signature I will send the draft tomorrow.".into(),
                ..Default::default()
            };
        } else {
            for (heard, wanted) in [
                ("at Casey", "@Casey"),
                ("articulate", "Articulate"),
                ("check in", "check-in"),
                ("repo", "repository"),
                ("qwen", "Qwen"),
            ] {
                self.settings
                    .entries
                    .push(dictionary::validate(heard, wanted).unwrap());
            }
            self.heard = "check in".into();
            self.wanted = "check-in".into();
            self.correction_editor.open = true;
            self.correction_editor.original = Some(self.settings.entries[2].clone());
            if std::env::var_os("ARTICULATE_UI_CONTEXTS").is_some() {
                self.correction_editor.cues = "meeting, project".into();
                self.correction_editor.contexts = vec![("calendar, tomorrow".into(), true)];
            }
        }
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;

    #[test]
    fn editing_preserves_and_can_remove_alternative_contexts() {
        let (mut app, _) = super::super::tests::app();
        let mut original = dictionary::validate("rust", "Rust").unwrap();
        original.cues = vec!["compiler".into()];
        original.contexts.push(dictionary::Context {
            cues: vec!["crate".into()],
            ignore_case: true,
        });
        app.heard = original.heard.clone();
        app.wanted = original.wanted.clone();
        app.correction_editor.cues = "compiler".into();
        app.correction_editor.contexts = vec![("crate".into(), true)];
        app.correction_editor.original = Some(original.clone());
        assert!(app.correction_draft().unwrap() == original);
        app.correction_editor.contexts.clear();
        let mut entries = vec![original.clone()];
        dictionary::save(
            &mut entries,
            app.correction_draft().unwrap(),
            Some(&original),
        );
        assert_eq!(entries.len(), 1);
        assert!(entries[0].contexts.is_empty());
        assert_eq!(entries[0].cues, ["compiler"]);
    }
}
