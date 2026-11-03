//! Discord connection, participants and companion status.
//!
//! The Discord module builds this view (`App::desktop_discord_status`); the
//! pairing key is never part of it.

use crate::app::App;
use serde::Serialize;
use std::borrow::Cow;

/// One member of the current voice channel. `avatar` is a
/// `bytes://discord-avatar/...` identifier the renderer resolves through
/// `desktop_discord_avatar`; it is absent while pictures are turned off.
#[derive(Serialize)]
pub(crate) struct Participant {
    pub id: String,
    pub name: String,
    pub speaking: bool,
    pub is_self: bool,
    pub avatar: Option<String>,
}

/// Companion installation and build state.
#[derive(Serialize)]
pub(crate) struct Companion<'a> {
    pub checked: bool,
    pub installed: bool,
    pub runtime_status: &'static str,
    pub source_found: bool,
    pub active: bool,
    pub busy: bool,
    pub status: &'a str,
    pub error: Option<&'a str>,
    pub plugin_status: &'static str,
    pub can_build: bool,
    pub native_audio: bool,
    pub installer_ready: bool,
}

#[derive(Serialize)]
pub(crate) struct DiscordView<'a> {
    pub connected: bool,
    pub listener_started: bool,
    pub status: &'static str,
    pub in_voice: bool,
    pub audio_ready: bool,
    pub audio_status: Option<String>,
    pub audio_message: Option<&'static str>,
    pub participants: Vec<Participant>,
    pub pairing_state: &'static str,
    pub pairing_key_present: bool,
    /// `current`, `outdated`, `newer` or `unknown` before a companion reports.
    pub companion_protocol: Cow<'a, str>,
    pub companion_protocol_message: Option<&'static str>,
    /// The chosen edition id, or empty for automatic selection.
    pub flavor: &'a str,
    pub installed_flavors: Vec<&'static str>,
    pub debugger_port: u16,
    pub debugger_open: bool,
    pub debugger_notice: Option<&'static str>,
    pub automatic_can_retry: bool,
    pub error: Option<Cow<'a, str>>,
    pub relaunching: bool,
    pub companion_selected: bool,
    pub automatic_status: &'a str,
    pub companion: Companion<'a>,
}

#[derive(Serialize)]
pub(crate) struct Discord<'a> {
    discord: DiscordView<'a>,
}

pub(crate) fn discord(app: &App) -> Discord<'_> {
    Discord {
        discord: app.desktop_discord_status(),
    }
}
