//! Discord connection and companion setup.

use crate::app::App;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    DiscordConnect { companion: bool },
    DiscordDisconnect,
    DiscordRetry,
    DiscordRelaunch,
    CompanionDetect,
    CompanionInstall,
    CompanionOpenInstaller,
}

impl App {
    pub(crate) fn desktop_discord_action(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::DiscordConnect { companion } => self.desktop_discord_connect(companion),
            Action::DiscordDisconnect => self.desktop_discord_disconnect(),
            Action::DiscordRetry => self.desktop_discord_retry(),
            Action::DiscordRelaunch => self.desktop_discord_relaunch(),
            Action::CompanionDetect => self.desktop_companion_action("detect"),
            Action::CompanionInstall => self.desktop_companion_action("install"),
            Action::CompanionOpenInstaller => self.desktop_companion_action("open_installer"),
        }
    }
}
