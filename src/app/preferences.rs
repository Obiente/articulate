use super::*;
use crate::writing_style::{StyleRule, WritingStyle};

fn style_name(style: WritingStyle) -> &'static str {
    match style {
        WritingStyle::Verbatim => "No cleanup",
        WritingStyle::Clean => "Clean",
        WritingStyle::Chat => "Casual chat",
    }
}

impl App {
    pub(super) fn shortcut_preferences(&mut self, ui: &mut egui::Ui, idle: bool) {
        ui.label(RichText::new(self.settings.hotkey.label()).color(ACCENT));
        ui.label(
            RichText::new(
                "Press once to start, again to finish. Choose Type live to write into another app.",
            )
            .small()
            .color(theme::MUTED),
        );
        ui.collapsing("Change keyboard shortcut", |ui| {
            ui.add_enabled_ui(idle && !self.hotkey_pending, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut self.hotkey_draft.ctrl, "Ctrl");
                    ui.checkbox(&mut self.hotkey_draft.alt, "Alt");
                    ui.checkbox(&mut self.hotkey_draft.shift, "Shift");
                    ui.checkbox(&mut self.hotkey_draft.win, "Win");
                    egui::ComboBox::from_id_salt("dictation_shortcut_key")
                        .selected_text(&self.hotkey_draft.key)
                        .show_ui(ui, |ui| {
                            for &key in platform::HOTKEY_KEYS {
                                ui.selectable_value(&mut self.hotkey_draft.key, key.to_owned(), key);
                            }
                        });
                });
                ui.horizontal_wrapped(|ui| {
                    let valid = self.hotkey_draft.validate();
                    let changed = self.hotkey_draft != self.settings.hotkey || self.shortcut_error.is_some();
                    if ui.add_enabled(valid.is_ok() && changed, egui::Button::new("Apply shortcut")).clicked() {
                        match self.hotkey_tx.send(self.hotkey_draft.clone()) {
                            Ok(()) => {
                                self.hotkey_pending = true;
                                self.shortcut_error = None;
                            }
                            Err(_) => self.shortcut_error = Some("Restart Articulate to change your shortcut.".into()),
                        }
                    }
                    if ui.button("Reset to default").clicked() {
                        self.hotkey_draft = platform::Hotkey::default();
                    }
                    if let Err(error) = valid {
                        ui.label(RichText::new(error).small().color(Color32::LIGHT_YELLOW));
                    }
                });
            });
            if self.hotkey_pending {
                ui.label("Checking shortcut availability…");
            }
            ui.label(RichText::new("Changes take effect after you apply them. If a shortcut is taken, your previous shortcut keeps working.").small().color(theme::MUTED));
        });
        if let Some(error) = &self.shortcut_error {
            ui.colored_label(Color32::LIGHT_YELLOW, error);
        }
        ui.add_space(14.0);
    }

    pub(super) fn style_preferences(&mut self, ui: &mut egui::Ui, idle: bool) {
        ui.collapsing("Writing style by app", |ui| {
            ui.label(RichText::new("Keep your wording, with a different cleanup preference for each app.").color(theme::MUTED));
            ui.add_enabled_ui(idle, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("App");
                    ui.add(egui::TextEdit::singleline(&mut self.style_app).hint_text("discord.exe").desired_width(220.0));
                    if let Some(app) = &self.dictation_app
                        && ui.button("Use last app").clicked()
                    {
                        self.style_app = app.clone();
                    }
                    egui::ComboBox::from_id_salt("writing_style")
                        .selected_text(style_name(self.style_draft))
                        .show_ui(ui, |ui| {
                            for style in [WritingStyle::Verbatim, WritingStyle::Clean, WritingStyle::Chat] {
                                ui.selectable_value(&mut self.style_draft, style, style_name(style));
                            }
                        });
                    if ui.button("Save style").clicked() {
                        match dictionary::app_scope(&self.style_app) {
                            Ok(Some(app)) => {
                                if let Some(rule) = self.settings.styles.iter_mut().find(|r| r.app == app) {
                                    rule.style = self.style_draft;
                                } else {
                                    self.settings.styles.push(StyleRule { app, style: self.style_draft });
                                }
                                self.style_message = "Writing style saved".into();
                                self.save();
                            }
                            Ok(None) => self.style_message = "Enter an app name, such as discord.exe.".into(),
                            Err(error) => self.style_message = error.to_string(),
                        }
                    }
                });
                ui.label(RichText::new(match self.style_draft {
                    WritingStyle::Verbatim => "No speech cleanup. Your vocabulary and shortcuts still apply.",
                    WritingStyle::Clean => "Removes supported repetitions and spoken corrections. Keeps your phrasing.",
                    WritingStyle::Chat => "Clean speech with no final period on short, simple messages. Keeps questions and exclamations.",
                }).small().color(theme::MUTED));
                let mut remove = None;
                for (index, rule) in self.settings.styles.iter().enumerate() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&rule.app).color(ACCENT));
                        ui.label(style_name(rule.style));
                        if ui.small_button("Edit").clicked() {
                            self.style_app = rule.app.clone();
                            self.style_draft = rule.style;
                        }
                        if ui.small_button("Remove").clicked() { remove = Some(index); }
                    });
                }
                if let Some(index) = remove {
                    self.settings.styles.remove(index);
                    self.save();
                }
            });
            if !self.style_message.is_empty() { ui.label(&self.style_message); }
        });
        ui.add_space(14.0);
    }
}
