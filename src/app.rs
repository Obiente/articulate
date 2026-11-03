use crate::{
    audio::{self, Recording},
    call_capture, calls, cleanup,
    dictionary::{self, Entry},
    engine::Engine,
    integration, learning, macros, model,
    platform::{self, Target},
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};
use transcribe_cpp::CancelToken;

mod assort_ui;
mod brain_ui;
pub(crate) mod call_tools;
mod capture_feedback;

pub(crate) mod desktop;
mod discord_ui;
mod history_ui;
mod insights_ui;
mod notetaker;
mod polish_ui;
mod search_ui;
mod shortcut_gesture;
mod transcript_editor;

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    close_to_tray: bool,
    start_with_windows: bool,
    automatic_update_checks: bool,
    cpu: bool,
    insert: bool,
    entries: Vec<Entry>,
    macros: Vec<macros::VoiceMacro>,
    model_path: String,
    transcription_language: String,
    microphone: Option<String>,
    #[serde(default = "enabled")]
    clean_speech: bool,
    output: Option<String>,
    #[serde(default = "enabled")]
    learn_corrections: bool,
    #[serde(default = "enabled")]
    live_insert: bool,
    hotkey: platform::Hotkey,
    quick_note_hotkey: platform::Hotkey,
    hotkey_mode: platform::HotkeyMode,
    audio_feedback: bool,
    audio_context: bool,
    styles: Vec<crate::writing_style::StyleRule>,
    spoken_editing_commands: bool,
    input_profiles: Vec<crate::writing_style::InputProfile>,
    discord_companion: bool,
    discord_pairing_key: String,
    #[serde(default = "enabled")]
    discord_auto_connect: bool,
    discord_auto_transcribe: bool,
    discord_standard_configured: bool,
    /// Installed Discord edition to relaunch: "stable", "ptb", "canary" or "" for automatic.
    discord_flavor: String,
    /// Loopback debugger port of the last Articulate relaunch, or 0 when none is open.
    discord_debugger_port: u16,
    #[serde(default = "enabled")]
    discord_avatars: bool,
    vencord_source: String,
    vencord_auto_update: bool,
    meetings: crate::meetings::Settings,
    assort: assort_ui::Configuration,
    polish_style: crate::polish::Style,
    #[serde(default)]
    auto_polish: bool,
    /// Encrypt history and this file with Windows DPAPI for the current user.
    encrypt_at_rest: bool,
    /// Optional automatic deletion of old sessions.
    retention: crate::history::retention::Policy,
    /// Keep call audio beside the transcript by default (off unless chosen).
    keep_call_audio: bool,
    /// Set when the saved file could not be read, so it is never overwritten.
    #[serde(skip)]
    save_blocked: Option<String>,
}
fn enabled() -> bool {
    true
}
fn default_quick_note_hotkey() -> platform::Hotkey {
    platform::Hotkey {
        key: "N".into(),
        ..platform::Hotkey::default()
    }
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            close_to_tray: false,
            start_with_windows: false,
            automatic_update_checks: true,
            cpu: false,
            insert: false,
            entries: Vec::new(),
            macros: Vec::new(),
            model_path: String::new(),
            transcription_language: String::new(),
            microphone: None,
            clean_speech: true,
            output: None,
            learn_corrections: true,
            live_insert: true,
            hotkey: platform::Hotkey::default(),
            quick_note_hotkey: default_quick_note_hotkey(),
            hotkey_mode: platform::HotkeyMode::default(),
            audio_feedback: true,
            audio_context: false,
            styles: Vec::new(),
            spoken_editing_commands: false,
            input_profiles: Vec::new(),
            discord_companion: false,
            discord_pairing_key: String::new(),
            discord_auto_connect: true,
            discord_auto_transcribe: false,
            discord_standard_configured: false,
            discord_flavor: String::new(),
            discord_debugger_port: 0,
            discord_avatars: true,
            vencord_source: String::new(),
            vencord_auto_update: false,
            meetings: Default::default(),
            assort: Default::default(),
            polish_style: Default::default(),
            auto_polish: false,
            encrypt_at_rest: false,
            retention: Default::default(),
            keep_call_audio: false,
            save_blocked: None,
        }
    }
}

enum Command {
    Load(PathBuf, bool, Option<String>),
    Transcribe(Vec<f32>, u64, bool, CancelToken),
    Call(calls::Request, CancelToken),
    /// Import a file or rebuild a transcript from retained audio.
    File(call_tools::FileJob, CancelToken),
}
enum Event {
    MicrophoneChecked(Result<audio::AudioCheck, String>),
    Loaded(String, Vec<String>),
    Text(Result<String, String>, f32, u64, bool),
    Error(String),
    Progress(f32),
    Downloaded(PathBuf),
    Call(calls::Update),
    CallFinished(Result<(), String>),
    File(call_tools::FileEvent),
    #[allow(
        dead_code,
        reason = "Speaker model download event retained for model management"
    )]
    ContextProgress(f32),
    ContextInstalled(Result<(), String>),
    SpeakersDownloaded,
    SpeakersProgress(f32),
    SpeakersDownloadFailed(String),
    VerifierDownloaded,
    VerifierProgress(f32),
    VerifierDownloadFailed(String),
}

pub struct App {
    polish: polish_ui::State,
    pending_install: Option<crate::update::InstallRequest>,
    history: history_ui::State,
    call_tools: call_tools::State,
    insights: insights_ui::State,
    saved_search: search_ui::State,
    brain: brain_ui::State,
    notetaker: notetaker::State,
    capture_feedback: capture_feedback::State,
    updater: crate::update::State,
    context_progress: Option<f32>,
    context_status: String,
    call_context: Vec<calls::Row>,
    speakers_downloading: Option<f32>,
    speakers_download_status: String,
    verifier_downloading: Option<f32>,
    verifier_download_status: String,
    dictation_metrics: crate::insights::DictationMetrics,
    settings: Settings,
    commands: Sender<Command>,
    events: Receiver<Event>,
    event_tx: Sender<Event>,
    hotkeys: Receiver<platform::HotkeyEvent>,
    hotkey_tx: Sender<platform::Hotkey>,
    hotkey_draft: platform::Hotkey,
    hotkey_pending: bool,
    quick_note_hotkeys: Receiver<platform::HotkeyEvent>,
    quick_note_hotkey_tx: Sender<platform::Hotkey>,
    quick_note_hotkey_pending: bool,
    quick_note_shortcut_error: Option<String>,
    hold_recording: bool,
    shortcut_gesture: shortcut_gesture::Gesture,

    call_search: String,
    call_tab: usize,
    cancel: CancelToken,
    recording: Option<Recording>,
    target: Option<Target>,
    loading: bool,
    busy: bool,
    ready: bool,
    downloading: Option<f32>,
    model_cancels: std::collections::HashMap<model::ModelId, Arc<AtomicBool>>,
    status: String,
    backend: String,
    speech_languages: Vec<String>,
    text: String,
    raw: String,
    elapsed: Option<f32>,
    words_changed: usize,

    page: usize,
    utterance: u64,
    preview_inflight: bool,
    preview_audio_seconds: f32,
    capture_interruption: Option<String>,
    dictation_editor: macros::DictationEditor,
    recording_spoken_editing: bool,
    last_dictation_text: String,
    microphone_check: desktop::actions::capture::MicrophoneCheck,
    pending_final: Option<Vec<f32>>,
    cleanup_count: usize,
    microphones: Vec<String>,
    shortcut_error: Option<String>,
    committed_raw: String,
    chunk_inflight: bool,
    call: Option<std::sync::Arc<call_capture::Control>>,
    discord: Option<std::sync::Arc<crate::discord::Connection>>,

    discord_launch: discord_ui::LaunchState,
    call_rows: Vec<calls::Row>,
    call_committed: Vec<calls::Row>,
    speaker_names: Vec<String>,
    outputs: Vec<String>,
    call_levels: (f32, f32),
    call_status: String,
    call_native_audio: bool,
    call_native_received: bool,
    call_cues_enabled: bool,
    integration_tx: Sender<integration::Action>,
    integration_rx: Receiver<integration::Event>,
    edit_baseline: String,
    edited_at: Option<Instant>,
    remembered: Vec<learning::Change>,
    overlay_until: Option<Instant>,
    overlay_message: String,
    live_mode: bool,
    insertion_cancel: CancelToken,
    integration_inflight: bool,
    integration_pending: Option<integration::Action>,
    dictation_app: Option<String>,
    macro_used: Option<String>,

    call_notes: Option<crate::notes::Notes>,

    last_seconds: u64,
}

/// Parse stored preferences in either plaintext or encrypted form.
fn parse_settings(bytes: Vec<u8>) -> anyhow::Result<Settings> {
    Ok(serde_json::from_slice(&crate::history::protect::open(
        bytes,
    )?)?)
}

fn save_settings(directory: &std::path::Path, settings: &Settings) -> anyhow::Result<()> {
    use crate::history::protect;
    use std::io::Write;
    static SAVES: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(reason) = &settings.save_blocked {
        anyhow::bail!("{reason}");
    }
    // Serialize replacement within this process. Separate processes never share
    // a temporary file, so a failed save cannot remove another writer's data.
    let _guard = SAVES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::fs::create_dir_all(directory)?;
    let bytes = protect::seal_if(
        settings.encrypt_at_rest,
        serde_json::to_vec_pretty(settings)?,
    )?;
    let temp = directory.join(format!(
        "settings-{}-{}.tmp",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let main = directory.join("settings.json");
    let result = (|| -> anyhow::Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        // Keep the previous good file. A damaged one never replaces a good backup,
        // and the backup follows the current encryption choice.
        let backup = directory.join("settings.json.bak");
        if let Ok(previous) = std::fs::read(&main)
            && let Ok(plain) = protect::open(previous)
            && serde_json::from_slice::<Settings>(&plain).is_ok()
        {
            protect::write_atomic(&backup, &protect::seal_if(settings.encrypt_at_rest, plain)?)?;
        } else {
            // The backup is kept, but always in the current form. A failure
            // here is reported by `migrate_settings_copies`.
            let _ = protect::migrate_file(&backup, settings.encrypt_at_rest, MAX_SETTINGS_BYTES);
        }
        std::fs::rename(&temp, &main)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Largest preferences file read or migrated.
const MAX_SETTINGS_BYTES: u64 = 64 * 1024 * 1024;

/// Where loaded preferences came from. Only `Main` confirms the saved
/// encryption choice: a backup can predate it, and defaults know nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsSource {
    Main,
    Backup,
    Defaults,
}

/// The last good preferences, if `settings.json.bak` holds a valid copy.
fn read_settings_backup(directory: &std::path::Path) -> Option<Settings> {
    std::fs::read(directory.join("settings.json.bak"))
        .ok()
        .and_then(|bytes| parse_settings(bytes).ok())
}

/// Load preferences without ever letting a later save destroy unreadable data.
///
/// A file that cannot be parsed or decrypted is renamed to
/// `settings.corrupt-<timestamp>.json` before anything can save, then the last
/// good `settings.json.bak` is used if there is one. The backup is also used
/// when `settings.json` is missing. A file that cannot be read at all (for
/// example, it is locked) blocks saving until restart instead.
fn load_settings(directory: &std::path::Path) -> (Settings, Option<String>, SettingsSource) {
    let main = directory.join("settings.json");
    let mut attempt = 0;
    let bytes = loop {
        match std::fs::read(&main) {
            Ok(bytes) => break bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return match read_settings_backup(directory) {
                    Some(settings) => (
                        settings,
                        Some(
                            "Saved settings were missing, so your previous settings were restored."
                                .into(),
                        ),
                        SettingsSource::Backup,
                    ),
                    None => (Settings::default(), None, SettingsSource::Defaults),
                };
            }
            // Antivirus, backup and indexing tools hold files briefly.
            Err(_) if attempt < 4 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(50 * attempt));
            }
            Err(e) => {
                let reason = format!(
                    "Saved settings could not be read ({e}). Default settings are in use, and changes will not be saved until Articulate restarts so your saved file is not replaced."
                );
                return (
                    Settings {
                        save_blocked: Some(reason.clone()),
                        ..Default::default()
                    },
                    Some(reason),
                    SettingsSource::Defaults,
                );
            }
        }
    };
    let error = match parse_settings(bytes) {
        Ok(settings) => return (settings, None, SettingsSource::Main),
        Err(error) if error.is::<crate::history::protect::Undecryptable>() => {
            "they are encrypted for a different Windows account"
        }
        Err(_) => "they are damaged",
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut moved = None;
    for suffix in 0..100 {
        let name = if suffix == 0 {
            format!("settings.corrupt-{stamp}.json")
        } else {
            format!("settings.corrupt-{stamp}-{suffix}.json")
        };
        let target = directory.join(&name);
        if target.exists() {
            continue;
        }
        if std::fs::rename(&main, &target).is_ok() {
            moved = Some(name);
        }
        break;
    }
    let Some(moved) = moved else {
        let reason = format!(
            "Saved settings could not be used because {error}. Default settings are in use, and changes will not be saved until Articulate restarts so your saved file is not replaced."
        );
        return (
            Settings {
                save_blocked: Some(reason.clone()),
                ..Default::default()
            },
            Some(reason),
            SettingsSource::Defaults,
        );
    };
    match read_settings_backup(directory) {
        Some(settings) => (
            settings,
            Some(format!(
                "Saved settings could not be used because {error}. They were kept as {moved}, and your previous settings were restored."
            )),
            SettingsSource::Backup,
        ),
        None => (
            Settings::default(),
            Some(format!(
                "Saved settings could not be used because {error}. They were kept as {moved}, and default settings are in use."
            )),
            SettingsSource::Defaults,
        ),
    }
}

/// Preferences ready for startup.
struct StartupSettings {
    settings: Settings,
    warning: Option<String>,
    /// The saved encryption choice was read from `settings.json`, so an
    /// interrupted encryption change may be completed in either direction.
    /// Otherwise stored files are left in their current form.
    protection_confirmed: bool,
}

/// Load preferences and settle how stored data is protected.
///
/// Recovered or default preferences never turn encryption off: if any stored
/// file is encrypted, encryption stays on. Preferences recovered from the
/// backup are installed again as `settings.json`.
fn load_startup_settings(data: &std::path::Path) -> StartupSettings {
    let (mut settings, mut warning, source) = load_settings(data);
    if source == SettingsSource::Main {
        return StartupSettings {
            settings,
            warning,
            protection_confirmed: true,
        };
    }
    if !settings.encrypt_at_rest && protected_data_present(data) {
        settings.encrypt_at_rest = true;
        let note = "Encryption at rest stays on because your saved data is encrypted.";
        warning = Some(match warning {
            Some(warning) => format!("{warning} {note}"),
            None => note.into(),
        });
    }
    if source == SettingsSource::Backup
        && settings.save_blocked.is_none()
        && let Err(error) = save_settings(data, &settings)
    {
        let note = format!("The restored settings could not be saved yet ({error}).");
        warning = Some(match warning {
            Some(warning) => format!("{warning} {note}"),
            None => note,
        });
    }
    StartupSettings {
        settings,
        warning,
        protection_confirmed: false,
    }
}

/// Whether any preference, history or people file is stored encrypted,
/// including copies that cannot be decrypted by this account.
fn protected_data_present(data: &std::path::Path) -> bool {
    use crate::history::protect;
    let named = ["settings.json", "settings.json.bak", "people.json"]
        .into_iter()
        .map(|name| data.join(name));
    let side = settings_copies(data).into_iter().map(|(path, _)| path);
    if named.chain(side).any(|path| protect::is_sealed_file(&path)) {
        return true;
    }
    crate::history::contains_protected(&data.join("history"))
}

/// Recovery copies of preferences: isolated files, which can be restored, and
/// temporaries abandoned by an interrupted save (`true`), which are never read.
fn settings_copies(data: &std::path::Path) -> Vec<(PathBuf, bool)> {
    let Ok(entries) = std::fs::read_dir(data) else {
        return Vec::new();
    };
    let own = format!("settings-{}-", std::process::id());
    let own_partial = format!(".settings.json.bak.{}-", std::process::id());
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            if name.starts_with("settings.corrupt-") && name.ends_with(".json") {
                Some((entry.path(), false))
            } else if (name.starts_with("settings-")
                && name.ends_with(".tmp")
                && !name.starts_with(&own))
                || (name.starts_with(".settings.json.bak.")
                    && name.ends_with(".partial")
                    && !name.starts_with(&own_partial))
            {
                Some((entry.path(), true))
            } else {
                None
            }
        })
        .collect()
}

/// Bring every copy of the preferences to the requested form: the backup and
/// isolated copies are rewritten, and abandoned temporaries are removed so no
/// plaintext copy is left behind. `settings.json` itself is written by
/// `save_settings`. Each failure is counted.
fn migrate_settings_copies(
    data: &std::path::Path,
    protect: bool,
) -> crate::history::MigrationReport {
    use crate::history::protect as seal;
    let mut report = crate::history::MigrationReport {
        protect,
        ..Default::default()
    };
    let backup = data.join("settings.json.bak");
    let copies = std::iter::once((backup, false)).chain(settings_copies(data));
    for (path, abandoned) in copies {
        let result = if abandoned {
            std::fs::remove_file(&path).map(|()| true).or_else(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    Ok(false)
                } else {
                    Err(error.into())
                }
            })
        } else {
            seal::migrate_file(&path, protect, MAX_SETTINGS_BYTES)
        };
        match result {
            Ok(true) => report.changed += 1,
            Ok(false) => {}
            Err(error) => {
                log::warn!("Could not update {}: {error:#}", path.display());
                report.failed += 1;
            }
        }
    }
    report
}

/// Merge preferences and vocabulary from a restored backup. Machine-specific
/// and secret settings (pairing key, model and tool paths) are always kept.
fn merge_restored_settings(
    current: &Settings,
    restored: Option<serde_json::Value>,
    library: Option<&str>,
) -> anyhow::Result<Settings> {
    let mut merged = serde_json::to_value(current)?;
    if let (Some(target), Some(serde_json::Value::Object(source))) = (
        merged.as_object_mut(),
        restored.map(crate::history::backup::portable_settings),
    ) {
        for (key, value) in source {
            target.insert(key, value);
        }
    }
    let mut settings: Settings = serde_json::from_value(merged)?;
    settings.save_blocked = current.save_blocked.clone();
    if let Some(library) = library {
        crate::library::parse(library)?.merge(&mut settings.entries, &mut settings.macros);
        dictionary::deduplicate(&mut settings.entries);
    }
    Ok(settings)
}

impl App {
    fn new() -> Self {
        let data = model::data_dir();
        let StartupSettings {
            mut settings,
            warning,
            protection_confirmed,
        } = load_startup_settings(&data);
        if protection_confirmed {
            // Complete an interrupted change of the preference copies.
            let report = migrate_settings_copies(&data, settings.encrypt_at_rest);
            if report.failed > 0 {
                log::warn!("{} settings copies could not be updated", report.failed);
            }
        }
        dictionary::deduplicate(&mut settings.entries);
        if settings.model_path.is_empty() {
            settings.model_path = model::default_path().to_string_lossy().into_owned();
        }
        let (commands, rx) = mpsc::channel();
        let (tx, events) = mpsc::channel();
        let worker_tx = tx.clone();
        let cancel = CancelToken::new();
        let worker_cancel = cancel.clone();

        std::thread::spawn(move || {
            let mut engine: Option<Engine> = None;
            while let Ok(command) = rx.recv() {
                let result = match command {
                    Command::Call(request, token) => {
                        let result = if let Some(engine) = engine.as_mut() {
                            engine.set_cancel_token(&token);
                            let preference = request.language_preference.clone();
                            engine
                                .with_call_preference(preference, |engine| {
                                    calls::run(engine, request, |update| {
                                        let _ = worker_tx.send(Event::Call(update));
                                    })
                                })
                                .map_err(|e| format!("{e:#}"))
                        } else {
                            Err("Load a speech model first".into())
                        };
                        Event::CallFinished(result)
                    }
                    Command::Load(path, cpu, language) => {
                        engine = None;
                        match Engine::load_with_language(&path, cpu, &worker_cancel, language) {
                            Ok(e) => {
                                let backend = e.backend.clone();
                                let languages = e.languages.clone();
                                engine = Some(e);
                                Event::Loaded(backend, languages)
                            }
                            Err(e) => Event::Error(format!("Could not load the model: {e:#}")),
                        }
                    }
                    Command::File(job, token) => {
                        let result = match engine.as_mut() {
                            Some(engine) => {
                                engine.set_cancel_token(&token);
                                call_tools::run_file_job(engine, job, &token, &mut |event| {
                                    let _ = worker_tx.send(Event::File(event));
                                })
                                .map_err(|e| format!("{e:#}"))
                            }
                            None => Err("Load a speech model first".into()),
                        };
                        Event::File(call_tools::FileEvent::Finished(result))
                    }
                    Command::Transcribe(pcm, id, final_pass, token) => {
                        let start = Instant::now();
                        let result = match engine.as_mut() {
                            Some(e) => {
                                e.set_cancel_token(&token);
                                (if final_pass {
                                    e.transcribe_final(&pcm)
                                } else {
                                    e.transcribe_preview(&pcm)
                                })
                                .map_err(|e| format!("{e:#}"))
                            }
                            None => Err("Load a model first".into()),
                        };
                        Event::Text(result, start.elapsed().as_secs_f32(), id, final_pass)
                    }
                };
                let _ = worker_tx.send(result);
            }
        });
        let (hotkey_tx, hotkeys) = mpsc::channel();
        let hotkey_draft = settings.hotkey.clone();
        let hotkey_tx = platform::hotkey(settings.hotkey.clone(), hotkey_tx);
        let (quick_note_events, quick_note_hotkeys) = mpsc::channel();
        let quick_note_hotkey_tx =
            platform::hotkey(settings.quick_note_hotkey.clone(), quick_note_events);

        let (integration_tx, integration_rx) = integration::start(|| {});
        let call_tools = call_tools::State::start(settings.encrypt_at_rest);
        let mut app = Self {
            polish: Default::default(),

            pending_install: None,
            history: history_ui::State::start(settings.encrypt_at_rest, protection_confirmed),
            call_tools,
            insights: Default::default(),
            saved_search: Default::default(),
            brain: Default::default(),
            notetaker: Default::default(),
            capture_feedback: Default::default(),
            updater: Default::default(),
            context_progress: None,
            context_status: String::new(),
            call_context: Vec::new(),
            speakers_downloading: None,
            speakers_download_status: String::new(),
            verifier_downloading: None,
            verifier_download_status: String::new(),
            dictation_metrics: Default::default(),
            settings,
            commands,
            events,
            event_tx: tx,
            hotkeys,
            hotkey_tx,
            hotkey_draft,
            hotkey_pending: true,
            quick_note_hotkeys,
            quick_note_hotkey_tx,
            quick_note_hotkey_pending: true,
            quick_note_shortcut_error: None,
            hold_recording: false,
            shortcut_gesture: Default::default(),

            call_search: String::new(),
            call_tab: 0,
            cancel,
            recording: None,
            target: None,
            loading: false,
            busy: false,
            ready: false,
            downloading: None,
            model_cancels: Default::default(),
            status: "Choose a model to get started".into(),
            backend: String::new(),
            speech_languages: Vec::new(),
            text: String::new(),
            raw: String::new(),
            elapsed: None,
            words_changed: 0,

            page: 0,
            utterance: 0,
            preview_inflight: false,
            preview_audio_seconds: 0.0,
            capture_interruption: None,
            dictation_editor: Default::default(),
            recording_spoken_editing: false,
            last_dictation_text: String::new(),
            microphone_check: Default::default(),
            pending_final: None,
            cleanup_count: 0,
            microphones: audio::input_devices(),
            shortcut_error: None,
            committed_raw: String::new(),
            chunk_inflight: false,
            call: None,
            discord: None,

            discord_launch: Default::default(),
            call_rows: Vec::new(),
            call_committed: Vec::new(),
            speaker_names: calls::empty_speaker_names(),
            outputs: call_capture::outputs(),
            call_levels: (0.0, 0.0),
            call_status: "Ready to capture a call".into(),
            call_native_audio: false,
            call_native_received: false,
            call_cues_enabled: false,
            integration_tx,
            integration_rx,
            edit_baseline: String::new(),
            edited_at: None,
            remembered: Vec::new(),
            overlay_until: None,
            overlay_message: String::new(),
            live_mode: false,
            insertion_cancel: CancelToken::new(),
            integration_inflight: false,
            integration_pending: None,
            dictation_app: None,
            macro_used: None,

            call_notes: None,

            last_seconds: 0,
        };
        if PathBuf::from(&app.settings.model_path).is_file() {
            app.load();
        }
        if let Some(warning) = warning {
            app.status = warning;
        }
        app
    }

    fn save(&mut self) {
        if let Err(e) = self.save_preferences() {
            self.status = format!("Could not save preferences: {e}");
        }
    }

    fn save_preferences(&self) -> anyhow::Result<()> {
        save_settings(&model::data_dir(), &self.settings)
    }

    fn load(&mut self) {
        self.ready = false;
        self.loading = true;
        self.status = "Loading your model...".into();
        self.cancel.reset();
        if self
            .commands
            .send(Command::Load(
                PathBuf::from(&self.settings.model_path),
                self.settings.cpu,
                (!self.settings.transcription_language.is_empty())
                    .then(|| self.settings.transcription_language.clone()),
            ))
            .is_err()
        {
            self.loading = false;
            self.status = "The transcription worker stopped. Please restart the app.".into();
        }
    }

    fn toggle(&mut self, target: Option<Target>) {
        if let Some(recording) = self.recording.take() {
            self.hold_recording = false;
            self.shortcut_gesture = Default::default();
            self.last_seconds = recording.seconds() as u64;
            self.dictation_metrics.audio_duration_ms =
                Some((f64::from(recording.seconds()) * 1000.0) as u64);
            let result = recording.stop().map(|captured| {
                self.capture_interruption = captured.interruption.map(str::to_owned);
                if self.capture_interruption.is_some() {
                    self.target = None;
                    self.insertion_cancel.cancel();
                    self.integration_pending = None;
                }
                captured.pcm
            });
            if self.settings.audio_feedback {
                crate::audio_cues::play(crate::audio_cues::Cue::Stopped);
            }
            match result {
                Ok(pcm)
                    if pcm.len() >= 3200
                        || !self.committed_raw.is_empty()
                        || self.chunk_inflight =>
                {
                    self.busy = true;
                    self.status = "Finishing your transcript...".into();
                    if self.preview_inflight {
                        self.pending_final = Some(pcm);
                        if !self.chunk_inflight {
                            self.cancel.cancel();
                        }
                    } else {
                        self.submit(pcm, true);
                    }
                }
                Ok(_) => {
                    self.status = "Recording was too short. Try again.".into();
                    self.target = None;
                }
                Err(e) => {
                    self.status = e.to_string();
                    self.target = None;
                }
            }
        } else {
            if self.busy
                || self.calls_file_job_running()
                || self.loading
                || !self.ready
                || self.downloading.is_some()
                || self.call.is_some()
            {
                return;
            }
            match Recording::start(self.settings.microphone.as_deref()) {
                Ok(recording) => {
                    self.history_save_dictation();
                    self.history.dictation = None;
                    self.history.dictation_deleted = false;
                    self.status = audio::device_notice(
                        self.settings.microphone.as_deref(),
                        &recording.device,
                    )
                    .unwrap_or_else(|| format!("Listening on {}", recording.device));
                    self.capture_interruption = None;
                    if !self.recording_spoken_editing {
                        self.dictation_editor = Default::default();
                        self.dictation_editor
                            .apply(&self.text, &macros::EditingSettings::default());
                    }
                    self.dictation_editor.begin_recording(
                        self.text == self.last_dictation_text
                            && self.dictation_app.is_none()
                            && target.is_none(),
                    );
                    self.recording = Some(recording);
                    if self.settings.audio_feedback {
                        crate::audio_cues::play(crate::audio_cues::Cue::Started);
                    }
                    self.last_seconds = 0;
                    self.target = target;
                    self.dictation_app = target.and_then(platform::app_name);
                    self.recording_spoken_editing = self.spoken_editing();
                    self.dictation_metrics = crate::insights::DictationMetrics {
                        app: self.dictation_app.clone(),
                        ..Default::default()
                    };
                    self.macro_used = None;
                    self.elapsed = None;
                    self.utterance += 1;
                    self.insertion_cancel.cancel();
                    self.insertion_cancel = CancelToken::new();
                    self.integration_inflight = false;
                    self.integration_pending = None;
                    self.live_mode = self.insert_enabled()
                        && self
                            .input_profile()
                            .and_then(|p| p.live_insert)
                            .unwrap_or(self.settings.live_insert)
                        && target.is_some()
                        && !self.hold_recording;
                    let _ = self.integration_tx.send(integration::Action::Cancel);
                    let line_break = self
                        .input_profile()
                        .and_then(|p| p.line_break)
                        .unwrap_or_else(|| {
                            crate::writing_style::default_line_break(self.dictation_app.as_deref())
                        });
                    let _ = self
                        .integration_tx
                        .send(integration::Action::LineBreak(line_break));
                    if self.insert_enabled()
                        && let Some(target) = target
                    {
                        let _ = self.integration_tx.send(integration::Action::Arm(
                            self.utterance,
                            target,
                            self.live_mode,
                        ));
                    }
                    self.edit_baseline.clear();
                    self.edited_at = None;
                    self.preview_audio_seconds = 0.0;
                    self.pending_final = None;
                    self.text.clear();
                    self.raw.clear();
                    self.committed_raw.clear();
                    self.words_changed = 0;
                    self.cleanup_count = 0;
                }
                Err(e) => self.status = format!("{e:#}"),
            }
        }
    }

    fn shortcut(&mut self, target: Target, at: Instant) {
        if self.recording.is_some() {
            if self.input_hotkey_mode() == platform::HotkeyMode::Toggle
                || (self.hold_recording && self.shortcut_gesture.press_again(at))
            {
                self.toggle(None);
            }
            return;
        }
        let blocked = if self.call.is_some() {
            Some("Finish your call recording before starting dictation.")
        } else if self.downloading.is_some() {
            Some("Your speech model is downloading. Dictation will be ready when it finishes.")
        } else if self.loading {
            Some("Your speech model is getting ready. Try again in a moment.")
        } else if !self.ready {
            Some("Open Settings in Articulate to set up your speech model.")
        } else if self.busy {
            Some("Finishing your current dictation. Try again in a moment.")
        } else {
            None
        };
        if let Some(message) = blocked {
            self.status = message.into();
            self.overlay_message = message.into();
            self.overlay_until = Some(Instant::now() + Duration::from_secs(4));
            return;
        }
        let app = platform::app_name(target);
        let mode = self
            .settings
            .input_profiles
            .iter()
            .rev()
            .find(|p| {
                app.as_deref()
                    .is_some_and(|app| app.eq_ignore_ascii_case(&p.app))
            })
            .and_then(|p| p.hotkey_mode)
            .unwrap_or(self.settings.hotkey_mode);
        if mode == platform::HotkeyMode::Hold {
            // A release may finish only the recording this press started. A
            // manual capture must remain under its explicit Finish control.
            self.hold_recording = true;
            self.shortcut_gesture.start(at);
            self.toggle(target.is_external().then_some(target));
            if self.recording.is_none() {
                self.hold_recording = false;
                self.shortcut_gesture = Default::default();
            }
        } else {
            self.toggle(target.is_external().then_some(target));
        }
    }

    fn release_shortcut(&mut self, at: Instant) {
        if self.hold_recording {
            if self.recording.is_some() && self.shortcut_gesture.release(at) {
                self.toggle(None);
            } else if self.recording.is_none() {
                self.hold_recording = false;
                self.shortcut_gesture = Default::default();
            }
        }
    }

    fn shortcut_instruction(&self) -> String {
        let key = self.settings.hotkey.label();
        match self.input_hotkey_mode() {
            platform::HotkeyMode::Hold
                if self.hold_recording && self.shortcut_gesture.hands_free =>
            {
                format!("Hands-free · Press {key} to finish")
            }
            platform::HotkeyMode::Hold
                if self.hold_recording && self.shortcut_gesture.awaiting_second_press() =>
            {
                "Press again for hands-free".into()
            }
            platform::HotkeyMode::Hold if self.hold_recording => format!("Release {key} to finish"),
            platform::HotkeyMode::Hold if self.recording.is_some() => {
                "Choose Finish to end this recording".into()
            }
            platform::HotkeyMode::Hold => format!("Hold {key} to speak"),
            platform::HotkeyMode::Toggle if self.recording.is_some() => {
                format!("Press {key} to finish")
            }
            platform::HotkeyMode::Toggle => format!("Press {key} to start"),
        }
    }

    fn cancel_dictation(&mut self) {
        self.capture_interruption = None;
        self.cancel.cancel();
        self.insertion_cancel.cancel();
        self.pending_final = None;
        self.integration_pending = None;
        self.target = None;
        self.recording = None;
        self.hold_recording = false;
        self.shortcut_gesture = Default::default();
        self.live_mode = false;
        self.busy = self.calls_file_job_running();
        self.preview_inflight = false;
        self.chunk_inflight = false;
        self.integration_inflight = false;
        // Late ASR and insertion acknowledgements belong to the cancelled job.
        self.utterance += 1;
        let _ = self.integration_tx.send(integration::Action::Cancel);
        self.status = "Transcription cancelled. Your visible text is kept.".into();
        self.overlay_message.clone_from(&self.status);
        self.overlay_until = Some(Instant::now() + Duration::from_secs(5));
    }

    #[allow(
        dead_code,
        reason = "Audio import controller retained for the Tauri file import flow"
    )]
    fn transcribe(&mut self, pcm: Vec<f32>) {
        self.capture_interruption = None;
        self.history_save_dictation();
        self.history.dictation = None;
        self.history.dictation_deleted = false;
        self.dictation_app = None;
        self.dictation_editor = Default::default();
        self.dictation_metrics = Default::default();
        // The previous view is already saved. A failed import must not later
        // create another session containing the previous dictation's words.
        self.text.clear();
        self.raw.clear();
        self.edit_baseline.clear();
        self.edited_at = None;
        self.elapsed = None;
        self.words_changed = 0;
        self.cleanup_count = 0;
        self.macro_used = None;
        self.committed_raw.clear();
        self.utterance += 1;
        self.busy = true;
        self.status = "Transcribing on your computer...".into();
        self.submit(pcm, true);
    }

    fn submit(&mut self, pcm: Vec<f32>, final_pass: bool) {
        self.cancel = CancelToken::new();
        self.preview_inflight = true;
        if self
            .commands
            .send(Command::Transcribe(
                pcm,
                self.utterance,
                final_pass,
                self.cancel.clone(),
            ))
            .is_err()
        {
            self.busy = false;
            self.preview_inflight = false;
            self.status = "The transcription worker stopped. Please restart the app.".into();
        }
    }

    fn input_profile(&self) -> Option<&crate::writing_style::InputProfile> {
        self.settings.input_profiles.iter().rev().find(|p| {
            self.dictation_app
                .as_deref()
                .is_some_and(|app| app.eq_ignore_ascii_case(&p.app))
        })
    }
    fn input_hotkey_mode(&self) -> platform::HotkeyMode {
        self.input_profile()
            .and_then(|p| p.hotkey_mode)
            .unwrap_or(self.settings.hotkey_mode)
    }
    fn spoken_editing(&self) -> bool {
        self.input_profile()
            .and_then(|p| p.spoken_editing_commands)
            .unwrap_or(self.settings.spoken_editing_commands)
    }
    fn insert_enabled(&self) -> bool {
        self.input_profile()
            .and_then(|p| p.insert)
            .unwrap_or(self.settings.insert)
    }
    fn dictation_style(&self) -> crate::writing_style::WritingStyle {
        self.input_profile()
            .and_then(|p| p.style)
            .unwrap_or_else(|| {
                crate::writing_style::effective(
                    &self.settings.styles,
                    self.dictation_app.as_deref(),
                    self.settings.clean_speech,
                )
            })
    }
    fn finalized_text(&mut self, segment: String) {
        if self.recording_spoken_editing {
            let edited = self.dictation_editor.apply(
                &segment,
                &macros::EditingSettings {
                    spoken_editing_commands: true,
                },
            );
            self.committed_raw.clear();
            self.update_text(edited);
        } else {
            self.update_text(segment);
        }
    }

    fn update_text(&mut self, raw: String) {
        self.raw = [self.committed_raw.as_str(), raw.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let style = self.dictation_style();
        let protected: Vec<_> = self
            .settings
            .entries
            .iter()
            .filter(|entry| {
                entry.enabled
                    && entry.app.as_deref().is_none_or(|scope| {
                        self.dictation_app
                            .as_deref()
                            .is_some_and(|app| scope.eq_ignore_ascii_case(app))
                    })
            })
            .flat_map(|entry| [entry.heard.clone(), entry.wanted.clone()])
            .collect();
        let (cleaned, count) = crate::writing_style::apply(&self.raw, style, &protected);
        self.cleanup_count = count;
        (self.text, self.words_changed) = dictionary::apply_in(
            &cleaned,
            &self.settings.entries,
            self.dictation_app.as_deref(),
        );
        self.edit_baseline.clone_from(&self.text);
        self.edited_at = None;
    }

    fn preview(&mut self) {
        #[cfg(windows)]
        if let Some((microphones, outputs)) = crate::platform::devices::refresh() {
            self.microphones = microphones;
            self.outputs = outputs;
        }
        if let Some(recording) = self.recording.as_mut() {
            recording.drain();
        }
        if self
            .recording
            .as_ref()
            .is_some_and(|r| r.full.load(Ordering::Relaxed) || r.failed.load(Ordering::Relaxed))
        {
            self.toggle(None);
            return;
        }
        if self.preview_inflight || self.busy {
            return;
        }
        if let Some(recording) = self.recording.as_mut() {
            match recording.take_window() {
                Ok(Some(pcm)) => {
                    self.chunk_inflight = true;
                    self.preview_audio_seconds = 0.0;
                    self.submit(pcm, true);
                    return;
                }
                Ok(None) => {}
                Err(error) => {
                    self.recording = None;
                    self.hold_recording = false;
                    self.shortcut_gesture = Default::default();
                    self.status = error.to_string();
                    return;
                }
            }
            let seconds = recording.window_seconds();
            if seconds >= 1.2
                && seconds - self.preview_audio_seconds
                    >= self.elapsed.unwrap_or(0.8).clamp(0.8, 8.0) * 1.25
            {
                self.preview_audio_seconds = seconds;
                match recording.snapshot() {
                    Ok(pcm) => self.submit(pcm, false),
                    Err(error) => self.status = error.to_string(),
                }
            }
        }
    }

    fn queue_integration(&mut self, action: integration::Action) -> bool {
        // Keep at most one update waiting; a final result supersedes previews.
        if self.integration_inflight {
            self.integration_pending = Some(action);
            true
        } else {
            self.integration_inflight = self.integration_tx.send(action).is_ok();
            self.integration_inflight
        }
    }

    fn publish_live_preview(&mut self) {
        if self.recording_spoken_editing
            && self.raw.is_empty()
            && self.dictation_editor.notice.is_none()
        {
            return;
        }
        if self.live_mode
            && let Some(target) = self.target
        {
            let action = integration::Action::Preview {
                id: self.utterance,
                target,
                // Hold spoken commands until the complete utterance is known.
                text: if macros::candidate(&self.raw)
                    && self.settings.macros.iter().any(|m| m.enabled)
                {
                    String::new()
                } else {
                    self.text.clone()
                },
                cancel: self.insertion_cancel.clone(),
            };
            if !self.queue_integration(action) {
                self.target = None;
                self.status = "Live typing stopped. Your transcript continues here.".into();
            }
        }
    }

    fn receive(&mut self) {
        self.poll_capture_stop_feedback();
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Call(update) => match update {
                    calls::Update::Started => {
                        self.capture_started_feedback();
                        if self.is_note_capture() {
                            self.call_status = "Listening to your thoughts".into();
                        } else {
                            self.discord_launch.automation.capture_started();
                            self.call_status = "Listening to your microphone and call audio".into();
                        }
                    }
                    calls::Update::SpeakerFallback => {
                        self.call_status = "Recording call audio. Individual speaker identification is unavailable.".into();
                    }
                    calls::Update::NativeAudio(received) => {
                        self.call_native_received = received;
                        self.call_status = if received {
                            "Capturing separate participant audio"
                        } else {
                            "Waiting for participant audio"
                        }
                        .into();
                    }
                    calls::Update::Levels(mic, output) => self.call_levels = (mic, output),
                    calls::Update::AudioContext(row) => {
                        if self.call_context.len() < 4096 {
                            crate::sensevoice::apply(&mut self.call_rows, &row);
                            crate::sensevoice::apply_existing(&mut self.call_committed, &row);
                            self.call_context.push(row);
                            self.history_call_changed();
                        }
                    }
                    calls::Update::AudioContextStatus(message) => self.context_status = message,
                    calls::Update::Interruption(gap) => self.calls_interruption(gap),
                    calls::Update::AudioGap(lost) => {
                        if self.call.is_some()
                            && let Some(session) = &mut self.history.call
                            && session.kind == crate::history::Kind::Call
                            && lost > session.audio_packets_lost
                        {
                            session.audio_packets_lost = lost;
                            if let Some(selected) = &mut self.history.selected
                                && selected.id == session.id
                            {
                                selected.audio_packets_lost = lost;
                            }
                            self.history_call_changed();
                        }
                    }
                    calls::Update::Rows(mut rows) => {
                        for annotation in &self.call_context {
                            crate::sensevoice::apply_existing(&mut rows, annotation);
                        }
                        self.calls_commit_rows(rows);
                        self.call_rows.clone_from(&self.call_committed);
                        self.refresh_call_notes();
                        self.history_call_changed();
                    }
                    calls::Update::Preview(mut rows) => {
                        for annotation in &self.call_context {
                            crate::sensevoice::apply_existing(&mut rows, annotation);
                        }
                        self.calls_preview_rows(rows);
                        self.refresh_call_notes();
                        self.history_call_changed();
                    }
                },
                Event::CallFinished(result) => {
                    self.capture_finished_feedback(result.as_ref().err().map(String::as_str));
                    let spoken_note = self.is_note_capture();
                    if result.is_err() && !spoken_note {
                        self.discord_launch.automation.capture_failed(
                            Instant::now(),
                            !self.call_rows.is_empty(),
                            self.call
                                .as_ref()
                                .is_some_and(|control| control.stop_ns.load(Ordering::SeqCst) != 0),
                        );
                    }
                    self.calls_capture_finished();
                    self.history_save_call();
                    if spoken_note && let Some(session) = &self.history.call {
                        self.notetaker_queue_filing(session.clone());
                    }
                    self.brain_queue_finished_call();
                    self.call = None;
                    self.call_levels = (0.0, 0.0);
                    self.call_status = match result {
                        Ok(()) if spoken_note => "Your spoken note is saved".into(),
                        Ok(()) => "Call transcript ready".into(),
                        Err(error) => error,
                    };
                }
                Event::File(event) => self.calls_file_event(event),
                Event::ContextProgress(p) => self.context_progress = Some(p),
                Event::ContextInstalled(result) => {
                    self.context_progress = None;
                    self.context_status = match result {
                        Ok(()) => "Audio context is installed. Enable sound cues below.".into(),
                        Err(e) => e,
                    };
                }
                Event::SpeakersDownloaded => {
                    self.speakers_downloading = None;
                    self.speakers_download_status = "Speaker identification is ready".into();
                    self.call_status = "Speaker identification is ready".into();
                }
                Event::SpeakersProgress(progress) => self.speakers_downloading = Some(progress),
                Event::SpeakersDownloadFailed(error) => {
                    self.speakers_downloading = None;
                    self.speakers_download_status = error;
                }
                Event::VerifierDownloaded => {
                    self.verifier_downloading = None;
                    self.verifier_download_status = "Second speech model is ready".into();
                    if self.recording.is_none()
                        && self.call.is_none()
                        && !self.busy
                        && PathBuf::from(&self.settings.model_path).is_file()
                    {
                        self.load();
                    }
                }
                Event::VerifierProgress(progress) => self.verifier_downloading = Some(progress),
                Event::VerifierDownloadFailed(error) => {
                    self.verifier_downloading = None;
                    self.verifier_download_status = error;
                }
                Event::MicrophoneChecked(result) => {
                    self.microphone_check.running = false;
                    match result {
                        Ok(check) => self.microphone_check.result = Some(check),
                        Err(error) => self.microphone_check.error = Some(error),
                    }
                }
                Event::Loaded(backend, languages) => {
                    self.speech_languages = languages;
                    self.loading = false;
                    self.ready = true;
                    self.backend = backend;
                    self.status = "Ready when you are".into();
                }
                Event::Text(result, elapsed, id, final_pass) => {
                    if id != self.utterance {
                        continue;
                    }
                    self.preview_inflight = false;
                    let was_chunk = self.chunk_inflight;
                    self.chunk_inflight = false;
                    if was_chunk && id == self.utterance {
                        match &result {
                            Ok(text) => {
                                self.finalized_text(text.clone());
                                self.committed_raw = self.raw.clone();
                                self.publish_live_preview();
                            }
                            Err(error) => {
                                self.recording = None;
                                self.hold_recording = false;
                                self.shortcut_gesture = Default::default();
                                self.busy = false;
                                self.pending_final = None;
                                self.target = None;
                                self.status = format!("Recording stopped: {error}");
                                continue;
                            }
                        }
                    }
                    if let Some(pcm) = self.pending_final.take() {
                        self.submit(pcm, true);
                        continue;
                    }
                    if was_chunk {
                        continue;
                    }
                    if final_pass {
                        self.busy = false;
                    }
                    if self.cancel.is_cancelled() {
                        self.target = None;
                        if final_pass {
                            self.status = "Transcription cancelled".into();
                        }
                        continue;
                    }
                    let text = match result {
                        Ok(text) => text,
                        Err(error) => {
                            if final_pass {
                                self.target = None;
                                self.status = format!(
                                    "Final check failed. The visible preview may be incomplete: {error}"
                                );
                            }
                            continue;
                        }
                    };
                    if final_pass
                        && self.recording_spoken_editing
                        && self.insert_enabled()
                        && self.committed_raw.is_empty()
                        && let Some(undo) = macros::previous_command(&text)
                        && let Some(target) = self.target.take()
                    {
                        self.busy = true;
                        self.integration_inflight = true;
                        if self
                            .integration_tx
                            .send(integration::Action::EditPrevious {
                                id: self.utterance,
                                target,
                                undo,
                                cancel: self.insertion_cancel.clone(),
                            })
                            .is_err()
                        {
                            self.busy = false;
                            self.integration_inflight = false;
                            self.status = "Previous insertion editing is unavailable.".into();
                        }
                        continue;
                    }
                    if final_pass {
                        self.finalized_text(text);
                    } else if !self.recording_spoken_editing {
                        self.update_text(text);
                    }
                    self.elapsed = Some(elapsed);
                    if !final_pass {
                        self.publish_live_preview();
                        continue;
                    }
                    if let Some(message) = &self.capture_interruption {
                        self.status = message.clone();
                        self.history_save_dictation();
                        continue;
                    }
                    // Resolve from speech, before dictionary rules, and never rewrite literal templates.
                    let spoken =
                        if self.dictation_style() != crate::writing_style::WritingStyle::Verbatim {
                            cleanup::apply(&self.raw).0
                        } else {
                            self.raw.clone()
                        };
                    match macros::expand(
                        &spoken,
                        &self.settings.macros,
                        self.dictation_app.as_deref(),
                    ) {
                        Ok(Some(expansion)) => {
                            self.text = expansion.text;
                            self.macro_used = Some(expansion.trigger);
                            self.words_changed = 0;
                            self.cleanup_count = 0;
                            self.edit_baseline.clone_from(&self.text);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            self.status = error.to_string();
                            self.target = None;
                            self.insertion_cancel.cancel();
                            self.integration_pending = None;
                            self.overlay_message.clone_from(&self.status);
                            self.overlay_until = Some(Instant::now() + Duration::from_secs(7));
                            continue;
                        }
                    }
                    self.last_dictation_text.clone_from(&self.text);
                    self.dictation_metrics.recognized_words =
                        Some(self.raw.split_whitespace().count() as u64);
                    self.dictation_metrics.dictionary_replacements =
                        Some(self.words_changed as u64);
                    self.dictation_metrics.cleanup_edits = Some(self.cleanup_count as u64);
                    self.status = if let Some(notice) = &self.dictation_editor.notice {
                        notice.clone()
                    } else if self.text.is_empty() {
                        "No speech was recognized".into()
                    } else {
                        "Your transcript is ready".into()
                    };
                    if self.auto_polish_start() {
                        continue;
                    }
                    self.history_save_dictation();
                    if self.insert_enabled()
                        && (!self.text.is_empty() || self.live_mode)
                        && let Some(target) = self.target.take()
                    {
                        let action = integration::Action::Insert {
                            id: self.utterance,
                            target,
                            text: self.text.clone(),
                            learn: self.settings.learn_corrections && self.macro_used.is_none(),
                            cancel: self.insertion_cancel.clone(),
                        };
                        if self.queue_integration(action) {
                            self.busy = true;
                            self.status = "Typing into your app...".into();
                        } else {
                            self.status =
                                "App insertion is unavailable. Your transcript is ready to copy."
                                    .into();
                        }
                    }
                    if self.target.is_some() || self.busy {
                        self.overlay_message = self.status.clone();
                        self.overlay_until = Some(Instant::now() + Duration::from_secs(5));
                    }
                    self.target = None;
                }
                Event::Error(error) => {
                    self.loading = false;
                    self.busy = false;
                    self.downloading = None;
                    self.target = None;
                    self.status = if self.cancel.is_cancelled() {
                        "Transcription cancelled".into()
                    } else {
                        error
                    };
                    self.call_status.clone_from(&self.status);
                }
                Event::Progress(progress) => self.downloading = Some(progress),
                Event::Downloaded(path) => {
                    self.downloading = None;
                    self.settings.model_path = path.to_string_lossy().into_owned();
                    self.save();
                    self.load();
                }
            }
        }
        while let Ok(event) = self.hotkeys.try_recv() {
            match event {
                platform::HotkeyEvent::Pressed(target, at) if self.pending_install.is_none() => {
                    self.shortcut(target, at)
                }
                platform::HotkeyEvent::Pressed(_, _) => {}
                platform::HotkeyEvent::Released(at) => self.release_shortcut(at),
                platform::HotkeyEvent::Registered(hotkey) => {
                    let changed = self.settings.hotkey != hotkey;
                    self.settings.hotkey = hotkey;
                    self.hotkey_pending = false;
                    self.shortcut_error = None;
                    if changed {
                        self.save();
                    }
                }
                platform::HotkeyEvent::Error { requested, message } => {
                    self.hotkey_draft = requested;
                    self.hotkey_pending = false;
                    self.shortcut_error = Some(message);
                }
            }
        }
        while let Ok(event) = self.quick_note_hotkeys.try_recv() {
            match event {
                platform::HotkeyEvent::Pressed(_, _) if self.pending_install.is_none() => {
                    if let Err(error) = self.notetaker_toggle_capture() {
                        self.status.clone_from(&error);
                        self.overlay_message = error;
                        self.overlay_until = Some(Instant::now() + Duration::from_secs(5));
                    }
                }
                platform::HotkeyEvent::Registered(hotkey) => {
                    let changed = self.settings.quick_note_hotkey != hotkey;
                    self.settings.quick_note_hotkey = hotkey;
                    self.quick_note_hotkey_pending = false;
                    self.quick_note_shortcut_error = None;
                    if changed {
                        self.save();
                    }
                }
                platform::HotkeyEvent::Error { message, .. } => {
                    self.quick_note_hotkey_pending = false;
                    self.quick_note_shortcut_error = Some(message);
                }
                platform::HotkeyEvent::Pressed(_, _) | platform::HotkeyEvent::Released(_) => {}
            }
        }
        while let Ok(event) = self.integration_rx.try_recv() {
            match event {
                integration::Event::LiveSupport(id, supported) if id == self.utterance => {
                    if !supported {
                        self.live_mode = false;
                        self.status =
                            "This field supports insertion after you finish. Keep speaking.".into();
                    }
                }
                integration::Event::Previewed(id, result) if id == self.utterance => {
                    self.integration_inflight = false;
                    if let Err(error) = result {
                        log::warn!(
                            "Live typing stopped in {}: {error}",
                            self.dictation_app.as_deref().unwrap_or("an unknown app")
                        );
                        self.insertion_cancel.cancel();
                        self.integration_pending = None;
                        self.target = None;
                        self.live_mode = false;
                        self.busy = self.recording.is_none() && self.preview_inflight;
                        self.status = error;
                        self.overlay_message.clone_from(&self.status);
                        self.overlay_until = Some(Instant::now() + Duration::from_secs(7));
                    } else if let Some(action) = self.integration_pending.take() {
                        self.queue_integration(action);
                    }
                }
                integration::Event::Edited(id, result) if id == self.utterance => {
                    self.integration_inflight = false;
                    self.busy = false;
                    self.status =
                        result.map_or_else(|e| e, |_| "Previous insertion updated.".into());
                    self.overlay_message.clone_from(&self.status);
                    self.overlay_until = Some(Instant::now() + Duration::from_secs(5));
                }
                integration::Event::Inserted(id, result) if id == self.utterance => {
                    self.integration_inflight = false;
                    self.integration_pending = None;
                    self.busy = false;
                    self.status = match result {
                        Ok(true) => {
                            "Text inserted. Short corrections in this field will be remembered."
                                .into()
                        }
                        Ok(false) => "Text inserted. Ready for your next dictation.".into(),
                        Err(error) => {
                            // Messages describe the field, never the dictated text.
                            log::warn!(
                                "Insertion failed in {}: {error}",
                                self.dictation_app.as_deref().unwrap_or("an unknown app")
                            );
                            error
                        }
                    };
                    self.overlay_message.clone_from(&self.status);
                    self.overlay_until = Some(Instant::now() + Duration::from_secs(5));
                }
                integration::Event::Learned(entry, before) if self.settings.learn_corrections => {
                    self.remember(learning::bind_context(
                        entry,
                        &before,
                        &self.settings.entries,
                    ))
                }
                _ => {}
            }
        }
        if self.settings.learn_corrections
            && self.macro_used.is_none()
            && self.recording.is_none()
            && !self.busy
            && self
                .edited_at
                .is_some_and(|t| t.elapsed() >= Duration::from_secs(2))
        {
            self.edited_at = None;
            if let Some(mut entry) = learning::correction(&self.edit_baseline, &self.text) {
                entry.app.clone_from(&self.dictation_app);
                self.remember(learning::bind_context(
                    entry,
                    &self.edit_baseline,
                    &self.settings.entries,
                ));
            }
            self.edit_baseline.clone_from(&self.text);
        }
        if self
            .recording
            .as_ref()
            .is_some_and(|r| r.full.load(Ordering::Relaxed) || r.failed.load(Ordering::Relaxed))
        {
            self.toggle(None);
        }
        if self.hold_recording && self.shortcut_gesture.expired(Instant::now()) {
            if self.recording.is_some() {
                self.toggle(None);
            }
            self.hold_recording = false;
            self.shortcut_gesture = Default::default();
        }
        self.history_poll();
        self.calls_tick();
        self.updater.poll();
        self.poll_saved_search();
        self.polish_poll();
        self.desktop_assort_poll();
        self.brain_poll();
        self.notetaker_poll_filing();
        self.preview();
    }

    fn remember(&mut self, entry: Entry) {
        let Some(change) = learning::Change::apply(&mut self.settings.entries, entry) else {
            return;
        };
        self.overlay_message = format!(
            "{}: {} → {}. Undo in Articulate.",
            if change.updated_context() {
                "Updated context"
            } else if change.updated_existing() {
                "Updated correction"
            } else {
                "Remembered"
            },
            change.entry.heard,
            change.entry.wanted
        );
        self.overlay_until = Some(Instant::now() + Duration::from_secs(7));
        self.remembered.push(change);
        if self.remembered.len() > 5 {
            self.remembered.remove(0);
        }
        self.save();
    }

    fn download(&mut self) {
        self.downloading = Some(0.0);
        let cancel = Arc::new(AtomicBool::new(false));
        self.model_cancels
            .insert(model::ModelId::Speech, cancel.clone());
        self.status = "Downloading the speech model...".into();
        let tx = self.event_tx.clone();

        std::thread::spawn(move || {
            let mut last = Instant::now();
            let result = model::download_cancellable(&cancel, |p| {
                if last.elapsed() > Duration::from_millis(100) {
                    let _ = tx.send(Event::Progress(p));

                    last = Instant::now();
                }
            });
            let _ = tx.send(match result {
                Ok(p) => Event::Downloaded(p),
                Err(e) => Event::Error(format!("{e:#}")),
            });
        });
    }
}

impl App {
    fn refresh_call_notes(&mut self) {
        if self.call_notes.is_some() {
            self.call_notes = Some(crate::notes::Notes::build(&self.call_rows));
        }
    }

    fn install_saves_settled(&self) -> Result<bool, String> {
        if self.history.error.is_some() {
            return Err(
                "Some changes could not be saved. Retry saving in History before updating.".into(),
            );
        }
        self.history
            .worker
            .as_ref()
            .ok_or_else(|| {
                "Local history is unavailable. Save or export your text before updating.".to_owned()
            })?
            .saves_settled()
    }

    fn start_call_capture(&mut self) -> bool {
        let preference = (!self.settings.transcription_language.is_empty())
            .then(|| self.settings.transcription_language.clone());
        self.start_stream_capture(
            false,
            self.settings.discord_companion,
            true,
            preference,
            None,
        )
    }

    fn start_manual_call_capture(
        &mut self,
        separate_discord_audio: bool,
        language_preference: Option<String>,
        audio_context: Option<bool>,
    ) -> bool {
        self.start_stream_capture(
            false,
            separate_discord_audio,
            separate_discord_audio,
            language_preference,
            audio_context,
        )
    }

    fn start_stream_capture(
        &mut self,
        microphone_only: bool,
        separate_discord_audio: bool,
        discord_attribution: bool,
        language_preference: Option<String>,
        audio_context: Option<bool>,
    ) -> bool {
        if self.calls_file_job_running() {
            self.call_status = "Wait for the import or re-transcription to finish.".into();
            return false;
        }
        let native_audio = match calls::select_native_source(
            !microphone_only && separate_discord_audio,
            self.discord
                .as_ref()
                .is_some_and(|connection| connection.native_audio_ready()),
        ) {
            Ok(native_audio) => native_audio,
            Err(message) => {
                self.call_status = message.to_string();
                self.capture_failed_feedback(microphone_only, &message.to_string());
                self.call_tab = 2;
                return false;
            }
        };
        if !microphone_only {
            self.save();
        }
        self.history_save_call();
        if let Some(session) = &self.history.call {
            self.brain.remember(session);
        }
        self.history.call = None;
        self.history.call_deleted = false;
        self.call_tab = 0;
        self.call_rows.clear();
        self.call_committed.clear();
        self.call_search.clear();
        self.call_notes = None;
        self.speaker_names = calls::empty_speaker_names();
        let control = call_capture::Control::new();
        if !microphone_only && discord_attribution {
            control.set_discord(self.discord.clone());
        }
        self.cancel = CancelToken::new();
        self.call_native_audio = native_audio;
        self.call_native_received = false;
        self.context_status.clear();
        self.call_context.clear();
        self.call_tools.live_rules.clear();
        let kind = if microphone_only {
            crate::history::Kind::Note
        } else {
            crate::history::Kind::Call
        };
        let mut session = crate::history::Session::new(kind);
        if microphone_only {
            session.title = "Spoken note".into();
            session.auto_file_pending = true;
        }
        let keep_audio = self
            .call_tools
            .keep_audio
            .take()
            .unwrap_or(self.settings.keep_call_audio);
        let recorder = if keep_audio && !microphone_only {
            let remote = if native_audio { "participants" } else { "call" };
            self.calls_start_recorder(&mut session, vec!["microphone".into(), remote.into()])
        } else {
            None
        };
        let request = calls::Request {
            recorder,
            audio_context: audio_context.unwrap_or(self.settings.audio_context),
            language_preference,
            microphone: self.settings.microphone.clone(),
            output: if microphone_only {
                None
            } else {
                self.settings.output.clone()
            },
            cpu: self.settings.cpu,
            native_audio: self.call_native_audio,
            microphone_only,
            control: control.clone(),
        };
        let call_cues_enabled = request.audio_context;
        match self
            .commands
            .send(Command::Call(request, self.cancel.clone()))
        {
            Ok(()) => {
                self.call = Some(control);
                self.call_cues_enabled = call_cues_enabled;
                self.history.call = Some(session);
                self.capture_feedback.prepare(
                    if microphone_only {
                        capture_feedback::Kind::Note
                    } else {
                        capture_feedback::Kind::Call
                    },
                    self.history.call.as_ref().map(|session| session.id.clone()),
                );
                self.history_call_changed();
                self.call_status = if microphone_only {
                    "Preparing your microphone..."
                } else {
                    "Preparing call capture..."
                }
                .into();
                true
            }
            Err(_) => {
                if let Some(recorder) = self.call_tools.recorder.take() {
                    let _ = recorder.finish();
                    if let Some(path) = self.call_tools.audio_path(&session.id) {
                        let _ = std::fs::remove_file(path);
                    }
                }
                self.call_status = "The speech worker stopped. Restart the app.".into();
                self.capture_failed_feedback(
                    microphone_only,
                    "The speech worker stopped. Restart the app.",
                );
                false
            }
        }
    }

    fn call_row_is_provisional(&self, index: usize) -> bool {
        self.call_rows.get(index).is_some_and(|row| {
            self.call_committed.get(index).is_none_or(|committed| {
                row.text != committed.text
                    || row.start_ms != committed.start_ms
                    || row.end_ms != committed.end_ms
                    || row.microphone != committed.microphone
                    || row.speakers != committed.speakers
                    || row.discord != committed.discord
            })
        })
    }

    #[allow(
        dead_code,
        reason = "Call status classification retained for renderer status updates"
    )]
    fn call_live_label(&self) -> &'static str {
        if self
            .call
            .as_ref()
            .is_some_and(|control| control.stop_ns.load(Ordering::SeqCst) != 0)
        {
            "Finishing the transcript…"
        } else if (0..self.call_rows.len()).any(|index| self.call_row_is_provisional(index)) {
            "Refining the latest words"
        } else {
            "Listening"
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.history_save_personal_notes();
        if self.history.dictation_dirty.is_some() {
            self.history_save_dictation();
        }
        if self.history.call_dirty.is_some() {
            self.history_save_call();
        }
        self.insertion_cancel.cancel();
        let _ = self.integration_tx.send(integration::Action::Cancel);
        if let Some(control) = &self.call {
            control.abort.store(true, Ordering::Relaxed);
            control.stop();
        }
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn command_only_recording_routes_to_verified_previous_field_edit() {
        let (mut app, _) = app();
        let (tx, rx) = mpsc::channel();
        app.integration_tx = tx;
        app.target = Some(platform::test_target());
        app.settings.insert = true;
        app.recording_spoken_editing = true;
        app.event_tx
            .send(Event::Text(
                Ok("Scratch that.".into()),
                0.1,
                app.utterance,
                true,
            ))
            .unwrap();
        app.receive();
        assert!(matches!(
            rx.try_recv().unwrap(),
            integration::Action::EditPrevious { undo: false, .. }
        ));
        assert!(app.busy);
    }

    #[test]
    fn newer_settings_preserve_existing_preferences() {
        let settings: super::Settings = serde_json::from_value(serde_json::json!({
            "discord_pairing_key": "synthetic-pairing-value",
            "hotkey": {"key": "F8", "future": true},
            "hotkey_mode": "future",
            "polish_style": "future",
            "meetings": {"start": "future", "extra": 42},
            "input_profiles": [{"app": "editor.exe", "future": true, "style": "future",
                "line_break": "future", "hotkey_mode": "future"}],
            "styles": [{"app": "editor.exe", "style": "future", "extra": 1}]
        }))
        .unwrap();
        assert_eq!(settings.discord_pairing_key, "synthetic-pairing-value");
        assert_eq!(settings.hotkey.key, "F8");
        assert_eq!(settings.hotkey_mode, crate::platform::HotkeyMode::default());
        assert_eq!(
            settings.input_profiles[0].style,
            Some(crate::writing_style::WritingStyle::default())
        );
    }

    #[test]
    fn changing_editing_preference_does_not_drop_committed_text() {
        let (mut app, _) = app();
        app.recording_spoken_editing = false;
        app.committed_raw = "First sentence.".into();
        app.settings.spoken_editing_commands = true;
        app.finalized_text("Second sentence.".into());
        assert_eq!(app.raw, "First sentence. Second sentence.");
        app.recording_spoken_editing = true;
        app.dictation_editor = Default::default();
        app.finalized_text("First sentence.".into());
        app.committed_raw = app.raw.clone();
        app.settings.spoken_editing_commands = false;
        app.finalized_text("Second sentence.".into());
        assert_eq!(app.raw, "First sentence. Second sentence.");
    }
    use super::*;

    #[test]
    fn concurrent_preferences_saves_leave_complete_json_without_shared_temp_files() {
        let directory = std::env::temp_dir().join(format!(
            "articulate-save-race-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let barrier = std::sync::Barrier::new(16);
        std::thread::scope(|scope| {
            for index in 0..16 {
                let directory = &directory;
                let barrier = &barrier;
                scope.spawn(move || {
                    let settings = Settings {
                        transcription_language: format!("synthetic-{index}"),
                        ..Default::default()
                    };
                    barrier.wait();
                    for _ in 0..4 {
                        save_settings(directory, &settings).unwrap();
                    }
                });
            }
        });
        let settings: Settings =
            serde_json::from_slice(&std::fs::read(directory.join("settings.json")).unwrap())
                .unwrap();
        assert!(settings.transcription_language.starts_with("synthetic-"));
        let backup: Settings =
            serde_json::from_slice(&std::fs::read(directory.join("settings.json.bak")).unwrap())
                .unwrap();
        assert!(backup.transcription_language.starts_with("synthetic-"));
        // Only the file and its previous good copy remain; no shared temporaries.
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn settings_directory(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "articulate-settings-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn unreadable_settings_are_moved_aside_and_never_overwritten() {
        let directory = settings_directory("corrupt");
        let damaged = b"{\"insert\": tru".to_vec();
        std::fs::write(directory.join("settings.json"), &damaged).unwrap();
        let (settings, warning, _) = load_settings(&directory);
        let warning = warning.unwrap();
        assert!(warning.contains("settings.corrupt-"));
        assert!(warning.contains("default settings are in use"));
        assert!(!directory.join("settings.json").exists());
        let moved: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .flatten()
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.starts_with("settings.corrupt-") && name.ends_with(".json")
            })
            .collect();
        assert_eq!(moved.len(), 1);
        assert_eq!(std::fs::read(moved[0].path()).unwrap(), damaged);
        // A later automatic save cannot destroy the damaged original.
        save_settings(&directory, &settings).unwrap();
        assert_eq!(std::fs::read(moved[0].path()).unwrap(), damaged);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn damaged_settings_fall_back_to_the_previous_good_copy() {
        let directory = settings_directory("backup");
        let first = Settings {
            transcription_language: "synthetic-first".into(),
            ..Default::default()
        };
        save_settings(&directory, &first).unwrap();
        let second = Settings {
            transcription_language: "synthetic-second".into(),
            ..Default::default()
        };
        save_settings(&directory, &second).unwrap();
        std::fs::write(directory.join("settings.json"), b"\0garbage").unwrap();
        let (settings, warning, _) = load_settings(&directory);
        assert_eq!(settings.transcription_language, "synthetic-first");
        assert!(warning.unwrap().contains("previous settings were restored"));
        // A damaged file is never copied over the good backup.
        std::fs::write(directory.join("settings.json"), b"garbage").unwrap();
        save_settings(&directory, &settings).unwrap();
        let backup: Settings =
            serde_json::from_slice(&std::fs::read(directory.join("settings.json.bak")).unwrap())
                .unwrap();
        assert_eq!(backup.transcription_language, "synthetic-first");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn blocked_settings_refuse_to_save() {
        let directory = settings_directory("blocked");
        let settings = Settings {
            save_blocked: Some("Synthetic read failure".into()),
            ..Default::default()
        };
        assert!(save_settings(&directory, &settings).is_err());
        assert!(!directory.join("settings.json").exists());
        let (loaded, warning, _) = load_settings(&directory);
        assert!(warning.is_none() && loaded.save_blocked.is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn encrypted_settings_roundtrip_and_keep_secrets_off_disk() {
        let directory = settings_directory("encrypted");
        let settings = Settings {
            encrypt_at_rest: true,
            discord_pairing_key: "synthetic-pairing-secret".into(),
            ..Default::default()
        };
        save_settings(&directory, &settings).unwrap();
        save_settings(&directory, &settings).unwrap();
        for name in ["settings.json", "settings.json.bak"] {
            let raw = std::fs::read(directory.join(name)).unwrap();
            assert!(crate::history::protect::is_sealed(&raw));
            assert!(!String::from_utf8_lossy(&raw).contains("synthetic-pairing-secret"));
        }
        let (loaded, warning, _) = load_settings(&directory);
        assert!(warning.is_none());
        assert!(loaded.encrypt_at_rest);
        assert_eq!(loaded.discord_pairing_key, "synthetic-pairing-secret");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn missing_settings_are_restored_from_the_backup_and_reinstalled() {
        let directory = settings_directory("missing");
        let first = Settings {
            transcription_language: "synthetic-first".into(),
            ..Default::default()
        };
        save_settings(&directory, &first).unwrap();
        save_settings(&directory, &first).unwrap();
        std::fs::remove_file(directory.join("settings.json")).unwrap();
        let loaded = load_startup_settings(&directory);
        assert_eq!(loaded.settings.transcription_language, "synthetic-first");
        assert!(!loaded.protection_confirmed);
        assert!(
            loaded
                .warning
                .unwrap()
                .contains("previous settings were restored")
        );
        // The recovered copy is the main file again, so it survives a restart.
        let (again, warning, source) = load_settings(&directory);
        assert_eq!(source, SettingsSource::Main);
        assert!(warning.is_none());
        assert_eq!(again.transcription_language, "synthetic-first");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn startup_without_encrypted_data_keeps_the_loaded_choice() {
        let directory = settings_directory("plain-start");
        let loaded = load_startup_settings(&directory);
        assert!(!loaded.settings.encrypt_at_rest);
        assert!(!loaded.protection_confirmed);
        assert!(loaded.warning.is_none());
        save_settings(&directory, &Settings::default()).unwrap();
        let loaded = load_startup_settings(&directory);
        assert!(loaded.protection_confirmed && !loaded.settings.encrypt_at_rest);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn recovered_or_default_settings_never_turn_encryption_off() {
        use crate::history::protect;
        let directory = settings_directory("keep-protection");
        let history = directory.join("history");
        std::fs::create_dir_all(&history).unwrap();
        let id = crate::history::Session::new(crate::history::Kind::Note).id;
        std::fs::write(
            history.join(format!("{id}.json")),
            protect::seal(b"{\"synthetic\":true}").unwrap(),
        )
        .unwrap();
        // No preferences at all.
        let loaded = load_startup_settings(&directory);
        assert!(loaded.settings.encrypt_at_rest && !loaded.protection_confirmed);
        // Damaged preferences with an older plaintext backup that predates encryption.
        save_settings(&directory, &Settings::default()).unwrap();
        save_settings(&directory, &Settings::default()).unwrap();
        std::fs::write(directory.join("settings.json"), b"{damaged").unwrap();
        let loaded = load_startup_settings(&directory);
        assert!(loaded.settings.encrypt_at_rest && !loaded.protection_confirmed);
        assert!(loaded.warning.unwrap().contains("stays on"));
        // The reinstalled preferences keep encryption on at the next start.
        let (again, _, source) = load_settings(&directory);
        assert_eq!(source, SettingsSource::Main);
        assert!(again.encrypt_at_rest);
        assert!(protect::is_sealed(
            &std::fs::read(directory.join("settings.json")).unwrap()
        ));
        // Preferences that cannot be decrypted are encrypted data too.
        std::fs::remove_dir_all(&history).unwrap();
        for entry in std::fs::read_dir(&directory).unwrap().flatten() {
            std::fs::remove_file(entry.path()).unwrap();
        }
        let mut sealed = protect::seal(b"{}").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0xff;
        std::fs::write(directory.join("settings.json"), sealed).unwrap();
        let loaded = load_startup_settings(&directory);
        assert!(loaded.settings.encrypt_at_rest && !loaded.protection_confirmed);
        // A successfully read choice is confirmed, so an interrupted change completes.
        for entry in std::fs::read_dir(&directory).unwrap().flatten() {
            std::fs::remove_file(entry.path()).unwrap();
        }
        save_settings(&directory, &Settings::default()).unwrap();
        std::fs::create_dir_all(&history).unwrap();
        std::fs::write(
            history.join(format!("{id}.json")),
            protect::seal(b"{}").unwrap(),
        )
        .unwrap();
        let loaded = load_startup_settings(&directory);
        assert!(loaded.protection_confirmed && !loaded.settings.encrypt_at_rest);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn encryption_changes_update_every_settings_copy() {
        use crate::history::protect;
        let directory = settings_directory("copies");
        save_settings(&directory, &Settings::default()).unwrap();
        save_settings(&directory, &Settings::default()).unwrap();
        // The main file is unreadable, so a save cannot rebuild the backup from it.
        std::fs::write(directory.join("settings.json"), b"{damaged").unwrap();
        std::fs::write(
            directory.join("settings.corrupt-1.json"),
            b"{\"isolated\":1}",
        )
        .unwrap();
        std::fs::write(
            directory.join("settings-999999-0.tmp"),
            b"{\"abandoned\":1}",
        )
        .unwrap();
        std::fs::write(
            directory.join(".settings.json.bak.999999-0.partial"),
            b"{\"abandoned\":2}",
        )
        .unwrap();
        let protected = Settings {
            encrypt_at_rest: true,
            ..Default::default()
        };
        save_settings(&directory, &protected).unwrap();
        let report = migrate_settings_copies(&directory, true);
        assert_eq!(report.failed, 0);
        let names: Vec<String> = std::fs::read_dir(&directory)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 3, "{names:?}");
        for name in [
            "settings.json",
            "settings.json.bak",
            "settings.corrupt-1.json",
        ] {
            assert!(
                protect::is_sealed(&std::fs::read(directory.join(name)).unwrap()),
                "{name}"
            );
        }
        let report = migrate_settings_copies(&directory, false);
        assert_eq!((report.changed, report.failed), (2, 0));
        assert_eq!(
            std::fs::read(directory.join("settings.corrupt-1.json")).unwrap(),
            b"{\"isolated\":1}"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn restored_settings_keep_this_devices_discord_state() {
        let current = Settings {
            discord_debugger_port: 9229,
            discord_flavor: "ptb".into(),
            microphone: Some("Local microphone".into()),
            ..Default::default()
        };
        let merged = merge_restored_settings(
            &current,
            Some(serde_json::json!({
                "discord_debugger_port": 1234,
                "discord_flavor": "canary",
                "microphone": "Archived microphone",
                "transcription_language": "fr",
            })),
            None,
        )
        .unwrap();
        assert_eq!(merged.discord_debugger_port, 9229);
        assert_eq!(merged.discord_flavor, "ptb");
        assert_eq!(merged.microphone.as_deref(), Some("Local microphone"));
        assert_eq!(merged.transcription_language, "fr");
    }

    #[test]
    fn restored_settings_keep_local_secrets_and_merge_vocabulary() {
        let current = Settings {
            discord_pairing_key: "local-secret".into(),
            model_path: "C:/local/model.gguf".into(),
            ..Default::default()
        };
        let entry = dictionary::validate("Jon", "John").unwrap();
        let library = crate::library::export(&[entry], &[]).unwrap();
        let merged = merge_restored_settings(
            &current,
            Some(serde_json::json!({
                "discord_pairing_key": "archived",
                "model_path": "C:/elsewhere.gguf",
                "transcription_language": "fr",
            })),
            Some(&library),
        )
        .unwrap();
        assert_eq!(merged.discord_pairing_key, "local-secret");
        assert_eq!(merged.model_path, "C:/local/model.gguf");
        assert_eq!(merged.transcription_language, "fr");
        assert_eq!(merged.entries.len(), 1);
    }

    fn call_row(speaker: i32, start_ms: u64, text: &str) -> calls::Row {
        calls::Row {
            cues: Vec::new(),
            start_ms,
            end_ms: start_ms + 1000,
            microphone: false,
            speakers: vec![speaker],
            discord: None,
            text: text.into(),
        }
    }

    fn call_event(app: &mut App, update: calls::Update) {
        app.event_tx.send(Event::Call(update)).unwrap();
        app.receive();
    }

    #[test]
    fn sound_cues_survive_preview_revision_and_save_without_replacing_words() {
        let (mut app, _) = app();
        app.call = Some(call_capture::Control::new());
        app.history.call = Some(crate::history::Session::new(crate::history::Kind::Call));
        let first = call_row(1, 1000, "A draft.");
        call_event(&mut app, calls::Update::Preview(vec![first.clone()]));
        let mut annotation = first;
        annotation.text.clear();
        annotation.cues.push(crate::sensevoice::Cue {
            start_ms: 1000,
            end_ms: 1500,
            label: "Laughing".into(),
        });
        call_event(&mut app, calls::Update::AudioContext(annotation));
        assert!(app.call_committed.is_empty());
        assert_eq!(app.call_rows[0].text, "A draft.");
        call_event(
            &mut app,
            calls::Update::Rows(vec![call_row(1, 1000, "Final words.")]),
        );
        assert_eq!(app.call_rows.len(), 1);
        assert_eq!(app.call_rows[0].cues.len(), 1);
        call_event(&mut app, calls::Update::Preview(Vec::new()));
        app.history_save_call();
        let saved = app.history.call.as_ref().unwrap();
        assert_eq!(saved.rows[0].text, "Final words.");
        assert_eq!(saved.rows[0].cues[0].label, "Laughing");
    }

    #[test]
    fn call_audio_gap_is_cumulative_and_belongs_only_to_the_active_call() {
        let (mut app, _) = app();
        app.call = Some(call_capture::Control::new());
        app.history.call = Some(crate::history::Session::new(crate::history::Kind::Call));
        app.history.selected = Some(crate::history::Session::new(crate::history::Kind::Call));
        app.history.dictation = Some(crate::history::Session::new(
            crate::history::Kind::Dictation,
        ));
        call_event(&mut app, calls::Update::AudioGap(3));
        call_event(&mut app, calls::Update::AudioGap(2));
        assert_eq!(app.history.call.as_ref().unwrap().audio_packets_lost, 3);
        assert_eq!(app.history.selected.as_ref().unwrap().audio_packets_lost, 0);
        assert_eq!(
            app.history.dictation.as_ref().unwrap().audio_packets_lost,
            0
        );
        assert!(app.history.call_dirty.is_some());
        app.history.selected = app.history.call.clone();
        call_event(&mut app, calls::Update::AudioGap(4));
        assert_eq!(app.history.selected.as_ref().unwrap().audio_packets_lost, 4);
        app.history.call = Some(crate::history::Session::new(crate::history::Kind::Call));
        assert_eq!(app.history.call.as_ref().unwrap().audio_packets_lost, 0);
        app.call = None;
        call_event(&mut app, calls::Update::AudioGap(9));
        assert_eq!(app.history.call.as_ref().unwrap().audio_packets_lost, 0);
    }

    #[test]
    fn call_revisions_replace_preview_and_commit_without_duplicates() {
        let (mut app, _) = app();
        call_event(
            &mut app,
            calls::Update::Rows(vec![call_row(1, 0, "Opening remarks.")]),
        );
        call_event(
            &mut app,
            calls::Update::Preview(vec![call_row(2, 1000, "Please send the wrong draft.")]),
        );
        call_event(
            &mut app,
            calls::Update::Preview(vec![call_row(2, 1000, "Please send the revised draft.")]),
        );
        assert_eq!(app.call_rows.len(), 2);
        assert_eq!(app.call_committed.len(), 1);
        assert_eq!(app.call_rows[1].text, "Please send the revised draft.");
        call_event(
            &mut app,
            calls::Update::Rows(vec![call_row(2, 1000, "Please send the final draft.")]),
        );
        assert_eq!(app.call_rows.len(), 2);
        assert_eq!(app.call_committed.len(), 2);
        assert_eq!(app.call_rows[1].text, "Please send the final draft.");
        call_event(&mut app, calls::Update::Preview(Vec::new()));
        assert_eq!(app.call_rows.len(), 2);
        assert_eq!(app.call_rows[1].text, "Please send the final draft.");
    }

    #[test]
    fn empty_call_preview_removes_only_uncommitted_text() {
        let (mut app, _) = app();
        call_event(
            &mut app,
            calls::Update::Rows(vec![call_row(1, 0, "Keep this.")]),
        );
        call_event(
            &mut app,
            calls::Update::Preview(vec![call_row(1, 1000, "Temporary wording.")]),
        );
        assert_eq!(app.call_rows[0].text, "Keep this. Temporary wording.");
        assert!(app.call_row_is_provisional(0));
        assert_eq!(app.call_live_label(), "Refining the latest words");
        call_event(&mut app, calls::Update::Preview(Vec::new()));
        assert_eq!(app.call_rows[0].text, "Keep this.");
        assert_eq!(app.call_committed[0].text, "Keep this.");
        assert!(!app.call_row_is_provisional(0));
        assert_eq!(app.call_live_label(), "Listening");
    }

    #[test]
    fn live_call_state_reports_finalization_before_preview() {
        let (mut app, _) = app();
        let control = call_capture::Control::new();
        app.call = Some(control.clone());
        assert_eq!(app.call_live_label(), "Listening");
        call_event(
            &mut app,
            calls::Update::Preview(vec![call_row(1, 0, "Still speaking.")]),
        );
        assert_eq!(app.call_live_label(), "Refining the latest words");
        control.stop_ns.store(1, Ordering::SeqCst);
        assert_eq!(app.call_live_label(), "Finishing the transcript…");
    }

    #[test]
    fn revised_call_notes_remain_valid_for_history_after_preview_shrinks() {
        let (mut app, _) = app();
        call_event(
            &mut app,
            calls::Update::Rows(vec![call_row(1, 0, "We agreed to send the report.")]),
        );
        call_event(
            &mut app,
            calls::Update::Preview(vec![
                call_row(2, 1000, "Please schedule the team review."),
                call_row(3, 2000, "I will send the invoices tomorrow."),
            ]),
        );
        app.call_notes = Some(crate::notes::Notes::build(&app.call_rows));
        assert!(
            app.call_notes
                .as_ref()
                .unwrap()
                .actions
                .iter()
                .any(|quote| quote.row > 0)
        );
        call_event(&mut app, calls::Update::Preview(Vec::new()));
        app.history_save_call();
        let session = app.history.call.clone().unwrap();
        assert_eq!(session.rows.len(), 1);
        assert!(
            session
                .notes
                .as_ref()
                .unwrap()
                .highlights
                .iter()
                .chain(&session.notes.as_ref().unwrap().actions)
                .all(|quote| quote.row < session.rows.len()
                    && session.rows[quote.row].text.contains(&quote.text))
        );
        let temp_root = std::env::temp_dir();
        let directory = temp_root.join(format!("articulate-notes-test-{}", session.id));
        assert_eq!(directory.parent(), Some(temp_root.as_path()));
        let history = crate::history::History::open(directory.clone()).unwrap();
        history
            .save(session)
            .expect("Regenerated quotes must pass real history validation");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn old_preferences_leave_companion_off_and_update_requires_history() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.hotkey_mode, platform::HotkeyMode::Hold);
        assert_eq!(settings.quick_note_hotkey.label(), "Ctrl + Alt + N");
        assert!(settings.audio_feedback);
        let explicit: Settings =
            serde_json::from_str(r#"{"hotkey_mode":"Toggle","audio_feedback":false}"#).unwrap();
        assert_eq!(explicit.hotkey_mode, platform::HotkeyMode::Toggle);
        assert!(!explicit.audio_feedback);
        assert!(!settings.discord_companion);
        assert!(settings.discord_pairing_key.is_empty());
        let (mut app, _) = app();
        assert!(app.discord.is_none());
        assert!(app.install_saves_settled().is_err());
        app.history.error = Some("Synthetic save failure".into());
        assert!(
            app.install_saves_settled()
                .unwrap_err()
                .contains("Retry saving")
        );
    }
    #[test]
    fn shortcut_modes_preserve_defaults_and_release_cannot_start_audio() {
        let (mut app, commands) = app();
        app.settings.hotkey_mode = platform::HotkeyMode::Hold;
        app.settings.hotkey.key = "F8".into();
        assert_eq!(
            app.shortcut_instruction(),
            format!("Hold {} to speak", app.settings.hotkey.label())
        );
        let settings: Settings =
            serde_json::from_slice(&serde_json::to_vec(&app.settings).unwrap()).unwrap();
        assert_eq!(settings.hotkey_mode, platform::HotkeyMode::Hold);
        app.release_shortcut(Instant::now());
        assert!(app.recording.is_none() && commands.try_recv().is_err());
        app.hold_recording = true;
        app.release_shortcut(Instant::now());
        assert!(!app.hold_recording && app.recording.is_none());
        app.ready = false;
        app.shortcut(platform::test_target(), Instant::now());
        assert!(!app.hold_recording && app.recording.is_none());
        assert!(app.overlay_message.contains("set up your speech model"));
    }

    #[test]
    fn failed_recording_releases_hold_ownership() {
        let (mut app, commands) = app();
        app.hold_recording = true;
        app.chunk_inflight = true;
        app.event_tx
            .send(Event::Text(
                Err("Synthetic failure".into()),
                0.0,
                app.utterance,
                true,
            ))
            .unwrap();
        app.receive();
        assert!(!app.hold_recording);
        assert!(app.recording.is_none());
        app.release_shortcut(Instant::now());
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn companion_without_audio_does_not_start_mixed_capture_or_clear_transcript() {
        let (mut app, commands) = app();
        app.settings.discord_companion = true;
        app.call_rows
            .push(call_row(1, 0, "A saved synthetic sentence."));
        assert!(!app.start_call_capture());
        assert_eq!(app.call_rows.len(), 1);
        assert_eq!(app.call_tab, 2);
        assert!(app.call.is_none());
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn manual_system_call_starts_without_discord_audio_even_when_companion_is_selected() {
        let (mut app, commands) = app();
        app.settings.discord_companion = true;
        assert!(app.start_manual_call_capture(false, Some("en".into()), Some(true)));
        assert!(!app.call_native_audio);
        assert!(app.call.is_some());
        assert!(app.call.as_ref().unwrap().discord().is_none());
        assert!(
            matches!(commands.try_recv(), Ok(Command::Call(request, _)) if !request.native_audio && request.audio_context && request.language_preference.as_deref() == Some("en"))
        );
    }

    #[test]
    fn automatic_call_inherits_saved_language_preference_and_audio_cues() {
        let (mut app, commands) = app();
        app.settings.transcription_language = "en".into();
        app.settings.audio_context = true;
        assert!(app.start_call_capture());
        assert!(app.call_cues_enabled);
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::Call(request, _))
                if request.language_preference.as_deref() == Some("en")
                    && request.audio_context
        ));
    }

    #[test]
    fn shortcut_explains_unavailable_dictation_without_starting_audio() {
        let (mut app, _) = app();
        app.loading = true;
        app.shortcut(platform::test_target(), Instant::now());
        assert!(app.overlay_message.contains("getting ready"));
        assert!(app.overlay_until.is_some());
        assert!(app.recording.is_none());
        app.loading = false;
        app.busy = true;
        app.shortcut(platform::test_target(), Instant::now());
        assert!(app.overlay_message.contains("Finishing"));
        assert!(app.recording.is_none());
        app.busy = false;
        app.ready = false;
        app.shortcut(platform::test_target(), Instant::now());
        assert!(app.overlay_message.contains("Settings"));
        assert!(app.recording.is_none());
    }

    pub(super) fn app() -> (App, Receiver<Command>) {
        let (commands, receiver) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let (_, hotkeys) = mpsc::channel();
        let (hotkey_tx, _) = mpsc::channel();
        let (_, quick_note_hotkeys) = mpsc::channel();
        let (quick_note_hotkey_tx, _) = mpsc::channel();
        let (integration_tx, _) = mpsc::channel();
        let (_, integration_rx) = mpsc::channel();
        (
            App {
                polish: Default::default(),

                pending_install: None,
                history: history_ui::State::default(),
                call_tools: Default::default(),
                insights: Default::default(),
                saved_search: Default::default(),
                brain: Default::default(),
                notetaker: Default::default(),
                capture_feedback: Default::default(),
                updater: Default::default(),
                context_progress: None,
                context_status: String::new(),
                call_context: Vec::new(),
                speakers_downloading: None,
                speakers_download_status: String::new(),
                verifier_downloading: None,
                verifier_download_status: String::new(),
                dictation_metrics: Default::default(),
                settings: Settings::default(),
                commands,
                events,
                event_tx,
                hotkeys,
                hotkey_tx,
                hotkey_draft: platform::Hotkey::default(),
                hotkey_pending: false,
                quick_note_hotkeys,
                quick_note_hotkey_tx,
                quick_note_hotkey_pending: false,
                quick_note_shortcut_error: None,
                hold_recording: false,
                shortcut_gesture: Default::default(),

                call_search: String::new(),
                call_tab: 0,
                cancel: CancelToken::new(),
                recording: None,
                target: None,
                loading: false,
                busy: false,
                ready: true,
                downloading: None,
                model_cancels: Default::default(),
                status: String::new(),
                backend: "CPU".into(),
                speech_languages: vec!["en".into(), "nl".into()],
                text: String::new(),
                raw: String::new(),
                elapsed: None,
                words_changed: 0,

                page: 0,
                utterance: 1,
                preview_inflight: true,
                preview_audio_seconds: 0.0,
                capture_interruption: None,
                dictation_editor: Default::default(),
                recording_spoken_editing: false,
                last_dictation_text: String::new(),
                microphone_check: Default::default(),
                pending_final: None,
                cleanup_count: 0,
                microphones: Vec::new(),
                shortcut_error: None,
                committed_raw: String::new(),
                chunk_inflight: false,
                call: None,
                discord: None,

                discord_launch: Default::default(),
                call_rows: Vec::new(),
                call_committed: Vec::new(),
                speaker_names: calls::empty_speaker_names(),
                outputs: Vec::new(),
                call_levels: (0.0, 0.0),
                call_status: String::new(),
                call_native_audio: false,
                call_native_received: false,
                call_cues_enabled: false,
                integration_tx,
                integration_rx,
                edit_baseline: String::new(),
                edited_at: None,
                remembered: Vec::new(),
                overlay_until: None,
                overlay_message: String::new(),
                live_mode: false,
                insertion_cancel: CancelToken::new(),
                integration_inflight: false,
                integration_pending: None,
                dictation_app: None,
                macro_used: None,

                call_notes: None,

                last_seconds: 0,
            },
            receiver,
        )
    }

    #[test]
    fn automatic_cleanup_reaches_live_and_final_insertion_without_polish() {
        let (mut app, _) = app();
        let (tx, rx) = mpsc::channel();
        let (ack, events) = mpsc::channel();
        app.integration_tx = tx;
        app.integration_rx = events;
        app.target = Some(platform::test_target());
        app.settings.insert = true;
        app.settings.clean_speech = true;
        app.live_mode = true;
        app.dictation_app = Some("discord.exe".into());
        let raw = "So the um invoices are unpaid. Um, when will you pay them?";
        let expected = "So the invoices are unpaid. When will you pay them?";
        app.event_tx
            .send(Event::Text(Ok(raw.into()), 0.1, 1, false))
            .unwrap();
        app.receive();
        assert!(
            matches!(rx.try_recv().unwrap(), integration::Action::Preview { text, .. } if text == expected)
        );
        ack.send(integration::Event::Previewed(1, Ok(()))).unwrap();
        app.receive();
        app.event_tx
            .send(Event::Text(Ok(raw.into()), 0.1, 1, true))
            .unwrap();
        app.receive();
        assert!(
            matches!(rx.try_recv().unwrap(), integration::Action::Insert { text, .. } if text == expected)
        );
        assert_eq!(app.raw, raw);
        assert_eq!(app.edit_baseline, expected);
        assert_eq!(app.history.dictation.as_ref().unwrap().text, expected);
        assert!(app.cleanup_count >= 2);
        assert!(!app.polish_working());
    }

    #[test]
    fn automatic_cleanup_respects_verbatim_and_vocabulary() {
        let (mut app, _) = app();
        let raw = "Um, we we should send it.";
        app.settings.clean_speech = false;
        app.update_text(raw.into());
        assert_eq!(app.text, raw);
        app.settings.clean_speech = true;
        app.settings
            .entries
            .push(dictionary::validate("um", "UM Research").unwrap());
        app.update_text("Ask um to review it.".into());
        assert_eq!(app.text, "Ask UM Research to review it.");
    }

    #[test]
    fn macro_waits_for_final_result_and_literal_text_is_not_relearned() {
        let (mut app, _) = app();
        let (tx, rx) = mpsc::channel();
        let (ack, events) = mpsc::channel();
        app.integration_tx = tx;
        app.integration_rx = events;
        app.target = Some(platform::test_target());
        app.settings.insert = true;
        app.live_mode = true;
        app.settings
            .macros
            .push(macros::validate("reply", "Thanks. {text}", "editor.exe").unwrap());
        app.dictation_app = Some("editor.exe".into());
        app.settings
            .entries
            .push(dictionary::validate("Thanks", "Wrong").unwrap());
        for (speech, final_pass) in [
            ("Bang reply", false),
            ("Bang reply I will check tomorrow.", true),
        ] {
            app.event_tx
                .send(Event::Text(Ok(speech.into()), 0.1, 1, final_pass))
                .unwrap();
            app.receive();
        }
        assert!(
            matches!(rx.try_recv().unwrap(), integration::Action::Preview {text, ..} if text.is_empty())
        );
        ack.send(integration::Event::Previewed(1, Ok(()))).unwrap();
        app.receive();
        assert!(
            matches!(rx.try_recv().unwrap(), integration::Action::Insert {text, learn: false, ..} if text == "Thanks. I will check tomorrow.")
        );
        assert_eq!(app.raw, "Bang reply I will check tomorrow.");
        assert_eq!(app.macro_used.as_deref(), Some("reply"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn incomplete_macro_does_not_insert_and_chunk_commit_does_not_expand() {
        let (mut app, _) = app();
        let (tx, rx) = mpsc::channel();
        app.integration_tx = tx;
        app.settings
            .macros
            .push(macros::validate("reply", "Thanks. {text}", "").unwrap());
        app.target = Some(platform::test_target());
        app.settings.insert = true;
        app.chunk_inflight = true;
        app.event_tx
            .send(Event::Text(Ok("Bang reply".into()), 0.1, 1, true))
            .unwrap();
        app.receive();
        assert!(app.macro_used.is_none());
        assert!(app.target.is_some());
        app.event_tx
            .send(Event::Text(Ok("".into()), 0.1, 1, true))
            .unwrap();
        app.receive();
        assert!(app.target.is_none());
        assert!(app.status.contains("followed by"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn legacy_settings_preserve_learned_mentions_and_enable_new_defaults() {
        let settings: Settings = serde_json::from_str(
            r#"{"insert":true,"entries":[{"heard":"at ExampleHandle","wanted":"@ExampleHandle"}]}"#,
        )
        .unwrap();
        assert!(settings.macros.is_empty());
        assert!(settings.insert && settings.learn_corrections && settings.live_insert);
        assert_eq!(
            dictionary::apply_in(
                "Hello at ExampleHandle.",
                &settings.entries,
                Some("editor.exe")
            )
            .0,
            "Hello @ExampleHandle."
        );
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(restored.entries[0].wanted, "@ExampleHandle");
    }

    #[test]
    fn live_updates_coalesce_and_final_text_includes_learned_mentions() {
        let (mut app, _) = app();
        let (tx, rx) = mpsc::channel();
        let (ack, events) = mpsc::channel();
        app.integration_tx = tx;
        app.integration_rx = events;
        app.target = Some(platform::test_target());
        app.settings.insert = true;
        app.live_mode = true;
        app.settings
            .entries
            .push(dictionary::validate("at ExampleHandle", "@ExampleHandle").unwrap());
        for (text, final_pass) in [
            ("Hello at Ex", false),
            ("Hello at ExampleHandle", false),
            ("Hello at ExampleHandle.", true),
        ] {
            app.event_tx
                .send(Event::Text(Ok(text.into()), 0.1, 1, final_pass))
                .unwrap();
            app.receive();
        }
        assert!(
            matches!(rx.try_recv().unwrap(), integration::Action::Preview { text, .. } if text == "Hello at Ex")
        );
        assert!(rx.try_recv().is_err());
        ack.send(integration::Event::Previewed(1, Ok(()))).unwrap();
        app.receive();
        assert!(
            matches!(rx.try_recv().unwrap(), integration::Action::Insert { text, .. } if text == "Hello @ExampleHandle.")
        );
        assert!(rx.try_recv().is_err());
        ack.send(integration::Event::Inserted(1, Ok(true))).unwrap();
        app.receive();
        assert!(!app.busy);
    }

    #[test]
    fn disrupted_live_update_does_not_append_the_final_transcript_again() {
        let (mut app, _) = app();
        let (tx, rx) = mpsc::channel();
        let (ack, events) = mpsc::channel();
        app.integration_tx = tx;
        app.integration_rx = events;
        app.target = Some(platform::test_target());
        app.settings.insert = true;
        app.live_mode = true;
        for final_pass in [false, true] {
            app.event_tx
                .send(Event::Text(Ok("Hello.".into()), 0.1, 1, final_pass))
                .unwrap();
            app.receive();
        }
        assert!(matches!(
            rx.try_recv().unwrap(),
            integration::Action::Preview { .. }
        ));
        ack.send(integration::Event::Previewed(
            1,
            Err("Caret changed".into()),
        ))
        .unwrap();
        app.receive();
        assert!(rx.try_recv().is_err());
        assert_eq!(app.text, "Hello.");
        assert!(!app.busy);
        assert!(app.target.is_none() && app.insertion_cancel.is_cancelled());
    }

    #[test]
    fn interrupted_final_preserves_text_and_reports_incomplete() {
        let (mut app, _) = app();
        app.capture_interruption = Some("Incomplete transcript: microphone interrupted".into());
        app.target = None;
        app.busy = true;
        app.event_tx
            .send(Event::Text(
                Ok("Valid captured prefix.".into()),
                0.2,
                1,
                true,
            ))
            .unwrap();
        app.receive();
        assert_eq!(app.text, "Valid captured prefix.");
        assert!(app.status.starts_with("Incomplete transcript"));
        assert!(!app.busy);
    }
    #[test]
    fn stale_preview_cannot_replace_new_utterance() {
        let (mut app, _rx) = app();
        app.utterance = 2;
        app.text = "New recording".into();
        app.event_tx
            .send(Event::Text(Ok("Old recording".into()), 0.2, 1, false))
            .unwrap();
        app.receive();
        assert_eq!(app.text, "New recording");
        assert!(app.preview_inflight);
    }

    #[test]
    fn explicit_cancel_does_not_restart_pending_final_or_publish_late_chunks() {
        let (mut app, commands) = app();
        app.capture_interruption = Some("Interrupted capture".into());
        let (tx, actions) = mpsc::channel();
        app.integration_tx = tx;
        app.busy = true;
        app.chunk_inflight = true;
        app.pending_final = Some(vec![0.2; 32000]);
        app.text = "Visible preview".into();
        let cancelled_id = app.utterance;
        app.cancel_dictation();
        assert!(app.capture_interruption.is_none());
        assert!(app.cancel.is_cancelled() && app.insertion_cancel.is_cancelled());
        assert!(matches!(
            actions.try_recv().unwrap(),
            integration::Action::Cancel
        ));
        app.event_tx
            .send(Event::Text(
                Ok("Late chunk".into()),
                0.1,
                cancelled_id,
                true,
            ))
            .unwrap();
        app.receive();
        assert_eq!(app.text, "Visible preview");
        assert!(app.pending_final.is_none());
        assert!(commands.try_recv().is_err() && actions.try_recv().is_err());
        assert!(!app.busy && !app.preview_inflight);

        // A new job may already be pending when an old result arrives.
        app.pending_final = Some(vec![0.1; 16000]);
        app.preview_inflight = true;
        app.chunk_inflight = true;
        app.event_tx
            .send(Event::Text(
                Ok("Old preview".into()),
                0.1,
                cancelled_id,
                false,
            ))
            .unwrap();
        app.receive();
        assert!(app.preview_inflight && app.chunk_inflight && app.pending_final.is_some());
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn explicit_cancel_stops_insertion_and_ignores_its_late_acknowledgement() {
        let (mut app, _) = app();
        let (ack, events) = mpsc::channel();
        app.integration_rx = events;
        let token = app.insertion_cancel.clone();
        let id = app.utterance;
        app.busy = true;
        app.integration_inflight = true;
        app.integration_pending = Some(integration::Action::Insert {
            id,
            target: platform::test_target(),
            text: "Pending text".into(),
            learn: false,
            cancel: token.clone(),
        });
        app.cancel_dictation();
        assert!(token.is_cancelled());
        assert!(app.integration_pending.is_none());
        ack.send(integration::Event::Inserted(id, Ok(false)))
            .unwrap();
        app.receive();
        assert!(app.status.starts_with("Transcription cancelled"));
        assert!(!app.busy);
    }

    #[test]
    fn finish_waits_for_preview_then_checks_complete_audio() {
        let (mut app, rx) = app();
        app.busy = true;
        app.pending_final = Some(vec![0.2; 32000]);
        app.cancel.cancel();
        app.event_tx
            .send(Event::Text(Err("cancelled".into()), 0.2, 1, false))
            .unwrap();
        app.receive();
        let Command::Transcribe(pcm, id, final_pass, cancel) = rx.try_recv().unwrap() else {
            panic!("Expected final decode")
        };
        assert_eq!(pcm.len(), 32000);
        assert!(final_pass && !cancel.is_cancelled());
        assert_eq!(id, 1);
        app.event_tx
            .send(Event::Text(Ok("I I agree.".into()), 0.2, 1, true))
            .unwrap();
        app.receive();
        assert_eq!(app.text, "I agree.");
        assert_eq!(app.raw, "I I agree.");
        assert!(!app.busy);
    }

    #[test]
    fn live_corrections_are_recomputed_from_raw_not_cascaded() {
        let (mut app, _rx) = app();
        app.settings
            .entries
            .push(dictionary::validate("flow", "Flow").unwrap());
        for _ in 0..2 {
            app.event_tx
                .send(Event::Text(Ok("I I use flow.".into()), 0.1, 1, false))
                .unwrap();
            app.receive();
            assert_eq!(app.raw, "I I use flow.");
            assert_eq!(app.text, "I use Flow.");
            assert_eq!(app.cleanup_count + app.words_changed, 2);
        }
    }

    #[test]
    fn stopping_during_window_commit_keeps_both_completed_and_tail_text() {
        let (mut app, rx) = app();
        app.chunk_inflight = true;
        app.busy = true;
        app.pending_final = Some(vec![0.2; 16000]);
        app.event_tx
            .send(Event::Text(Ok("First section.".into()), 0.1, 1, true))
            .unwrap();
        app.receive();
        assert_eq!(app.committed_raw, "First section.");
        assert!(matches!(
            rx.try_recv().unwrap(),
            Command::Transcribe(_, 1, true, _)
        ));
        app.event_tx
            .send(Event::Text(Ok("Final section.".into()), 0.1, 1, true))
            .unwrap();
        app.receive();
        assert_eq!(app.text, "First section. Final section.");
        assert!(!app.busy);
    }
    #[test]
    fn failed_audio_import_does_not_duplicate_previous_dictation() {
        let (mut app, commands) = app();
        let temporary = std::env::temp_dir();
        let directory = temporary.join(format!(
            "articulate-failed-import-{}",
            crate::history::Session::new(crate::history::Kind::Dictation).id
        ));
        assert_eq!(directory.parent(), Some(temporary.as_path()));
        app.history.worker = Some(crate::history::Worker::test_directory(directory.clone()));
        app.text = "Previously saved words.".into();
        app.raw = app.text.clone();
        app.dictation_metrics.recognized_words = Some(3);
        app.history_save_dictation();
        let previous_id = app.history.dictation.as_ref().unwrap().id.clone();
        app.transcribe(vec![0.0; 3200]);
        assert!(matches!(
            commands.try_recv().unwrap(),
            Command::Transcribe(_, _, true, _)
        ));
        assert!(app.text.is_empty() && app.raw.is_empty());
        app.event_tx
            .send(Event::Text(
                Err("Synthetic recognition failure".into()),
                0.1,
                app.utterance,
                true,
            ))
            .unwrap();
        app.receive();
        app.history_save_dictation();
        assert!(app.history.dictation.is_none());
        drop(app);
        let history = crate::history::History::open(directory.clone()).unwrap();
        assert_eq!(history.list().unwrap().len(), 1);
        let saved = history.load(&previous_id).unwrap();
        assert_eq!(saved.text, "Previously saved words.");
        assert_eq!(saved.metrics.recognized_words, Some(3));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
