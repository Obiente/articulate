//! Preferences, devices and the vocabulary and snippet lists.

use crate::app::App;
use crate::{dictionary::Entry, macros::VoiceMacro, meetings, platform};
use serde::Serialize;

/// Renderer-visible preferences. Private values such as the Discord pairing
/// key and the model path are never included.
#[derive(Serialize)]
pub(crate) struct SettingsView<'a> {
    clean_speech: bool,
    insert: bool,
    live_insert: bool,
    cpu: bool,
    transcription_language: &'a str,
    audio_context: bool,
    learn_corrections: bool,
    audio_feedback: bool,
    microphone: Option<&'a str>,
    output: Option<&'a str>,
    hotkey: &'a platform::Hotkey,
    quick_note_hotkey: &'a platform::Hotkey,
    quick_note_hotkey_pending: bool,
    quick_note_shortcut_error: Option<&'a str>,
    hotkey_mode: platform::HotkeyMode,
    discord_auto_connect: bool,
    discord_auto_transcribe: bool,
    discord_companion: bool,
    vencord_auto_update: bool,
    discord_flavor: &'a str,
    discord_avatars: bool,
    meetings_teams: bool,
    meetings_zoom: bool,
    meetings_meet: bool,
    meetings_start: meetings::StartMode,
}

#[derive(Serialize)]
pub(crate) struct Settings<'a> {
    settings: SettingsView<'a>,
    microphones: &'a [String],
    outputs: &'a [String],
}

pub(crate) fn settings(app: &App) -> Settings<'_> {
    let settings = &app.settings;
    Settings {
        settings: SettingsView {
            clean_speech: settings.clean_speech,
            insert: settings.insert,
            live_insert: settings.live_insert,
            cpu: settings.cpu,
            transcription_language: &settings.transcription_language,
            audio_context: settings.audio_context,
            learn_corrections: settings.learn_corrections,
            audio_feedback: settings.audio_feedback,
            microphone: settings.microphone.as_deref(),
            output: settings.output.as_deref(),
            hotkey: &settings.hotkey,
            quick_note_hotkey: &settings.quick_note_hotkey,
            quick_note_hotkey_pending: app.quick_note_hotkey_pending,
            quick_note_shortcut_error: app.quick_note_shortcut_error.as_deref(),
            hotkey_mode: settings.hotkey_mode,
            discord_auto_connect: settings.discord_auto_connect,
            discord_auto_transcribe: settings.discord_auto_transcribe,
            discord_companion: settings.discord_companion,
            vencord_auto_update: settings.vencord_auto_update,
            discord_flavor: &settings.discord_flavor,
            discord_avatars: settings.discord_avatars,
            meetings_teams: settings.meetings.teams,
            meetings_zoom: settings.meetings.zoom,
            meetings_meet: settings.meetings.meet,
            meetings_start: settings.meetings.start,
        },
        microphones: &app.microphones,
        outputs: &app.outputs,
    }
}

#[derive(Serialize)]
pub(crate) struct Vocabulary<'a> {
    dictionary: &'a [Entry],
    macros: &'a [VoiceMacro],
}

pub(crate) fn vocabulary(app: &App) -> Vocabulary<'_> {
    Vocabulary {
        dictionary: &app.settings.entries,
        macros: &app.settings.macros,
    }
}
