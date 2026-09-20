use super::*;
use theme::{INK, Icon, LINE, MUTED, SURFACE};

impl App {
    pub(super) fn surface(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::from_rgb(15, 21, 23)))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                theme::background(ui.painter(), rect);
                let padding = if rect.width() > 1000.0 { 30.0 } else { 20.0 };
                let header = egui::Rect::from_min_max(
                    rect.min + egui::vec2(padding, 16.0),
                    egui::pos2(rect.right() - padding, rect.top() + 88.0),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(header), |ui| {
                    self.header(ui)
                });
                let footer = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + padding, rect.bottom() - 44.0),
                    egui::pos2(rect.right() - padding, rect.bottom() - 8.0),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
                    ui.horizontal(|ui| {
                        ui.add(Icon::Lock.image(18.0, MUTED));
                        ui.label(RichText::new("On this device").size(13.0).color(MUTED));
                        if self.history.error.is_some()
                            && ui.small_button("History needs attention").clicked()
                        {
                            self.page = 5;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(
                                    egui::Button::image_and_text(
                                        Icon::Bulb.image(19.0, MUTED),
                                        RichText::new("Say bang, then your shortcut")
                                            .size(13.0)
                                            .color(MUTED),
                                    )
                                    .frame(false),
                                )
                                .clicked()
                            {
                                self.page = 4;
                            }
                        });
                    });
                });
                if self.page == 0 {
                    let dock_w = (rect.width() - 100.0).min(920.0);
                    let dock = egui::Rect::from_center_size(
                        egui::pos2(rect.center().x, rect.bottom() - 104.0),
                        egui::vec2(dock_w, 94.0),
                    );
                    let body = egui::Rect::from_min_max(
                        egui::pos2(rect.left() + padding, rect.top() + 98.0),
                        egui::pos2(rect.right() - padding, dock.top() - 18.0),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                        self.dictation_surface(ui)
                    });
                    ui.scope_builder(egui::UiBuilder::new().max_rect(dock), |ui| {
                        self.recording_dock(ui)
                    });
                } else {
                    let inset = if self.page == 3 || self.page == 5 {
                        padding
                    } else {
                        (rect.width() * 0.09).clamp(30.0, 130.0)
                    };
                    let content = egui::Rect::from_min_max(
                        egui::pos2(
                            rect.left() + inset,
                            rect.top()
                                + if self.page == 3 || self.page == 5 {
                                    100.0
                                } else {
                                    116.0
                                },
                        ),
                        egui::pos2(rect.right() - inset, footer.top() - 12.0),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
                        if self.page == 3 {
                            self.calls_ui(ui, ctx);
                            return;
                        }
                        if self.page == 5 {
                            self.history_ui(ui);
                            return;
                        }
                        egui::ScrollArea::vertical()
                            .id_salt(("workspace", self.page))
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                self.learning_notice(ui);
                                match self.page {
                                    1 => self.corrections_ui(ui),
                                    2 => self.settings_ui(ui, ctx),
                                    3 => self.calls_ui(ui, ctx),
                                    4 => self.macros_ui(ui),
                                    _ => {}
                                }
                            });
                    });
                }
            });
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        let wide = ui.available_width() > 1050.0;
        ui.horizontal(|ui| {
            if !wide {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.spacing_mut().button_padding.x = 10.0;
            }
            ui.add(
                egui::Image::new(egui::include_image!(
                    "../../assets/brand/articulate-mark.svg"
                ))
                .fit_to_exact_size(egui::vec2(46.0, 32.0)),
            );
            ui.label(
                RichText::new("Articulate")
                    .size(if wide { 27.0 } else { 24.0 })
                    .color(INK),
            );
            ui.add_space(if wide {
                (ui.available_width() - 690.0).max(22.0) * 0.4
            } else {
                10.0
            });
            for (page, label, icon) in [
                (0, "Dictate", Icon::Mic),
                (3, "Calls", Icon::Phone),
                (1, "Vocabulary", Icon::Book),
                (4, "Shortcuts", Icon::Bolt),
                (5, "History", Icon::History),
            ] {
                let active = self.page == page;
                let tint = if active { ACCENT } else { MUTED };
                let button = egui::Button::image_and_text(
                    icon.image(if wide { 21.0 } else { 18.0 }, tint),
                    RichText::new(label)
                        .size(if wide { 17.0 } else { 14.0 })
                        .color(if active { INK } else { MUTED }),
                )
                .corner_radius(28)
                .fill(if active {
                    SURFACE
                } else {
                    Color32::TRANSPARENT
                })
                .stroke(if active {
                    egui::Stroke::new(1.0_f32, LINE)
                } else {
                    egui::Stroke::NONE
                })
                .min_size(egui::vec2(if wide { 112.0 } else { 75.0 }, 48.0));
                let response = ui.add(button);
                let selected =
                    ui.ctx()
                        .animate_bool_with_time(response.id.with("selected"), active, 0.18);
                if selected > 0.0 {
                    let y = response.rect.bottom() - 2.0;
                    ui.painter().line_segment(
                        [
                            egui::pos2(response.rect.left() + 22.0, y),
                            egui::pos2(response.rect.right() - 22.0, y),
                        ],
                        egui::Stroke::new(2.0_f32, ACCENT.gamma_multiply(selected)),
                    );
                }
                if response.clicked() {
                    self.page = page;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::image(
                            Icon::Gear.image(23.0, if self.page == 2 { ACCENT } else { MUTED }),
                        )
                        .frame(false),
                    )
                    .on_hover_text("Settings")
                    .clicked()
                {
                    self.page = 2;
                }
            });
        });
    }

    fn dictation_surface(&mut self, ui: &mut egui::Ui) {
        let idle = self.recording.is_none() && !self.busy && !self.loading && self.call.is_none();
        let bounds = ui.max_rect();
        let mode_bounds = egui::Rect::from_min_size(bounds.min, egui::vec2(bounds.width(), 48.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(mode_bounds).layout(egui::Layout::right_to_left(egui::Align::Min)),|ui| {
            ui.add_enabled_ui(idle,|ui| {
                egui::Frame::new().stroke(egui::Stroke::new(1.0_f32,LINE)).corner_radius(24).inner_margin(3.0).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let live = self.settings.insert;
                        if ui.add(egui::Button::new(RichText::new("Review").color(if live {MUTED}else{INK})).corner_radius(22).fill(if live {Color32::TRANSPARENT}else{SURFACE}).stroke(egui::Stroke::NONE)).on_hover_text("Keep your words here to review and copy.").clicked() { self.settings.insert=false; self.save(); }
                        let label = if live && !self.settings.live_insert { "On finish" } else { "Type live" };
                        if ui.add(egui::Button::new(RichText::new(label).color(if live {INK}else{MUTED})).corner_radius(22).fill(if live {Color32::from_rgb(36,65,58)}else{Color32::TRANSPARENT}).stroke(egui::Stroke::NONE)).on_hover_text(format!("Click a text field in another app, then press {}. Your words appear as you speak.", self.settings.hotkey.label())).clicked() {
                            self.settings.insert=true; self.settings.live_insert=true; self.save();
                        }

                    });
                });
            });
        });
        let gap = ((bounds.height() - 48.0) * 0.12).clamp(16.0, 64.0);
        let width = bounds.width();
        let side = (width * 0.095).clamp(26.0, 125.0);
        let content = egui::Rect::from_min_max(
            egui::pos2(bounds.left() + side, mode_bounds.bottom() + gap),
            egui::pos2(bounds.right() - side, bounds.bottom()),
        );
        // A horizontal row reports only its initial line height to nested children.
        // Give the editor its own bounded vertical workspace before measuring it.
        ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
            ui.set_width(content.width());
            let recording = self.recording.is_some();
            let state = if recording {
                if self.live_mode {
                    format!(
                        "Listening in {}",
                        self.dictation_app.as_deref().unwrap_or("your editor")
                    )
                } else {
                    "Listening to you".into()
                }
            } else if self.busy {
                "Finishing your words".into()
            } else if self.loading {
                "Getting ready".into()
            } else if self.text.is_empty() {
                "Ready when you are".into()
            } else {
                "Your words, ready to use".into()
            };
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(9.0, 20.0), egui::Sense::hover());
                ui.painter().circle_filled(
                    dot.center(),
                    4.0,
                    if self.ready { ACCENT } else { MUTED },
                );
                ui.label(RichText::new(state).size(17.0).color(ACCENT));
                if self.busy || self.loading {
                    ui.spinner();
                }
            });
            if self.text.is_empty() && !recording && !self.busy && !self.loading {
                ui.label(
                    RichText::new(if self.settings.insert {
                        format!(
                            "Click a text field in another app, then press {}.",
                            self.settings.hotkey.label()
                        )
                    } else {
                        format!(
                            "Speak, review, then copy. Press {} to start.",
                            self.settings.hotkey.label()
                        )
                    })
                    .size(14.0)
                    .color(MUTED),
                );
            }
            ui.add_space(18.0);
            let size = (ui.available_width() / 25.0).clamp(26.0, 38.0);
            let highlight = self.remembered.last().map(|c| c.entry.wanted.clone());
            let mut layout = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
                let text = buffer.as_str();
                let mut job = egui::text::LayoutJob::default();
                job.wrap.max_width = wrap_width;
                let normal = egui::TextFormat {
                    font_id: egui::FontId::proportional(size),
                    color: INK,
                    line_height: Some(size * 1.42),
                    ..Default::default()
                };
                if let Some(word) = highlight.as_deref().filter(|w| !w.is_empty()) {
                    let mut at = 0;
                    for (i, _) in text.match_indices(word) {
                        job.append(&text[at..i], 0.0, normal.clone());
                        job.append(
                            word,
                            0.0,
                            egui::TextFormat {
                                color: ACCENT,
                                background: Color32::from_rgb(28, 49, 45),
                                ..normal.clone()
                            },
                        );
                        at = i + word.len();
                    }
                    job.append(&text[at..], 0.0, normal);
                } else {
                    job.append(text, 0.0, normal);
                }
                ui.fonts_mut(|fonts| fonts.layout_job(job))
            };
            egui::ScrollArea::vertical()
                .id_salt("transcript_body")
                .max_height((content.bottom() - ui.cursor().top() - 128.0).max(size * 2.84 + 8.0))
                .auto_shrink([false, true])
                .stick_to_bottom(recording)
                .show(ui, |ui| {
                    let edit = ui.add(
                        egui::TextEdit::multiline(&mut self.text)
                            .desired_width(f32::INFINITY)
                            .desired_rows(2)
                            .font(egui::FontId::proportional(size))
                            .frame(false)
                            .interactive(idle)
                            .hint_text("Your next thought starts here.")
                            .layouter(&mut layout),
                    );
                    if edit.changed() {
                        self.edited_at = Some(Instant::now());
                        self.history_dictation_changed();
                    }
                });
            ui.add_space(18.0);
            ui.horizontal_wrapped(|ui| {
                if let Some(trigger) = &self.macro_used {
                    ui.label(
                        RichText::new(format!("Expanded: bang {trigger}"))
                            .color(ACCENT)
                            .size(13.0),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            !self.text.is_empty(),
                            egui::Button::image(Icon::Copy.image(23.0, MUTED)).frame(false),
                        )
                        .on_hover_text("Copy text")
                        .clicked()
                    {
                        self.status = match platform::copy(&self.text) {
                            Ok(()) => "Copied to clipboard".into(),
                            Err(e) => e.to_string(),
                        };
                    }
                    ui.add_enabled_ui(!self.raw.is_empty(), |ui| {
                        ui.menu_button(RichText::new("Original").color(MUTED), |ui| {
                            ui.set_max_width(560.0);
                            ui.label(&self.raw);
                            if ui
                                .add_enabled(
                                    idle,
                                    egui::Button::image_and_text(
                                        Icon::Undo.image(17.0, MUTED),
                                        "Undo text corrections",
                                    ),
                                )
                                .clicked()
                            {
                                self.text = self.raw.clone();
                                self.history_dictation_changed();
                                self.words_changed = 0;
                                self.cleanup_count = 0;
                                self.edit_baseline.clone_from(&self.text);
                                self.edited_at = None;
                                self.macro_used = None;
                                ui.close();
                            }
                        });
                    });
                    if ui
                        .add_enabled(
                            !self.text.is_empty() && idle,
                            egui::Button::image(Icon::Close.image(18.0, MUTED)).frame(false),
                        )
                        .on_hover_text("Clear transcript")
                        .clicked()
                    {
                        self.history_save_dictation();
                        self.history.dictation = None;
                        self.text.clear();
                        self.raw.clear();
                        self.edit_baseline.clear();
                        self.edited_at = None;
                        self.macro_used = None;
                        self.elapsed = None;
                        self.words_changed = 0;
                        self.cleanup_count = 0;
                        self.last_seconds = 0;
                    }
                });
            });
            self.learning_notice(ui);
            if !self.ready && !self.loading && ui.button("Set up your speech model").clicked() {
                self.page = 2;
            }
            if let Some(error) = &self.shortcut_error {
                ui.colored_label(Color32::LIGHT_YELLOW, error);
            }
            // Keep error and action feedback visible without displacing the primary status.
            if !recording
                && !self.loading
                && !matches!(
                    self.status.as_str(),
                    "Ready when you are" | "Your transcript is ready"
                )
            {
                ui.label(RichText::new(&self.status).size(13.0).color(MUTED));
            }
        });
    }

    fn recording_dock(&mut self, ui: &mut egui::Ui) {
        let recording = self.recording.is_some();
        let enabled = self.ready
            && !self.busy
            && !self.loading
            && self.downloading.is_none()
            && self.call.is_none();
        let unavailable = if self.call.is_some() {
            "Finish your call recording before starting dictation."
        } else if self.downloading.is_some() {
            "Your speech model is downloading."
        } else if self.loading {
            "Your speech model is getting ready."
        } else if self.busy {
            "Finishing your transcript. You can cancel with the close button."
        } else {
            "Choose a speech model in Settings to start dictating."
        };
        let action_hint = if recording {
            format!(
                "Finish dictation. You can also press {}.",
                self.settings.hotkey.label()
            )
        } else if self.settings.insert {
            format!(
                "To type in another app, click its text field and press {}.",
                self.settings.hotkey.label()
            )
        } else {
            format!(
                "Start dictation. You can also press {}.",
                self.settings.hotkey.label()
            )
        };
        if self.meter_at.elapsed() >= Duration::from_millis(65) {
            self.meter.rotate_left(1);
            self.meter[31] = self
                .recording
                .as_ref()
                .map(|r| (f32::from_bits(r.level.load(Ordering::Relaxed)) * 7.0).clamp(0.0, 1.0))
                .unwrap_or(0.0);
            self.meter_at = Instant::now();
        }
        let wide = ui.available_width() > 760.0;
        egui::Frame::new()
            .fill(Color32::from_rgba_unmultiplied(25, 34, 35, 225))
            .stroke(egui::Stroke::new(1.0_f32, LINE))
            .corner_radius(32)
            .inner_margin(egui::Margin::symmetric(24, 18))
            .shadow(egui::Shadow {
                offset: [0, 8],
                blur: 36,
                spread: 0,
                color: Color32::from_black_alpha(35),
            })
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    let mic = ui
                        .add_enabled(
                            enabled,
                            egui::Button::image(Icon::Mic.image(28.0, ACCENT))
                                .corner_radius(30)
                                .fill(Color32::from_rgb(29, 52, 47))
                                .stroke(egui::Stroke::new(1.0_f32, ACCENT))
                                .min_size(egui::vec2(58.0, 58.0)),
                        )
                        .on_hover_text(&action_hint)
                        .on_disabled_hover_text(unavailable);
                    if mic.clicked() {
                        self.toggle(None);
                    }
                    let wave_w = if wide { 180.0 } else { 110.0 };
                    let (wave, _) =
                        ui.allocate_exact_size(egui::vec2(wave_w, 44.0), egui::Sense::hover());
                    let step = wave.width() / 32.0;
                    for (i, level) in self.meter.iter().enumerate() {
                        let height = 3.0 + level * 38.0;
                        let x = wave.left() + i as f32 * step;
                        ui.painter().rect_filled(
                            egui::Rect::from_center_size(
                                egui::pos2(x, wave.center().y),
                                egui::vec2(2.5, height),
                            ),
                            2.0,
                            if recording {
                                ACCENT.gamma_multiply(0.3 + 0.7 * i as f32 / 31.0)
                            } else {
                                LINE
                            },
                        );
                    }
                    let seconds = self
                        .recording
                        .as_ref()
                        .map(|r| r.seconds() as u64)
                        .unwrap_or(self.last_seconds);
                    ui.label(
                        RichText::new(format!("{:02}:{:02}", seconds / 60, seconds % 60))
                            .size(17.0),
                    );
                    ui.add_space(if wide { 14.0 } else { 0.0 });
                    ui.separator();
                    let label = if recording {
                        "Finish dictation"
                    } else if self.busy {
                        "Finishing…"
                    } else if self.loading {
                        "Getting ready…"
                    } else {
                        "Start dictation"
                    };
                    if ui
                        .add_enabled(
                            enabled,
                            egui::Button::new(
                                RichText::new(label)
                                    .size(16.0)
                                    .color(Color32::from_rgb(12, 35, 28)),
                            )
                            .fill(ACCENT)
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(26)
                            .min_size(egui::vec2(170.0, 52.0)),
                        )
                        .on_hover_text(&action_hint)
                        .on_disabled_hover_text(unavailable)
                        .clicked()
                    {
                        self.toggle(None);
                    }
                    if self.busy {
                        if ui
                            .add(egui::Button::image(Icon::Close.image(18.0, MUTED)).frame(false))
                            .on_hover_text("Cancel transcription")
                            .clicked()
                        {
                            self.insertion_cancel.cancel();
                            self.integration_pending = None;
                            let _ = self.integration_tx.send(integration::Action::Cancel);
                            self.pending_final = None;
                            self.cancel.cancel();
                            self.utterance += 1;
                            self.busy = false;
                            self.target = None;
                            self.status = "Transcription cancelled".into();
                        }
                    } else if wide {
                        ui.add_space(8.0);
                        let shortcut = self.settings.hotkey.label();
                        let full = self.settings.hotkey.ctrl as usize
                            + self.settings.hotkey.alt as usize
                            + self.settings.hotkey.shift as usize
                            + self.settings.hotkey.win as usize
                            <= 2;
                        if full {
                            for part in shortcut.split(" + ") {
                                theme::keycap(ui, part);
                            }
                        } else {
                            ui.label(RichText::new(&shortcut).size(12.0).color(MUTED));
                        }
                    }
                });
            });
    }
}
