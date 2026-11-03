//! Application updates.

use crate::app::App;
use crate::update::Status;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[allow(
    clippy::enum_variant_names,
    reason = "Variant names are the renderer's action types"
)]
pub(crate) enum Action {
    UpdateCheck,
    UpdateDownload,
    /// Saves everything, then launches the verified installer. The IPC
    /// handler exits the app after this succeeds.
    UpdateInstall,
}

impl App {
    pub(crate) fn desktop_updates_action(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::UpdateCheck => self.updater.check(),
            Action::UpdateDownload => {
                let Status::Available(release) = &self.updater.status else {
                    return Err("Check for an available update first.".into());
                };
                self.updater.download(release.clone());
            }
            Action::UpdateInstall => {
                let Status::Ready(installer) = &self.updater.status else {
                    return Err("Download the update before installing it.".into());
                };
                let installer = installer.clone();
                // Refuses while capture, processing or any download is active.
                self.desktop_shutdown()?;
                self.pending_install = Some(installer.clone());
                if let Err(error) = installer.launch() {
                    self.pending_install = None;
                    return Err(error.to_string());
                }
            }
        }
        Ok(())
    }
}
