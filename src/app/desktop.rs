//! Narrow local renderer bridge. The native controller owns all capture and
//! persistence; the renderer receives typed state sections and sends feature
//! actions.
//!
//! - `bridge`: the controller thread, panic isolation and the close handshake.
//! - `publish`: section revisions, change detection and row splices.
//! - `view`: typed section views and the section registry.
//! - `actions`: feature action enums and their handlers.
//! - `lifecycle`: shutdown, download guards and document edit locks.
//!
//! See docs/desktop-ui-contract.md and docs/desktop-ui.md.

pub(super) mod actions;
mod bridge;
mod lifecycle;
mod publish;
mod view;

#[cfg(test)]
mod tests;

pub(crate) use actions::Action;
pub(crate) use bridge::{Bridge, CLOSE_ACK_TIMEOUT, CloseStep, Listeners};
pub(crate) use publish::StateUpdate;
pub(crate) use view::{ControllerHealth, Overlay};

impl super::App {
    /// Every App-derived section merged into one object, as the renderer
    /// sees it after a complete update. Used by tests.
    #[cfg(test)]
    pub(crate) fn desktop_snapshot(&self) -> serde_json::Value {
        let mut merged = serde_json::Map::new();
        for (_, section) in self.desktop_sections() {
            if let serde_json::Value::Object(values) = section {
                merged.extend(values);
            }
        }
        serde_json::Value::Object(merged)
    }

    /// Every App-derived section, freshly built, in registry order.
    #[cfg(test)]
    pub(crate) fn desktop_sections(&self) -> Vec<(&'static str, serde_json::Value)> {
        let mut collector = publish::Collector::snapshot();
        view::collect(self, &mut collector);
        collector.into_sections()
    }
}
