use super::*;
use theme::{INK, Icon, LINE, MUTED, SURFACE};

impl App {
    pub(super) fn surface(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BASE))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                theme::background(ui.painter(), rect);
                let sidebar_width = if rect.width() < 1050.0 { 180.0 } else { 212.0 };
                let sidebar = egui::Rect::from_min_max(
                    rect.min + egui::vec2(12.0, 24.0),
                    egui::pos2(rect.left() + sidebar_width - 12.0, rect.bottom() - 24.0),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(sidebar), |ui| {
                    self.sidebar(ui)
                });
                let canvas = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + sidebar_width, rect.top() + 16.0),
                    rect.max - egui::vec2(16.0, 16.0),
                );
                ui.painter().rect_filled(canvas, 24, theme::CANVAS);
                ui.painter().rect_stroke(
                    canvas,
                    24,
                    egui::Stroke::new(1.0_f32, LINE.gamma_multiply(0.55)),
                    egui::StrokeKind::Inside,
                );
                let padding = if rect.width() < 1050.0 { 22.0 } else { 32.0 };
                let content = canvas.shrink(padding);
                ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
                    ui.set_clip_rect(content);
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

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let bounds = ui.max_rect();
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add(
                egui::Image::new(egui::include_image!(
                    "../../assets/brand/articulate-mark.svg"
                ))
                .fit_to_exact_size(egui::vec2(34.0, 28.0)),
            );
            ui.label(RichText::new("Articulate").size(21.0).color(INK));
        });
        ui.add_space(36.0);
        for (page, label, icon) in [
            (0, "Dictate", Icon::Mic),
            (3, "Calls", Icon::Phone),
            (1, "Vocabulary", Icon::Book),
            (4, "Shortcuts", Icon::Bolt),
            (5, "History", Icon::History),
        ] {
            self.navigation_row(ui, page, label, icon);
        }
        if self.history.error.is_some() && ui.small_button("History needs attention").clicked() {
            self.page = 5;
        }
        let bottom = egui::Rect::from_min_max(
            egui::pos2(bounds.left(), bounds.bottom() - 98.0),
            bounds.max,
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(bottom), |ui| {
            self.navigation_row(ui, 2, "Settings", Icon::Gear);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add(Icon::Lock.image(16.0, MUTED));
                ui.label(RichText::new("On this device").size(12.0).color(MUTED));
            });
            ui.label(
                RichText::new(format!("Articulate {}", env!("CARGO_PKG_VERSION")))
                    .size(11.0)
                    .color(MUTED),
            );
        });
    }

    fn navigation_row(&mut self, ui: &mut egui::Ui, page: usize, label: &str, icon: Icon) {
        let active = self.page == page;
        let tint = if active { ACCENT } else { MUTED };
        let response = ui.add_sized(
            [ui.available_width(), 44.0],
            egui::Button::new("")
                .fill(if active {
                    theme::SELECTED
                } else {
                    Color32::TRANSPARENT
                })
                .stroke(egui::Stroke::NONE)
                .corner_radius(9),
        );
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), active, label)
        });
        #[cfg(test)]
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new(("navigation_response", page)), response.id)
        });
        icon.image(20.0, tint).paint_at(
            ui,
            egui::Rect::from_center_size(
                egui::pos2(response.rect.left() + 24.0, response.rect.center().y),
                egui::vec2(20.0, 20.0),
            ),
        );
        ui.painter().text(
            egui::pos2(response.rect.left() + 44.0, response.rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(15.0),
            if active { INK } else { MUTED },
        );
        if active {
            ui.painter().rect_filled(
                egui::Rect::from_center_size(
                    egui::pos2(response.rect.left() + 2.0, response.rect.center().y),
                    egui::vec2(3.0, 20.0),
                ),
                2,
                ACCENT,
            );
        }
        if response.clicked() {
            self.page = page;
        }
    }

    fn dictation_surface(&mut self, ui: &mut egui::Ui) {
        let idle = self.recording.is_none() && !self.busy && !self.loading && self.call.is_none();
        let compact = ui.available_width() < 780.0;
        let bounds = ui.available_rect_before_wrap();
        egui::ScrollArea::vertical()
            .id_salt("dictation_page")
            .max_height((bounds.height() - 64.0).max(200.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                theme::page_title(ui, "Dictate");
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    self.recording_hero(ui, true);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Open history").clicked() {
                            self.page = 5;
                        }
                    });
                });
                ui.add_space(18.0);
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
                            if idle && !self.preview_inflight && !self.text.is_empty() {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        self.assort_correction_button(ui);
                                    },
                                );
                            }
                        });
                        if self.text.is_empty() && !recording && !self.busy && !self.loading {
                            ui.label(
                                RichText::new(if self.settings.insert {
                                    format!("Focus a text field. {}.", self.shortcut_instruction())
                                } else {
                                    format!(
                                        "Speak, review, then copy. {}.",
                                        self.shortcut_instruction()
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
                            .min_scrolled_height(180.0)
                            .max_height((bounds.height() - 278.0).max(180.0))
                            .auto_shrink([false, true])
                            .stick_to_bottom(recording)
                            .show(ui, |ui| {
                                let edit = ui.add(
                                    egui::TextEdit::multiline(&mut self.text)
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(5)
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
            let hold = self.settings.hotkey_mode == platform::HotkeyMode::Hold;
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.separator();
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width() * 0.70, 40.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.label(if !self.settings.insert {
                            "Review and copy your words"
                        } else if hold {
                            "Insert into your app when you finish"
                        } else {
                            "Ready to type into your current app"
                        });
                        ui.label(
                            RichText::new(format!("{}.", self.shortcut_instruction()))
                                .small()
                                .color(MUTED),
                        );
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if hold {
                        if ui
                            .add_enabled(
                                idle,
                                egui::Checkbox::new(&mut self.settings.insert, "Insert into app"),
                            )
                            .changed()
                        {
                            self.save();
                        }
                        return;
                    }
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
        let diameter = if compact { 36.0 } else { 110.0 };
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
            .image(if compact { 22.0 } else { 38.0 }, INK)
            .paint_at(
                ui,
                egui::Rect::from_center_size(
                    center,
                    egui::vec2(
                        if compact { 26.0 } else { 42.0 },
                        if compact { 26.0 } else { 42.0 },
                    ),
                ),
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
            let binding = ui.label(
                RichText::new(self.settings.hotkey.label())
                    .small()
                    .color(MUTED),
            );
            if self.settings.hotkey_mode == platform::HotkeyMode::Hold {
                binding.on_hover_text("Hold to speak. Double-press for hands-free dictation, then press once to finish.");
            }
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
