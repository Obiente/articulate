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
                let max_width = if self.page == 0 {
                    980.0
                } else if self.page == 3 {
                    1380.0
                } else {
                    1120.0
                };
                let inset = padding.max((rect.width() - max_width) * 0.5);
                let content = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + inset, rect.top() + 108.0),
                    egui::pos2(rect.right() - inset, footer.top() - 12.0),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
                    match self.page {
                        0 => self.dictation_surface(ui),
                        3 => self.calls_ui(ui, ctx),
                        5 => self.history_ui(ui),
                        4 => self.macros_ui(ui),
                        1 => self.vocabulary_surface(ui),
                        _ => {
                            egui::ScrollArea::vertical()
                                .id_salt(("workspace", self.page))
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    if self.page == 2 {
                                        self.settings_ui(ui, ctx);
                                    }
                                });
                        }
                    }
                });
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
        let compact = ui.available_height() < 580.0;
        let bounds = ui.available_rect_before_wrap();
        egui::ScrollArea::vertical()
            .id_salt("dictation_page")
            .max_height((bounds.height() - 64.0).max(200.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = if compact { 4.0 } else { 7.0 };
                    ui.label(RichText::new("Speak naturally.").size(if compact {
                        28.0
                    } else {
                        38.0
                    }));
                    ui.label(
                        RichText::new("Articulate takes care of the rest.")
                            .size(18.0)
                            .color(MUTED),
                    );
                    ui.add_space(if compact { 8.0 } else { 10.0 });
                    if compact {
                        ui.allocate_ui_with_layout(
                            egui::vec2(430.0, 84.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| self.recording_hero(ui, true),
                        );
                    } else {
                        self.recording_hero(ui, false);
                    }
                });
                ui.add_space(if compact { 12.0 } else { 24.0 });
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(20, 29, 31, 170))
                    .stroke(egui::Stroke::new(1.0_f32, LINE))
                    .corner_radius(14)
                    .inner_margin(if compact { 18.0 } else { 22.0 })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;
                        ui.set_min_width(ui.available_width());
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
                            let (dot, _) =
                                ui.allocate_exact_size(egui::vec2(9.0, 20.0), egui::Sense::hover());
                            ui.painter().circle_filled(
                                dot.center(),
                                4.0,
                                if self.ready { ACCENT } else { MUTED },
                            );
                            ui.label(
                                RichText::new(if recording || self.busy || self.loading {
                                    state.as_str()
                                } else {
                                    "LATEST DICTATION"
                                })
                                .size(13.0)
                                .color(ACCENT),
                            );
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
                        ui.add_space(8.0);
                        let size = if compact { 22.0 } else { 26.0 };
                        let highlight = self.remembered.last().map(|c| c.entry.wanted.clone());
                        let mut layout =
                            |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
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
                            .max_height(if compact { 165.0 } else { 250.0 })
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
                        ui.add_space(8.0);
                        ui.horizontal_wrapped(|ui| {
                            if let Some(trigger) = &self.macro_used {
                                ui.label(
                                    RichText::new(format!("Expanded: bang {trigger}"))
                                        .color(ACCENT)
                                        .size(13.0),
                                );
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_enabled(
                                            !self.text.is_empty(),
                                            egui::Button::image_and_text(
                                                Icon::Copy
                                                    .image(20.0, Color32::from_rgb(13, 34, 30)),
                                                RichText::new("Copy text")
                                                    .color(Color32::from_rgb(13, 34, 30)),
                                            )
                                            .fill(ACCENT),
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
                                        ui.menu_button(
                                            RichText::new("Original").color(MUTED),
                                            |ui| {
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
                                            },
                                        );
                                    });
                                    if ui
                                        .add_enabled(
                                            !self.text.is_empty() && idle,
                                            egui::Button::image(Icon::Close.image(18.0, MUTED))
                                                .frame(false),
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
                                },
                            );
                        });
                    });
                if idle && !self.preview_inflight && !self.text.is_empty() {
                    self.assort_correction_ui(ui);
                }
                self.learning_notice(ui);
                if !self.ready && !self.loading && ui.button("Set up your speech model").clicked() {
                    self.page = 2;
                }
                if let Some(error) = &self.shortcut_error {
                    ui.colored_label(Color32::LIGHT_YELLOW, error);
                }
                // Keep error and action feedback visible without displacing the primary status.
                if self.recording.is_none()
                    && !self.loading
                    && !matches!(
                        self.status.as_str(),
                        "Ready when you are" | "Your transcript is ready"
                    )
                {
                    ui.label(RichText::new(&self.status).size(13.0).color(MUTED));
                }
            });
        let footer = egui::Rect::from_min_max(
            egui::pos2(bounds.left(), bounds.bottom() - 56.0),
            bounds.max,
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.separator();
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width() * 0.70, 40.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.label("Ready to type into your current app");
                        ui.label(
                            RichText::new(format!(
                                "Focus a text field, then press {}.",
                                self.settings.hotkey.label()
                            ))
                            .small()
                            .color(MUTED),
                        );
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut live = self.settings.insert && self.settings.live_insert;
                    if ui
                        .add_enabled(idle, egui::Checkbox::new(&mut live, "Live insertion"))
                        .changed()
                    {
                        self.settings.insert = live;
                        self.settings.live_insert = live;
                        self.save();
                    }
                });
            });
        });
    }

    fn recording_hero(&mut self, ui: &mut egui::Ui, compact: bool) {
        let recording = self.recording.is_some();
        let enabled = self.ready
            && !self.busy
            && !self.loading
            && self.downloading.is_none()
            && self.call.is_none();
        if self.meter_at.elapsed() >= Duration::from_millis(65) {
            self.meter.rotate_left(1);
            self.meter[31] = self
                .recording
                .as_ref()
                .map(|r| f32::from_bits(r.level.load(Ordering::Relaxed)))
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            self.meter_at = Instant::now();
        }
        let diameter = if compact { 64.0 } else { 110.0 };
        let (orb, response) = ui.allocate_exact_size(
            egui::vec2(diameter + 20.0, diameter + 20.0),
            egui::Sense::click(),
        );
        let center = orb.center();
        let radius = diameter * 0.5;
        let level = self
            .recording
            .as_ref()
            .map(|r| f32::from_bits(r.level.load(Ordering::Relaxed)))
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        for ring in (0..4).rev() {
            ui.painter().circle_stroke(
                center,
                radius + ring as f32 * 4.0 + level * 10.0,
                egui::Stroke::new(
                    if ring == 0 { 2.0_f32 } else { 1.0_f32 },
                    ACCENT.gamma_multiply(if ring == 0 { 0.9 } else { 0.12 / ring as f32 }),
                ),
            );
        }
        // Layered native geometry keeps the microphone glow sharp at every DPI.
        ui.painter().circle_filled(center, radius - 2.0, SURFACE);
        Icon::Mic
            .image(if compact { 28.0 } else { 38.0 }, INK)
            .paint_at(
                ui,
                egui::Rect::from_center_size(center, egui::vec2(42.0, 42.0)),
            );
        if response.clicked() && enabled {
            self.toggle(None);
        }
        let label = if recording {
            "Finish dictation"
        } else if self.busy {
            "Finishing…"
        } else if self.loading {
            "Getting ready…"
        } else {
            "Start dictating"
        };
        if ui
            .add_enabled(
                enabled,
                egui::Button::new(RichText::new(label).color(Color32::from_rgb(13, 34, 30)))
                    .fill(ACCENT)
                    .min_size(egui::vec2(180.0, 42.0)),
            )
            .on_disabled_hover_text(
                "Prepare your speech model in Settings, or finish the current capture.",
            )
            .clicked()
        {
            self.toggle(None);
        }
        if self.busy && ui.button("Cancel transcription").clicked() {
            self.cancel.cancel();
        }
        if compact {
            ui.label(
                RichText::new(self.settings.hotkey.label())
                    .small()
                    .color(MUTED),
            );
        } else {
            ui.allocate_ui_with_layout(
                egui::vec2(190.0, 32.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    theme::keycap(ui, &self.settings.hotkey.label());
                },
            );
        }
        if recording {
            let seconds = self
                .recording
                .as_ref()
                .map(|r| r.seconds() as u64)
                .unwrap_or(0);
            ui.label(
                RichText::new(format!(
                    "Listening · {:02}:{:02}",
                    seconds / 60,
                    seconds % 60
                ))
                .color(ACCENT),
            );
        }
    }
}
