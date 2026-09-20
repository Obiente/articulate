use super::*;

#[derive(Default)]
pub(super) struct CorrectionEditor {
    app: String,
    cues: String,
    ignore_case: bool,
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

fn editor_spacing(ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing = egui::vec2(18.0, 12.0);
    ui.spacing_mut().button_padding = egui::vec2(16.0, 10.0);
}

fn editor_surface() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::from_rgb(22, 30, 32))
        .corner_radius(20.0)
        .inner_margin(24.0)
}

fn field(value: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(value)
        .margin(egui::vec2(12.0, 10.0))
        .desired_width(f32::INFINITY)
}

fn primary(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).color(Color32::from_rgb(13, 34, 30)))
        .fill(ACCENT)
        .corner_radius(18.0)
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
            hint(ui, "Import keeps your other entries and updates entries with the same trigger and scope.");
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

    pub(super) fn corrections_ui(&mut self, ui: &mut egui::Ui) {
        editor_spacing(ui);
        ui.label(RichText::new("Vocabulary").size(32.0));
        ui.label("Your words. Remembered your way.");
        hint(
            ui,
            "Names, phrases, and the words you use every day. Corrections learned in an app stay with that app.",
        );
        ui.add_space(18.0);
        let editor = editor_surface().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.strong(if self.correction_editor.original.is_some() { "Edit vocabulary" } else { "Add to your vocabulary" });
            ui.columns(2, |cols| {
                cols[0].label("When you say");
                cols[0].add(field(&mut self.heard).hint_text("at ExampleHandle"));
                cols[1].label("Write it as");
                cols[1].add(field(&mut self.wanted).hint_text("@ExampleHandle"));
            });
            scope_field(ui, &mut self.correction_editor.app, self.dictation_app.as_deref());
            ui.label("Context words · optional");
            ui.add(field(&mut self.correction_editor.cues).hint_text("crate, function, compiler"));
            hint(ui, "Use this spelling near any of these words in the same sentence. Leave empty for every context.");
            ui.checkbox(&mut self.correction_editor.ignore_case, "Match any capitalization");
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
            ui.horizontal(|ui| {
                if ui.add(primary("Save spelling")).clicked() {
                    match self.correction_draft() {
                        Ok(entry) => {
                            let original = self.correction_editor.original.take();
                            self.settings.entries.retain(|e| !e.same_key(&entry) && !original.as_ref().is_some_and(|old| old.same_key(e)));
                            self.settings.entries.push(entry);
                            self.heard.clear(); self.wanted.clear();
                            self.correction_editor = CorrectionEditor::default();
                            self.status = "Spelling saved".into();
                            self.save();
                            self.correction_editor.message.clone_from(&self.status);
                        }
                        Err(error) => self.correction_editor.message = error.to_string(),
                    }
                }
                if self.correction_editor.original.is_some() && ui.button("Cancel edit").clicked() {
                    self.heard.clear(); self.wanted.clear();
                    self.correction_editor = CorrectionEditor::default();
                }
            });
            if !self.correction_editor.message.is_empty() { ui.label(&self.correction_editor.message); }
        });
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.strong(format!("Your vocabulary · {}", self.settings.entries.len()));
            ui.add(
                field(&mut self.correction_editor.search)
                    .hint_text("Find a word or phrase")
                    .desired_width(260.0),
            );
        });
        let query = self.correction_editor.search.to_lowercase();
        let mut visible_entries = 0;
        let mut edit = None;
        let mut remove = None;
        let mut changed = false;
        for (i, entry) in self.settings.entries.iter_mut().enumerate() {
            if !format!(
                "{} {} {} {}",
                entry.heard,
                entry.wanted,
                entry.app.as_deref().unwrap_or("all apps"),
                entry.cues.join(" ")
            )
            .to_lowercase()
            .contains(&query)
            {
                continue;
            }
            visible_entries += 1;
            ui.push_id(i, |ui| {
                divider(ui);
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(4, 12))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal_wrapped(|ui| {
                            changed |= ui
                                .checkbox(&mut entry.enabled, "")
                                .on_hover_text("Use this spelling")
                                .changed();
                            ui.label(&entry.heard);
                            ui.label("→");
                            ui.label(RichText::new(&entry.wanted).color(ACCENT).strong());
                            if ui.small_button("Edit").clicked() {
                                edit = Some(entry.clone());
                            }
                            if ui.small_button("Remove").clicked() {
                                remove = Some(i);
                            }
                        });
                        let mut detail = entry.app.clone().unwrap_or_else(|| "All apps".into());
                        if !entry.cues.is_empty() {
                            detail.push_str(&format!(" · Near {}", entry.cues.join(", ")));
                        }
                        detail.push_str(if entry.ignore_case {
                            " · Any case"
                        } else {
                            " · Exact case"
                        });
                        hint(ui, &detail);
                    });
            });
        }
        if let Some(entry) = edit {
            self.heard = entry.heard.clone();
            self.wanted = entry.wanted.clone();
            self.correction_editor.app = entry.app.clone().unwrap_or_default();
            self.correction_editor.cues = entry.cues.join(", ");
            self.correction_editor.ignore_case = entry.ignore_case;
            self.correction_editor.original = Some(entry);
            self.correction_editor.message.clear();
            ui.scroll_to_rect(editor.response.rect, Some(egui::Align::Min));
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
        } else if visible_entries == 0 {
            ui.add_space(18.0);
            hint(ui, "No matches. Try another word, phrase, or app.");
        }
    }

    fn correction_draft(&self) -> anyhow::Result<Entry> {
        let mut entry = dictionary::validate(&self.heard, &self.wanted)?;
        entry.app = dictionary::app_scope(&self.correction_editor.app)?;
        entry.cues = dictionary::cue_words(&self.correction_editor.cues)?;
        entry.ignore_case = self.correction_editor.ignore_case;
        entry.enabled = self
            .correction_editor
            .original
            .as_ref()
            .is_none_or(|e| e.enabled);
        Ok(entry)
    }

    pub(super) fn macros_ui(&mut self, ui: &mut egui::Ui) {
        editor_spacing(ui);
        ui.label(RichText::new("Shortcuts").size(32.0));
        ui.label("A few spoken words. A complete reply.");
        hint(
            ui,
            "Say bang, then a shortcut. Your phrase or template appears when you finish dictation.",
        );
        ui.add_space(18.0);
        let editor = editor_surface().show(ui, |ui| {
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
            ui.label("Expand to");
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
            ui.label("Try it before saving");
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
        });
        ui.add_space(18.0);
        ui.strong(format!("Your shortcuts · {}", self.settings.macros.len()));
        let mut edit = None;
        let mut remove = None;
        let mut changed = false;
        for (i, m) in self.settings.macros.iter_mut().enumerate() {
            ui.push_id(i, |ui| {
                divider(ui);
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(4, 12))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal_wrapped(|ui| {
                            changed |= ui
                                .checkbox(&mut m.enabled, "")
                                .on_hover_text("Use this shortcut")
                                .changed();
                            ui.label(
                                RichText::new(format!("bang {}", m.trigger))
                                    .color(ACCENT)
                                    .strong(),
                            );
                            hint(ui, m.app.as_deref().unwrap_or("All apps"));
                            if ui.small_button("Edit").clicked() {
                                edit = Some(m.clone());
                            }
                            if ui.small_button("Remove").clicked() {
                                remove = Some(i);
                            }
                        });
                        hint(ui, &m.expansion);
                    });
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
            ui.scroll_to_rect(editor.response.rect, Some(egui::Align::Min));
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
