use crate::update::{InstallRequest, State, Status};
use eframe::egui::{self, RichText};

/// Returns an explicitly requested installer. The caller must save pending state,
/// call `launch`, then close Articulate only if launching succeeded.
pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut State,
    can_install: bool,
) -> Option<InstallRequest> {
    state.poll();
    let mut install = None;
    let mut check = false;
    let mut download = None;
    egui::Frame::new()
        .fill(super::theme::SURFACE)
        .stroke(egui::Stroke::new(1.0_f32, super::theme::LINE))
        .corner_radius(18)
        .inner_margin(18.0)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("App updates").strong().size(18.0));
                ui.label(RichText::new(format!("Articulate {}", env!("CARGO_PKG_VERSION"))).color(super::theme::MUTED));
            });
            match &state.status {
                Status::Idle => { ui.label("Check for a new version when you're ready."); }
                Status::Checking => { ui.horizontal(|ui| { ui.spinner(); ui.label("Checking for updates…"); }); }
                Status::Latest => { ui.label("You're using the latest version."); }
                Status::Available(release) => {
                    ui.label(format!("Articulate {} is available. {:.0} MB download.", release.version, release.size as f64 / 1_048_576.0));
                    if ui.button("Download update").clicked() { download = Some(release.clone()); }
                }
                Status::Downloading { version, progress } => {
                    ui.label(format!("Downloading Articulate {version}…"));
                    ui.add(egui::ProgressBar::new(*progress).show_percentage().animate(true));
                    ui.small("You can keep using Articulate while it downloads.");
                }
                Status::Ready(request) => {
                    ui.label(format!("Articulate {} is ready to install.", request.version));
                    ui.small("Articulate will close and open the installer. Your models, vocabulary and history stay on this device.");
                    if ui.add_enabled(can_install, egui::Button::new("Close and install update")).clicked() { install = Some(request.clone()); }
                    if !can_install { ui.small("Finish your recording before installing the update."); }
                }
                Status::Error(error) => { ui.label(RichText::new(error).color(egui::Color32::from_rgb(245, 179, 158))); }
            }
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!state.busy(), egui::Button::new(if matches!(state.status, Status::Error(_)) { "Retry update check" } else { "Check for updates" })).clicked() { check = true; }
                ui.hyperlink_to("Release notes", crate::update::RELEASES);
            });
            ui.small(RichText::new("Updates download from GitHub only when you choose. Your recordings and text are never sent.").color(super::theme::MUTED));
        });
    if check {
        state.check();
    } else if let Some(release) = download {
        state.download(release);
    }
    if state.busy() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    install
}
