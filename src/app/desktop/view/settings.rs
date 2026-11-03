//! Preferences, devices and the vocabulary and snippet lists.

use crate::app::App;
use crate::{dictionary::Entry, macros::VoiceMacro, meetings, platform};
use serde::Serialize;

/// Renderer-visible preferences. Private values such as the Discord pairing
/// key and the model path are never included.
#[derive(Serialize)]
pub(crate) struct SettingsView<'a> {
    spoken_editing_commands: bool,
    styles: &'a [crate::writing_style::StyleRule],
    input_profiles: &'a [crate::writing_style::InputProfile],
    recent_dictation_app: Option<&'a str>,
    shortcut_error: Option<&'a str>,
    microphone_check: &'a crate::app::desktop::actions::capture::MicrophoneCheck,
    clean_speech: bool,
    auto_polish: bool,
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
    close_to_tray: bool,
    start_with_windows: bool,
    automatic_update_checks: bool,
    discord_flavor: &'a str,
    discord_avatars: bool,
    meetings_teams: bool,
    meetings_zoom: bool,
    meetings_meet: bool,
    meetings_start: meetings::StartMode,
    keep_call_audio: bool,
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
            spoken_editing_commands: settings.spoken_editing_commands,
            styles: &settings.styles,
            input_profiles: &settings.input_profiles,
            recent_dictation_app: app.dictation_app.as_deref(),
            shortcut_error: app.shortcut_error.as_deref(),
            microphone_check: &app.microphone_check,
            clean_speech: settings.clean_speech,
            auto_polish: settings.auto_polish,
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
            close_to_tray: settings.close_to_tray,
            start_with_windows: settings.start_with_windows,
            automatic_update_checks: settings.automatic_update_checks,
            discord_flavor: &settings.discord_flavor,
            discord_avatars: settings.discord_avatars,
            meetings_teams: settings.meetings.teams,
            meetings_zoom: settings.meetings.zoom,
            meetings_meet: settings.meetings.meet,
            meetings_start: settings.meetings.start,
            keep_call_audio: settings.keep_call_audio,
        },
        microphones: &app.microphones,
        outputs: &app.outputs,
    }
}

#[derive(Serialize)]
pub(crate) struct Vocabulary<'a> {
    assort: crate::app::assort_ui::ReviewStatus<'a>,
    dictionary: &'a [Entry],
    macros: &'a [VoiceMacro],
}

pub(crate) fn vocabulary(app: &App) -> Vocabulary<'_> {
    Vocabulary {
        assort: app.desktop_assort_status(),
        dictionary: &app.settings.entries,
        macros: &app.settings.macros,
    }
}
