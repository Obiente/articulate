//! Typed renderer views, grouped by area.
//!
//! `collect` is the section registry. Each section is a small serde struct
//! whose fields are top-level renderer state keys; a key belongs to exactly
//! one section. See docs/desktop-ui-contract.md.

use super::publish::{Cadence, Collector};
use crate::app::App;

pub(super) mod capture;
pub(super) mod discord;
pub(super) mod library;
pub(super) mod models;
pub(super) mod rows;
pub(super) mod settings;
pub(super) mod shell;
pub(super) mod updates;

pub(crate) use shell::{ControllerHealth, Overlay};

/// Offer every App-derived section to `out`, in dependency order.
///
/// To add a section: write a view function in the module for its area, then
/// add one line here. Choose `Background` for large or rarely changing values.
pub(super) fn collect(app: &App, out: &mut Collector<'_>) {
    use Cadence::{Background, Live};
    out.section("shell", Live, || shell::shell(app));
    out.section("capture", Live, || capture::capture(app));
    out.section("dictation", Live, || capture::dictation(app));
    out.section("live", Live, || capture::live_sessions(app));
    capture::live_rows(app, out);
    // A newly opened or replaced document needs its body in the same update.
    if out.section("document", Live, || library::document(app)) {
        out.include_background();
    }
    out.section("notes", Live, || models::notes(app));
    out.section("settings", Live, || settings::settings(app));
    out.section("models", Live, || models::models(app));
    out.section("discord", Live, || discord::discord(app));
    out.section("updater", Live, || updates::updater(app));
    out.section("overlay", Live, || shell::overlay_section(app));
    out.section("history", Background, || library::history(app));
    out.section("document_body", Background, || library::document_body(app));
    out.section("vocabulary", Background, || settings::vocabulary(app));
    out.section("insights", Background, || library::insights(app));
}

/// The dictation indicator state, for the native indicator window.
pub(super) fn overlay(app: &App) -> Overlay {
    shell::overlay(app)
}
