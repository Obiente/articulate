use super::*;
pub(super) const INK: Color32 = Color32::from_rgb(241, 239, 230);
pub(super) const MUTED: Color32 = Color32::from_rgb(165, 178, 182);
pub(super) const SURFACE: Color32 = Color32::from_rgb(24, 33, 35);
pub(super) const LINE: Color32 = Color32::from_rgb(48, 63, 65);

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Mic,
    Phone,
    Book,
    Bolt,
    Gear,
    Copy,
    Undo,
    Bulb,
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
            Self::Bulb => egui::include_image!("../../assets/icons/lightbulb.svg"),
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
                Self::Bulb => "Shortcuts",
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
    style.spacing.item_spacing = egui::vec2(12.0, 14.0);
    style.spacing.button_padding = egui::vec2(18.0, 12.0);
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
