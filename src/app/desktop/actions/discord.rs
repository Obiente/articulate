//! Discord connection, companion setup, pairing and meeting detection.

use crate::app::App;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    DiscordConnect {
        companion: bool,
    },
    DiscordDisconnect,
    DiscordRetry,
    DiscordRelaunch,
    /// Restart the chosen edition without the debugger an earlier relaunch opened.
    DiscordRelaunchNormally,
    /// Replace the companion pairing key; every companion must pair again.
    DiscordRotateKey,
    /// Remove the pairing key and stop listening until the next connect.
    DiscordForgetPairing,
    CompanionDetect,
    CompanionInstall,
    CompanionOpenInstaller,
    /// Answer the prompt for a detected meeting.
    MeetingRespond {
        accept: bool,
    },
}

impl App {
    pub(crate) fn desktop_discord_action(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::DiscordConnect { companion } => self.desktop_discord_connect(companion),
            Action::DiscordDisconnect => self.desktop_discord_disconnect(),
            Action::DiscordRetry => self.desktop_discord_retry(),
            Action::DiscordRelaunch => self.desktop_discord_relaunch(),
            Action::DiscordRelaunchNormally => self.desktop_discord_relaunch_normally(),
            Action::DiscordRotateKey => self.desktop_discord_rotate_key(),
            Action::DiscordForgetPairing => self.desktop_discord_forget_pairing(),
            Action::CompanionDetect => self.desktop_companion_action("detect"),
            Action::CompanionInstall => self.desktop_companion_action("install"),
            Action::CompanionOpenInstaller => self.desktop_companion_action("open_installer"),
            Action::MeetingRespond { accept } => self.desktop_meeting_respond(accept),
        }
    }
}
