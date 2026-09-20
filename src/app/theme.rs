use super::*;
pub(super) const INK: Color32 = Color32::from_rgb(241, 239, 230);
pub(super) const MUTED: Color32 = Color32::from_rgb(165, 178, 182);
pub(super) const SURFACE: Color32 = Color32::from_rgb(24, 33, 35);
pub(super) const LINE: Color32 = Color32::from_rgb(48, 63, 65);
pub(super) const BASE: Color32 = Color32::from_rgb(15, 21, 23);
pub(super) const CANVAS: Color32 = Color32::from_rgb(18, 26, 28);
pub(super) const SELECTED: Color32 = Color32::from_rgb(32, 56, 51);

pub(super) fn page_title(ui: &mut egui::Ui, title: &str) -> egui::Response {
    ui.label(RichText::new(title).size(28.0).strong().color(INK))
}

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Mic,
    Phone,
    Book,
    Bolt,
    Gear,
    Copy,
    Undo,
    Lock,
    Close,
    History,
}
impl Icon {
    pub fn image(self, size: f32, tint: Color32) -> egui::Image<'static> {
        let source = match self {
            Self::Mic => egui::include_image!("../../assets/icons/microphone.svg"),
            Self::Phone => egui::include_image!("../../assets/icons/phone.svg"),
            Self::Book => egui::include_image!("../../assets/icons/book-open.svg"),
            Self::Bolt => egui::include_image!("../../assets/icons/lightning.svg"),
            Self::Gear => egui::include_image!("../../assets/icons/gear.svg"),
            Self::Copy => egui::include_image!("../../assets/icons/copy.svg"),
            Self::Undo => egui::include_image!("../../assets/icons/arrow-counter-clockwise.svg"),
            Self::Lock => egui::include_image!("../../assets/icons/lock.svg"),
            Self::Close => egui::include_image!("../../assets/icons/x.svg"),
            Self::History => egui::ImageSource::Bytes { uri: "bytes://articulate-history.svg".into(), bytes: br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3 11a9 9 0 1 1 2.5 7M3 5v6h6M12 7v5l3 2"/></svg>"#.as_slice().into() },
        };
        egui::Image::new(source)
            .fit_to_exact_size(egui::vec2(size, size))
            .tint(tint)
            .alt_text(match self {
                Self::Mic => "Dictation",
                Self::Phone => "Calls",
                Self::Book => "Vocabulary",
                Self::Bolt => "Shortcuts",
                Self::Gear => "Settings",
                Self::Copy => "Copy text",
                Self::Undo => "Undo",
                Self::Lock => "Local",
                Self::Close => "Close",
                Self::History => "History",
            })
    }
}

pub(super) fn configure(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
    ctx.set_fonts(crate::fonts::definitions());
    ctx.set_visuals(egui::Visuals::dark());
    let mut style = (*ctx.style()).clone();
    style.animation_time = 0.18;
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 8.0);
    style.spacing.interact_size.y = 38.0;
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(16.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(16.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(13.0));
    style.visuals.override_text_color = Some(INK);
    style.visuals.panel_fill = Color32::TRANSPARENT;
    style.visuals.window_fill = SURFACE;
    style.visuals.extreme_bg_color = Color32::from_rgb(17, 25, 27);
    style.visuals.faint_bg_color = SURFACE;
    style.visuals.selection.bg_fill = Color32::from_rgb(42, 79, 70);
    style.visuals.selection.stroke = egui::Stroke::new(1.0_f32, ACCENT);
    for widget in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.noninteractive,
    ] {
        widget.corner_radius = egui::CornerRadius::same(10);
        widget.bg_stroke = egui::Stroke::new(1.0_f32, LINE);
        widget.fg_stroke.color = MUTED;
    }
    style.visuals.widgets.inactive.bg_fill = SURFACE;
    style.visuals.widgets.inactive.weak_bg_fill = SURFACE;
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(34, 51, 49);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(34, 51, 49);
    style.visuals.widgets.hovered.fg_stroke.color = INK;
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(40, 73, 65);
    style.visuals.widgets.active.weak_bg_fill = Color32::from_rgb(40, 73, 65);
    style.visuals.widgets.active.fg_stroke.color = ACCENT;
    style.visuals.widgets.noninteractive.bg_fill = SURFACE;
    ctx.set_style(style);
}

/// Native vertex-color gradients. No bitmap is sampled or stretched.
pub(super) fn background(painter: &egui::Painter, rect: egui::Rect) {
    let mut mesh = egui::Mesh::default();
    const X: usize = 28;
    const Y: usize = 20;
    for y in 0..=Y {
        for x in 0..=X {
            let u = x as f32 / X as f32;
            let v = y as f32 / Y as f32;
            let bloom = (-20.0 * ((u - 0.5).powi(2) * 1.5 + (v - 1.05).powi(2))).exp();
            let wash = (-5.0 * (u * u + v * v)).exp();
            let color = Color32::from_rgb(
                (15.0 + 3.0 * wash + 3.0 * bloom) as u8,
                (21.0 + 4.0 * wash + 20.0 * bloom) as u8,
                (23.0 + 3.0 * wash + 14.0 * bloom) as u8,
            );
            mesh.colored_vertex(
                egui::pos2(
                    rect.left() + u * rect.width(),
                    rect.top() + v * rect.height(),
                ),
                color,
            );
        }
    }
    for y in 0..Y {
        for x in 0..X {
            let a = (y * (X + 1) + x) as u32;
            let b = a + 1;
            let c = a + (X + 1) as u32;
            let d = c + 1;
            mesh.add_triangle(a, b, c);
            mesh.add_triangle(b, d, c);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

pub(super) fn keycap(ui: &mut egui::Ui, label: &str) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0_f32, LINE))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(10, 9))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(13.0));
        });
}

/// A shared reading rhythm for live calls and saved transcripts.
pub(super) fn transcript_row(
    ui: &mut egui::Ui,
    row: &calls::Row,
    names: &[String; 4],
    avatars: &crate::discord::avatar::Cache,
) {
    ui.push_id((row.start_ms, row.microphone, &row.text), |ui| {
        ui.horizontal(|ui| {
            if let Some(named) = row
                .discord
                .as_ref()
                .filter(|named| named.speakers.len() == 1)
                .and_then(|named| named.speakers.first())
            {
                avatar(ui, &named.name, named.avatar.as_ref(), avatars, 26.0);
            }
            ui.label(
                RichText::new(calls::label(row, names))
                    .size(17.0)
                    .color(ACCENT),
            );
            ui.add_space(10.0);
            ui.label(
                RichText::new(format!(
                    "{:02}:{:02}",
                    row.start_ms / 60_000,
                    row.start_ms / 1000 % 60
                ))
                .small()
                .color(MUTED),
            );
        });
        ui.add_space(2.0);
        let mut job = egui::text::LayoutJob::default();
        job.append(
            &row.text,
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(20.0),
                color: INK,
                line_height: Some(30.0),
                ..Default::default()
            },
        );
        ui.add(egui::Label::new(job).wrap().selectable(true));
    });
}

pub(super) fn people(
    ui: &mut egui::Ui,
    rows: &[calls::Row],
    names: &[String; 4],
    avatars: &crate::discord::avatar::Cache,
) {
    ui.label(RichText::new("In this conversation").size(18.0));
    ui.add_space(14.0);
    let mut people: Vec<(String, String, Option<crate::discord::avatar::Avatar>)> = Vec::new();
    for row in rows {
        if let Some(named) = &row.discord {
            for speaker in &named.speakers {
                let key = speaker
                    .avatar
                    .as_ref()
                    .map(|avatar| avatar.user_id.clone())
                    .unwrap_or_else(|| {
                        if speaker.id.is_empty() {
                            speaker.name.clone()
                        } else {
                            speaker.id.clone()
                        }
                    });
                if !people.iter().any(|person| person.0 == key) {
                    people.push((key, speaker.name.clone(), speaker.avatar.clone()));
                }
            }
        } else {
            let label = calls::label(row, names);
            if !people.iter().any(|person| person.0 == label) {
                people.push((label.clone(), label, None));
            }
        }
    }
    for (_, label, picture) in people.iter().take(32) {
        ui.horizontal_wrapped(|ui| {
            avatar(ui, label, picture.as_ref(), avatars, 34.0);
            ui.label(label);
        });
        ui.add_space(8.0);
    }
    ui.separator();
    ui.label(
        RichText::new("Transcripts stay on this device.")
            .small()
            .color(MUTED),
    );
}

fn avatar(
    ui: &mut egui::Ui,
    label: &str,
    picture: Option<&crate::discord::avatar::Avatar>,
    cache: &crate::discord::avatar::Cache,
    size: f32,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    if let Some(picture) = picture {
        cache.request(picture);
        if let Some(bytes) = cache.bytes(picture) {
            egui::Image::from_bytes(picture.uri(), bytes)
                .fit_to_exact_size(rect.size())
                .corner_radius(egui::CornerRadius::same((size * 0.5) as u8))
                .paint_at(ui, rect);
            return;
        }
    }
    ui.painter()
        .circle_filled(rect.center(), size * 0.5, Color32::from_rgb(32, 58, 52));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label.chars().next().unwrap_or('?'),
        egui::FontId::proportional(size * 0.47),
        INK,
    );
}

pub(super) fn preference_switch(
    ui: &mut egui::Ui,
    value: &mut bool,
    title: &str,
    description: &str,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let text_width = (ui.available_width() - 80.0).max(180.0);
        ui.allocate_ui_with_layout(
            egui::vec2(text_width, 46.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.label(RichText::new(title).size(16.0));
                ui.label(RichText::new(description).small().color(MUTED));
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (rect, mut response) =
                ui.allocate_exact_size(egui::vec2(44.0, 24.0), egui::Sense::click());
            if response.clicked() {
                *value = !*value;
                response.mark_changed();
                changed = true;
            }
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Checkbox,
                    ui.is_enabled(),
                    *value,
                    title,
                )
            });
            let t = ui.ctx().animate_bool_with_time(response.id, *value, 0.14);
            let fill = if *value { ACCENT } else { LINE };
            ui.painter().rect_filled(
                rect,
                12.0,
                if ui.is_enabled() {
                    fill
                } else {
                    fill.gamma_multiply(0.4)
                },
            );
            let center = egui::pos2(rect.left() + 12.0 + t * 20.0, rect.center().y);
            ui.painter().circle_filled(center, 9.0, INK);
            if response.has_focus() {
                ui.painter().rect_stroke(
                    rect.expand(3.0),
                    14.0,
                    egui::Stroke::new(1.0_f32, ACCENT),
                    egui::StrokeKind::Outside,
                );
            }
        });
    });
    ui.separator();
    changed
}
