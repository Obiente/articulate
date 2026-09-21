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
        atomic::Ordering,
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};
use transcribe_cpp::CancelToken;

mod assort_ui;
mod brain_ui;
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
    discord_companion: bool,
    discord_pairing_key: String,
    #[serde(default = "enabled")]
    discord_auto_connect: bool,
    discord_auto_transcribe: bool,
    discord_standard_configured: bool,
    vencord_source: String,
    vencord_auto_update: bool,
    assort: assort_ui::Configuration,
    polish_style: crate::polish::Style,
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
            discord_companion: false,
            discord_pairing_key: String::new(),
            discord_auto_connect: true,
            discord_auto_transcribe: false,
            discord_standard_configured: false,
            vencord_source: String::new(),
            vencord_auto_update: false,
            assort: Default::default(),
            polish_style: Default::default(),
        }
    }
}

enum Command {
    Load(PathBuf, bool, Option<String>),
    Transcribe(Vec<f32>, u64, bool, CancelToken),
    Call(calls::Request, CancelToken),
}
enum Event {
    Loaded(String, Vec<String>),
    Text(Result<String, String>, f32, u64, bool),
    Error(String),
    Progress(f32),
    Downloaded(PathBuf),
    Call(calls::Update),
    CallFinished(Result<(), String>),
    #[allow(
        dead_code,
        reason = "Speaker model download event retained for model management"
    )]
    ContextProgress(f32),
    ContextInstalled(Result<(), String>),
    SpeakersDownloaded,
    SpeakersProgress(f32),
    SpeakersDownloadFailed(String),
}

pub struct App {
    polish: polish_ui::State,
    pending_install: Option<crate::update::InstallRequest>,
    history: history_ui::State,
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
    speaker_names: [String; 4],
    outputs: Vec<String>,
    call_levels: (f32, f32),
    call_status: String,
    call_native_audio: bool,
    call_native_received: bool,
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

fn save_settings(directory: &std::path::Path, settings: &Settings) -> anyhow::Result<()> {
    use std::io::Write;
    static SAVES: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    // Serialize replacement within this process. Separate processes never share
    // a temporary file, so a failed save cannot remove another writer's data.
    let _guard = SAVES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::fs::create_dir_all(directory)?;
    let temp = directory.join(format!(
        "settings-{}-{}.tmp",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| -> anyhow::Result<()> {
        file.write_all(&serde_json::to_vec_pretty(settings)?)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, directory.join("settings.json"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

impl App {
    fn new() -> Self {
        let settings_path = model::data_dir().join("settings.json");
        let (mut settings, warning) = match std::fs::read(&settings_path) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(s) => (s, None),
                Err(_) => (Settings::default(), Some("Saved settings could not be read. Your previous file has not been changed.".to_owned())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
            Err(e) => (Settings::default(), Some(format!("Could not read saved settings: {e}"))),
        };
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
                            calls::run(engine, request, |update| {
                                let _ = worker_tx.send(Event::Call(update));
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
                    Command::Transcribe(pcm, id, final_pass, token) => {
                        let start = Instant::now();
                        let result = match engine.as_mut() {
                            Some(e) => {
                                e.set_cancel_token(&token);
                                e.transcribe(&pcm).map_err(|e| format!("{e:#}"))
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
        let mut app = Self {
            polish: Default::default(),

            pending_install: None,
            history: history_ui::State::start(),
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
            speaker_names: Default::default(),
            outputs: call_capture::outputs(),
            call_levels: (0.0, 0.0),
            call_status: "Ready to capture a call".into(),
            call_native_audio: false,
            call_native_received: false,
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
            let result = recording.stop();
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
                    self.status = format!("Listening on {}", recording.device);
                    self.recording = Some(recording);
                    if self.settings.audio_feedback {
                        crate::audio_cues::play(crate::audio_cues::Cue::Started);
                    }
                    self.last_seconds = 0;
                    self.target = target;
                    self.dictation_app = target.and_then(platform::app_name);
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
                    self.live_mode = self.settings.insert
                        && self.settings.live_insert
                        && target.is_some()
                        && !self.hold_recording;
                    let _ = self.integration_tx.send(integration::Action::Cancel);
                    if self.settings.insert
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
            if self.settings.hotkey_mode == platform::HotkeyMode::Toggle
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
        if self.settings.hotkey_mode == platform::HotkeyMode::Hold {
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
        match self.settings.hotkey_mode {
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
        self.cancel.cancel();
        self.insertion_cancel.cancel();
        self.pending_final = None;
        self.integration_pending = None;
        self.target = None;
        self.recording = None;
        self.hold_recording = false;
        self.shortcut_gesture = Default::default();
        self.live_mode = false;
        self.busy = false;
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
        self.history_save_dictation();
        self.history.dictation = None;
        self.history.dictation_deleted = false;
        self.dictation_app = None;
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

    fn update_text(&mut self, raw: String) {
        self.raw = [self.committed_raw.as_str(), raw.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let style = crate::writing_style::effective(
            &self.settings.styles,
            self.dictation_app.as_deref(),
            self.settings.clean_speech,
        );
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
            if seconds >= 1.2 && seconds - self.preview_audio_seconds >= 0.8 {
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
                            self.call_context.push(row);
                            self.history_call_changed();
                        }
                    }
                    calls::Update::AudioContextStatus(message) => self.context_status = message,
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
                    calls::Update::Rows(rows) => {
                        if self.is_note_capture() {
                            // Keep source identities and timestamps immutable so
                            // live note updates can reuse earlier model sections.
                            self.call_committed.extend(rows);
                        } else {
                            calls::append_rows(&mut self.call_committed, rows);
                        }
                        for annotation in &self.call_context {
                            crate::sensevoice::apply(&mut self.call_committed, annotation);
                        }
                        self.call_rows.clone_from(&self.call_committed);
                        self.refresh_call_notes();
                        self.history_call_changed();
                    }
                    calls::Update::Preview(rows) => {
                        self.call_rows.clone_from(&self.call_committed);
                        calls::append_rows(&mut self.call_rows, rows);
                        for annotation in &self.call_context {
                            crate::sensevoice::apply(&mut self.call_rows, annotation);
                        }
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
                                self.update_text(text.clone());
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
                    self.update_text(text);
                    self.elapsed = Some(elapsed);
                    if !final_pass {
                        self.publish_live_preview();
                        continue;
                    }
                    // Resolve from speech, before dictionary rules, and never rewrite literal templates.
                    let spoken = if crate::writing_style::effective(
                        &self.settings.styles,
                        self.dictation_app.as_deref(),
                        self.settings.clean_speech,
                    ) != crate::writing_style::WritingStyle::Verbatim
                    {
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
                    self.dictation_metrics.recognized_words =
                        Some(self.raw.split_whitespace().count() as u64);
                    self.dictation_metrics.dictionary_replacements =
                        Some(self.words_changed as u64);
                    self.dictation_metrics.cleanup_edits = Some(self.cleanup_count as u64);
                    self.status = if self.text.is_empty() {
                        "No speech was recognized".into()
                    } else {
                        "Your transcript is ready".into()
                    };
                    self.history_save_dictation();
                    if self.settings.insert
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
                        Err(error) => error,
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
        self.updater.poll();
        self.poll_saved_search();
        self.polish_poll();
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
        self.status = "Downloading the speech model...".into();
        let tx = self.event_tx.clone();

        std::thread::spawn(move || {
            let mut last = Instant::now();
            let result = model::download(|p| {
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
        self.start_stream_capture(false)
    }

    fn start_stream_capture(&mut self, microphone_only: bool) -> bool {
        let native_audio = match calls::select_native_source(
            !microphone_only && self.settings.discord_companion,
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
        self.speaker_names = Default::default();
        let control = call_capture::Control::new();
        if !microphone_only {
            control.set_discord(self.discord.clone());
        }
        self.cancel = CancelToken::new();
        self.call_native_audio = native_audio;
        self.call_native_received = false;
        self.call_context.clear();
        let request = calls::Request {
            audio_context: self.settings.audio_context,
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
        match self
            .commands
            .send(Command::Call(request, self.cancel.clone()))
        {
            Ok(()) => {
                self.call = Some(control);
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
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_file(directory.join("settings.json")).unwrap();
        std::fs::remove_dir(directory).unwrap();
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
                speaker_names: Default::default(),
                outputs: Vec::new(),
                call_levels: (0.0, 0.0),
                call_status: String::new(),
                call_native_audio: false,
                call_native_received: false,
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
        let (tx, actions) = mpsc::channel();
        app.integration_tx = tx;
        app.busy = true;
        app.chunk_inflight = true;
        app.pending_final = Some(vec![0.2; 32000]);
        app.text = "Visible preview".into();
        let cancelled_id = app.utterance;
        app.cancel_dictation();
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
