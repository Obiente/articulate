use crate::{
    audio::{self, Recording},
    call_capture, calls, cleanup,
    dictionary::{self, Entry},
    engine::Engine,
    integration, learning, macros, model,
    platform::{self, Target},
};
use eframe::egui::{self, Color32, RichText};
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

const ACCENT: Color32 = Color32::from_rgb(144, 223, 201);
mod discord_ui;
mod editors;
mod history_ui;
mod preferences;
mod surface;
mod theme;
#[cfg(test)]
mod ui_capture;
mod update_ui;

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    cpu: bool,
    insert: bool,
    entries: Vec<Entry>,
    macros: Vec<macros::VoiceMacro>,
    model_path: String,
    microphone: Option<String>,
    #[serde(default = "enabled")]
    clean_speech: bool,
    output: Option<String>,
    #[serde(default = "enabled")]
    learn_corrections: bool,
    #[serde(default = "enabled")]
    live_insert: bool,
    hotkey: platform::Hotkey,
    styles: Vec<crate::writing_style::StyleRule>,
    discord_companion: bool,
    discord_pairing_key: String,
}
fn enabled() -> bool {
    true
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            cpu: false,
            insert: false,
            entries: Vec::new(),
            macros: Vec::new(),
            model_path: String::new(),
            microphone: None,
            clean_speech: true,
            output: None,
            learn_corrections: true,
            live_insert: true,
            hotkey: platform::Hotkey::default(),
            styles: Vec::new(),
            discord_companion: false,
            discord_pairing_key: String::new(),
        }
    }
}

enum Command {
    Load(PathBuf, bool),
    Transcribe(Vec<f32>, u64, bool, CancelToken),
    Call(calls::Request, CancelToken),
}
enum Event {
    Loaded(String),
    Text(Result<String, String>, f32, u64, bool),
    Error(String),
    Progress(f32),
    Downloaded(PathBuf),
    Call(calls::Update),
    CallFinished(Result<(), String>),
    SpeakersDownloaded,
}

pub struct App {
    updates: crate::update::State,
    pending_install: Option<crate::update::InstallRequest>,
    history: history_ui::State,
    settings: Settings,
    commands: Sender<Command>,
    events: Receiver<Event>,
    event_tx: Sender<Event>,
    hotkeys: Receiver<platform::HotkeyEvent>,
    hotkey_tx: Sender<platform::Hotkey>,
    hotkey_draft: platform::Hotkey,
    hotkey_pending: bool,
    style_app: String,
    style_draft: crate::writing_style::WritingStyle,
    style_message: String,
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
    text: String,
    raw: String,
    elapsed: Option<f32>,
    words_changed: usize,
    heard: String,
    wanted: String,
    wav_path: String,
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
    correction_editor: editors::CorrectionEditor,
    macro_editor: editors::MacroEditor,
    call_notes: Option<crate::notes::Notes>,
    library_input: String,
    meter: [f32; 32],
    meter_at: Instant,
    last_seconds: u64,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        theme::configure(&ctx);
        let settings_path = model::data_dir().join("settings.json");
        let (mut settings, warning) = match std::fs::read(&settings_path) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(s) => (s, None),
                Err(_) => (Settings::default(), Some("Saved settings could not be read. Your previous file has not been changed.".to_owned())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
            Err(e) => (Settings::default(), Some(format!("Could not read saved settings: {e}"))),
        };
        if settings.model_path.is_empty() {
            settings.model_path = model::default_path().to_string_lossy().into_owned();
        }
        let (commands, rx) = mpsc::channel();
        let (tx, events) = mpsc::channel();
        let worker_tx = tx.clone();
        let cancel = CancelToken::new();
        let worker_cancel = cancel.clone();
        let worker_ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut engine: Option<Engine> = None;
            while let Ok(command) = rx.recv() {
                let result = match command {
                    Command::Call(request, token) => {
                        let result = if let Some(engine) = engine.as_mut() {
                            engine.set_cancel_token(&token);
                            calls::run(engine, request, |update| {
                                let _ = worker_tx.send(Event::Call(update));
                                worker_ctx.request_repaint();
                            })
                            .map_err(|e| format!("{e:#}"))
                        } else {
                            Err("Load a speech model first".into())
                        };
                        Event::CallFinished(result)
                    }
                    Command::Load(path, cpu) => {
                        engine = None;
                        match Engine::load(&path, cpu, &worker_cancel) {
                            Ok(e) => {
                                let backend = e.backend.clone();
                                engine = Some(e);
                                Event::Loaded(backend)
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
                worker_ctx.request_repaint();
            }
        });
        let (hotkey_tx, hotkeys) = mpsc::channel();
        let hotkey_draft = settings.hotkey.clone();
        let hotkey_tx = platform::hotkey(settings.hotkey.clone(), hotkey_tx);
        let integration_ctx = ctx.clone();
        let (integration_tx, integration_rx) =
            integration::start(move || integration_ctx.request_repaint());
        let mut app = Self {
            updates: Default::default(),
            pending_install: None,
            history: history_ui::State::start(),
            settings,
            commands,
            events,
            event_tx: tx,
            hotkeys,
            hotkey_tx,
            hotkey_draft,
            hotkey_pending: true,
            style_app: String::new(),
            style_draft: crate::writing_style::WritingStyle::Clean,
            style_message: String::new(),
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
            text: String::new(),
            raw: String::new(),
            elapsed: None,
            words_changed: 0,
            heard: String::new(),
            wanted: String::new(),
            wav_path: String::new(),
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
            correction_editor: Default::default(),
            macro_editor: Default::default(),
            call_notes: None,
            library_input: String::new(),
            meter: [0.0; 32],
            meter_at: Instant::now(),
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
        std::fs::create_dir_all(model::data_dir())?;
        let temp = model::data_dir().join("settings.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(&self.settings)?)?;
        std::fs::rename(temp, model::data_dir().join("settings.json"))?;
        Ok(())
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
            ))
            .is_err()
        {
            self.loading = false;
            self.status = "The transcription worker stopped. Please restart the app.".into();
        }
    }

    fn toggle(&mut self, target: Option<Target>) {
        if self.busy
            || self.loading
            || !self.ready
            || self.downloading.is_some()
            || self.call.is_some()
        {
            return;
        }
        if let Some(recording) = self.recording.take() {
            self.last_seconds = recording.seconds() as u64;
            match recording.stop() {
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
            match Recording::start(self.settings.microphone.as_deref()) {
                Ok(recording) => {
                    self.history_save_dictation();
                    self.history.dictation = None;
                    self.history.dictation_deleted = false;
                    self.status = format!("Listening on {}", recording.device);
                    self.recording = Some(recording);
                    self.last_seconds = 0;
                    self.target = target;
                    self.dictation_app = target.and_then(platform::app_name);
                    self.macro_used = None;
                    self.elapsed = None;
                    self.utterance += 1;
                    self.insertion_cancel.cancel();
                    self.insertion_cancel = CancelToken::new();
                    self.integration_inflight = false;
                    self.integration_pending = None;
                    self.live_mode =
                        self.settings.insert && self.settings.live_insert && target.is_some();
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

    fn shortcut(&mut self, target: Target) {
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
        self.toggle(target.is_external().then_some(target));
    }

    fn transcribe(&mut self, pcm: Vec<f32>) {
        self.history_save_dictation();
        self.history.dictation = None;
        self.history.dictation_deleted = false;
        self.dictation_app = None;
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
        let (cleaned, count) = crate::writing_style::apply(&self.raw, style);
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
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Call(update) => match update {
                    calls::Update::Started => {
                        self.call_status = "Listening to your microphone and call audio".into()
                    }
                    calls::Update::Levels(mic, output) => self.call_levels = (mic, output),
                    calls::Update::Rows(rows) => {
                        calls::append_rows(&mut self.call_committed, rows);
                        self.call_rows.clone_from(&self.call_committed);
                        self.refresh_call_notes();
                        self.history_call_changed();
                    }
                    calls::Update::Preview(rows) => {
                        self.call_rows.clone_from(&self.call_committed);
                        calls::append_rows(&mut self.call_rows, rows);
                        self.refresh_call_notes();
                        self.history_call_changed();
                    }
                },
                Event::CallFinished(result) => {
                    self.history_save_call();
                    self.call = None;
                    self.call_levels = (0.0, 0.0);
                    self.call_status = match result {
                        Ok(()) => "Call transcript ready".into(),
                        Err(error) => error,
                    };
                }
                Event::SpeakersDownloaded => {
                    self.downloading = None;
                    self.call_status = "Speaker identification is ready".into();
                }
                Event::Loaded(backend) => {
                    self.loading = false;
                    self.ready = true;
                    self.backend = backend;
                    self.status = "Ready when you are".into();
                }
                Event::Text(result, elapsed, id, final_pass) => {
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
                    if id != self.utterance {
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
                platform::HotkeyEvent::Pressed(target) if self.pending_install.is_none() => {
                    self.shortcut(target)
                }
                platform::HotkeyEvent::Pressed(_) => {}
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
        self.history_poll();
        self.preview();
    }

    fn remember(&mut self, entry: Entry) {
        let Some(change) = learning::Change::apply(&mut self.settings.entries, entry) else {
            return;
        };
        self.overlay_message = format!(
            "Remembered: {} → {}. Undo in Articulate.",
            change.entry.heard, change.entry.wanted
        );
        self.overlay_until = Some(Instant::now() + Duration::from_secs(7));
        self.remembered.push(change);
        if self.remembered.len() > 5 {
            self.remembered.remove(0);
        }
        self.save();
    }

    fn learning_notice(&mut self, ui: &mut egui::Ui) {
        if self.remembered.is_empty() {
            return;
        }
        let mut undo = false;
        let mut dismiss = false;
        egui::Frame::new()
            .fill(theme::SURFACE)
            .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
            .corner_radius(24)
            .inner_margin(egui::Margin::symmetric(16, 8))
            .show(ui, |ui| {
                let change = self.remembered.last().unwrap();
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "Remembered: {} → {}",
                            change.entry.heard, change.entry.wanted
                        ))
                        .color(ACCENT),
                    );
                    undo = ui.small_button("Undo").clicked();
                    dismiss = ui.small_button("Dismiss").clicked();
                });
            });
        if undo || dismiss {
            let change = self.remembered.pop().unwrap();
            if undo {
                if change.undo(&mut self.settings.entries) {
                    self.save();
                    self.status = "Learning undone. Your edited text is unchanged.".into();
                } else {
                    self.status =
                        "This correction has changed since then. Review it in Vocabulary.".into();
                }
            }
        }
    }

    fn overlay(&self, ctx: &egui::Context) {
        let recording = self.target.is_some() && self.recording.is_some();
        let finishing = self.target.is_some() && self.busy;
        let notice = self.overlay_until.is_some_and(|t| t > Instant::now());
        if !(recording || finishing || notice) {
            return;
        }
        let message = if recording {
            let seconds = self.recording.as_ref().unwrap().seconds() as u64;
            format!(
                "Listening  {:02}:{:02}\n{} to finish",
                seconds / 60,
                seconds % 60,
                self.settings.hotkey.label()
            )
        } else if finishing {
            "Finishing your transcript...".into()
        } else {
            self.overlay_message.clone()
        };
        let monitor = ctx
            .input(|i| i.viewport().monitor_size)
            .unwrap_or(egui::vec2(1280.0, 720.0));
        ctx.show_viewport_deferred(
            egui::ViewportId::from_hash_of("dictation_indicator"),
            egui::ViewportBuilder::default()
                .with_title("Articulate dictation")
                .with_inner_size([420.0, 84.0])
                .with_position([(monitor.x - 420.0) / 2.0, monitor.y - 150.0])
                .with_decorations(false)
                .with_resizable(false)
                .with_always_on_top()
                .with_taskbar(false)
                .with_active(false)
                .with_mouse_passthrough(true),
            move |ctx, _| {
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(Color32::from_rgb(19, 23, 26))
                            .inner_margin(14.0),
                    )
                    .show(ctx, |ui| {
                        ui.label(RichText::new(&message).color(ACCENT).size(15.0));
                    });
                ctx.request_repaint_after(Duration::from_millis(100));
            },
        );
    }

    fn download(&mut self, ctx: &egui::Context) {
        self.downloading = Some(0.0);
        self.status = "Downloading the speech model...".into();
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut last = Instant::now();
            let result = model::download(|p| {
                if last.elapsed() > Duration::from_millis(100) {
                    let _ = tx.send(Event::Progress(p));
                    ctx.request_repaint();
                    last = Instant::now();
                }
            });
            let _ = tx.send(match result {
                Ok(p) => Event::Downloaded(p),
                Err(e) => Event::Error(format!("{e:#}")),
            });
            ctx.request_repaint();
        });
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let muted = Color32::from_rgb(157, 172, 174);
        ui.label(RichText::new("Make yourself at home").size(30.0));
        ui.add_space(6.0);
        ui.label(RichText::new("Your voice. Your preferences. Your device.").color(muted));
        ui.add_space(28.0);
        let idle = !self.busy
            && self.call.is_none()
            && !self.preview_inflight
            && !self.loading
            && self.recording.is_none()
            && self.downloading.is_none();
        ui.label(RichText::new("Dictation").size(20.0).strong());
        ui.add_space(12.0);
        self.shortcut_preferences(ui, idle);
        if ui
            .add_enabled(
                idle,
                egui::Checkbox::new(
                    &mut self.settings.clean_speech,
                    "Clean up repeated words and spoken corrections",
                ),
            )
            .changed()
        {
            self.save();
        }
        ui.label(
            RichText::new(
                "Keeps emphasis and negations. The original transcription is always available.",
            )
            .small()
            .color(muted),
        );
        ui.add_space(14.0);
        self.style_preferences(ui, idle);
        if ui
            .add_enabled(
                idle,
                egui::Checkbox::new(&mut self.settings.insert, "Insert into the focused app"),
            )
            .changed()
        {
            self.save();
        }
        ui.label(
            RichText::new(
                "Keep the same field focused. Your text is inserted without pressing Enter.",
            )
            .small()
            .color(muted),
        );
        ui.add_space(14.0);
        if ui
            .add_enabled(
                idle && self.settings.insert,
                egui::Checkbox::new(
                    &mut self.settings.live_insert,
                    "Type into the app while I speak",
                ),
            )
            .changed()
        {
            self.save();
        }
        ui.label(RichText::new("Moving the caret or editing pauses live typing. Other fields receive text when you finish.").small().color(muted));
        ui.add_space(14.0);
        if ui
            .checkbox(
                &mut self.settings.learn_corrections,
                "Remember spelling corrections I make",
            )
            .changed()
        {
            self.edited_at = None;
            self.edit_baseline.clone_from(&self.text);
            if !self.settings.learn_corrections {
                let _ = self.integration_tx.send(integration::Action::StopLearning);
            }
            self.save();
        }
        ui.label(RichText::new("Learns short edits in your transcript or a supported app. Learning stops when you leave the field or after two minutes.").small().color(muted));
        ui.add_space(28.0);
        ui.label(RichText::new("Audio & recognition").size(20.0).strong());
        ui.add_space(12.0);
        ui.add_enabled_ui(idle, |ui| {
            ui.label("Microphone");
            let old = self.settings.microphone.clone();
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("microphone")
                    .width(300.0)
                    .selected_text(
                        self.settings
                            .microphone
                            .as_deref()
                            .unwrap_or("Windows default microphone"),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.settings.microphone,
                            None,
                            "Windows default microphone",
                        );
                        for name in &self.microphones {
                            ui.selectable_value(
                                &mut self.settings.microphone,
                                Some(name.clone()),
                                name,
                            );
                        }
                    });
                if ui.button("Refresh").clicked() {
                    self.microphones = audio::input_devices();
                }
            });
            if old != self.settings.microphone {
                self.save();
            }
            ui.add_space(14.0);
            if ui
                .checkbox(&mut self.settings.cpu, "Use CPU only")
                .changed()
            {
                self.save();
                if self.ready {
                    self.load();
                }
            }
            ui.label(
                RichText::new("Turn on if you prefer to keep your graphics card free.")
                    .small()
                    .color(muted),
            );
            ui.add_space(12.0);
            egui::CollapsingHeader::new("Speech model")
                .default_open(!self.ready)
                .show(ui, |ui| {
                    ui.add_space(8.0);
                    ui.label("Qwen3-ASR 1.7B · 8-bit · 2.19 GB");
                    ui.label(
                        RichText::new(
                            "Recommended for accuracy. Download once, then transcribe offline.",
                        )
                        .small()
                        .color(muted),
                    );
                    ui.add_space(8.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings.model_path)
                            .hint_text("Path to a local GGUF model")
                            .desired_width(f32::INFINITY),
                    );
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Load local model").clicked() {
                            self.save();
                            self.load();
                        }
                        if ui.button("Download recommended model").clicked() {
                            self.download(ctx);
                        }
                    });
                    ui.label(
                        RichText::new("Model downloads are provided by Hugging Face.")
                            .small()
                            .color(muted),
                    );
                });
        });
        if let Some(progress) = self.downloading {
            ui.add(egui::ProgressBar::new(progress).show_percentage());
        }
        ui.add_space(8.0);
        ui.label(RichText::new(&self.status).small().color(muted));
        ui.add_space(28.0);
        ui.label(RichText::new("Files & vocabulary").size(20.0).strong());
        ui.add_space(12.0);
        ui.collapsing("Transcribe an audio file", |ui| {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Choose a WAV file on this computer.")
                    .small()
                    .color(muted),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.wav_path)
                    .hint_text("Path to a WAV file")
                    .desired_width(f32::INFINITY),
            );
            if ui
                .add_enabled(
                    idle && self.ready && !self.wav_path.is_empty(),
                    egui::Button::new("Transcribe file"),
                )
                .clicked()
            {
                match audio::read_wav(&self.wav_path) {
                    Ok(pcm) => {
                        self.target = None;
                        self.transcribe(pcm);
                        self.page = 0;
                    }
                    Err(e) => self.status = format!("{e:#}"),
                }
            }
        });
        ui.add_space(10.0);
        ui.collapsing("Move your vocabulary & shortcuts", |ui| {
            ui.add_space(8.0);
            self.library_ui(ui);
        });
        ui.add_space(28.0);
        ui.label(RichText::new("On this device").size(20.0).strong());
        ui.add_space(8.0);
        ui.label(RichText::new("No account, telemetry or cloud transcription. Your transcripts, notes, preferences, vocabulary and shortcuts stay on this computer. Saved history stays here until you delete it.").color(muted));
        ui.add_space(28.0);
        if let Some(request) = update_ui::show(
            ui,
            &mut self.updates,
            idle && !self.integration_inflight && self.history.error.is_none(),
        ) {
            self.history_save_dictation();
            self.history_save_call();
            self.pending_install = Some(request);
            ctx.request_repaint();
        }
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

    fn poll_pending_install(&mut self, ctx: &egui::Context) -> bool {
        if self.pending_install.is_none() {
            return false;
        }
        let result = self.install_saves_settled();
        match result {
            Ok(false) => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(80.0);
                        ui.spinner();
                        ui.heading("Saving before the update");
                        ui.label("Your transcripts and notes are being saved on this device.");
                    });
                });
                ctx.request_repaint_after(Duration::from_millis(50));
                true
            }
            Ok(true) => {
                let request = self.pending_install.take().unwrap();
                match request.launch() {
                    Ok(()) => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        true
                    }
                    Err(error) => {
                        self.updates.status = crate::update::Status::Error(format!("{error:#}"));
                        self.page = 2;
                        false
                    }
                }
            }
            Err(error) => {
                self.pending_install = None;
                self.updates.status = crate::update::Status::Error(error);
                self.page = 2;
                false
            }
        }
    }

    fn calls_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let muted = Color32::from_rgb(157, 172, 174);
        // Keep call controls compact without changing the spacing of other pages.
        ui.spacing_mut().item_spacing = egui::vec2(10.0, 4.0);
        ui.spacing_mut().button_padding = egui::vec2(12.0, 6.0);
        ui.spacing_mut().interact_size.y = 28.0;
        let idle = self.call.is_none()
            && self.recording.is_none()
            && !self.busy
            && !self.preview_inflight
            && !self.loading
            && self.downloading.is_none();
        egui::Frame::new()
            .fill(Color32::from_rgb(23, 32, 34))
            .corner_radius(22)
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_min_width((ui.available_width() - 1.0).max(0.0));
                ui.horizontal_wrapped(|ui| {
                    if let Some(control) = &self.call {
                        let seconds = control.end_seconds() as u64;
                        ui.label(
                            RichText::new(format!("{:02}:{:02}", seconds / 60, seconds % 60))
                                .size(24.0)
                                .monospace()
                                .color(ACCENT),
                        );
                        ui.add_space(14.0);
                        if ui
                            .add_enabled(
                                control.stop_ns.load(Ordering::SeqCst) == 0,
                                egui::Button::new(
                                    RichText::new("Finish capture")
                                        .color(Color32::from_rgb(13, 28, 25)),
                                )
                                .fill(ACCENT)
                                .corner_radius(22)
                                .min_size(egui::vec2(156.0, 36.0)),
                            )
                            .clicked()
                        {
                            control.stop();
                            self.call_status = "Finishing the last section...".into();
                        }
                    } else if ui
                        .add_enabled(
                            idle && self.ready && crate::speakers::path().is_file(),
                            egui::Button::new(
                                RichText::new("Capture conversation")
                                    .color(Color32::from_rgb(13, 28, 25)),
                            )
                            .fill(ACCENT)
                            .corner_radius(22)
                            .min_size(egui::vec2(192.0, 36.0)),
                        )
                        .clicked()
                    {
                        self.save();
                        self.history_save_call();
                        self.history.call = None;
                        self.history.call_deleted = false;
                        self.call_tab = 0;
                        self.call_rows.clear();
                        self.call_committed.clear();
                        self.call_search.clear();
                        self.call_notes = None;
                        self.speaker_names = Default::default();
                        let control = call_capture::Control::new();
                        control.set_discord(self.discord.clone());
                        self.cancel = CancelToken::new();
                        let request = calls::Request {
                            microphone: self.settings.microphone.clone(),
                            output: self.settings.output.clone(),
                            cpu: self.settings.cpu,
                            control: control.clone(),
                        };
                        match self
                            .commands
                            .send(Command::Call(request, self.cancel.clone()))
                        {
                            Ok(()) => {
                                self.call = Some(control);
                                self.call_status = "Preparing call capture...".into();
                            }
                            Err(_) => {
                                self.call_status =
                                    "The speech worker stopped. Restart the app.".into()
                            }
                        }
                    }
                    if ui
                        .add_enabled(
                            !self.call_rows.is_empty(),
                            egui::Button::new("Copy transcript")
                                .min_size(egui::vec2(140.0, 36.0))
                                .corner_radius(22),
                        )
                        .clicked()
                    {
                        self.call_status = match platform::copy(&calls::text(
                            &self.call_rows,
                            &self.speaker_names,
                        )) {
                            Ok(()) => "Call transcript copied".into(),
                            Err(e) => e.to_string(),
                        };
                    }
                    ui.add_enabled_ui(!self.call_rows.is_empty(), |ui| {
                        ui.menu_button("Export…", |ui| {
                            use crate::call_export::Format;
                            for (label, extension, format) in [
                                ("Plain text", "txt", Format::Text),
                                ("Markdown", "md", Format::Markdown),
                                ("Subtitles (SRT)", "srt", Format::Srt),
                                ("Subtitles (WebVTT)", "vtt", Format::WebVtt),
                            ] {
                                ui.horizontal(|ui| {
                                    ui.label(label);
                                    if ui.small_button("Save").clicked() {
                                        let text = crate::call_export::export(
                                            &self.call_rows,
                                            &self.speaker_names,
                                            format,
                                        );
                                        self.call_status =
                                            match crate::export_file::save(&text, extension) {
                                                Ok(true) => "Call transcript saved".into(),
                                                Ok(false) => "Export cancelled".into(),
                                                Err(error) => {
                                                    format!("Could not save transcript: {error}")
                                                }
                                            };
                                        ui.close();
                                    }
                                    if ui.small_button("Copy").clicked() {
                                        let text = crate::call_export::export(
                                            &self.call_rows,
                                            &self.speaker_names,
                                            format,
                                        );
                                        self.call_status = match platform::copy(&text) {
                                            Ok(()) => format!("{label} copied"),
                                            Err(error) => error.to_string(),
                                        };
                                        ui.close();
                                    }
                                });
                            }
                        });
                    });
                });
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    let status = if !self.ready {
                        "Load a speech model in Settings to capture a call."
                    } else if !crate::speakers::path().is_file() {
                        "Open Setup to prepare speaker recognition."
                    } else {
                        &self.call_status
                    };
                    ui.label(RichText::new(status).small().color(muted));
                    ui.add_space(12.0);
                    let snapshot = self
                        .discord
                        .as_ref()
                        .map(|connection| connection.snapshot());
                    let (label, active) = discord_ui::connection_label(snapshot.as_ref());
                    ui.label(
                        RichText::new(format!("Discord · {label}"))
                            .small()
                            .color(if active { ACCENT } else { muted }),
                    );
                    if ui.small_button("Setup").clicked() {
                        self.call_tab = 2;
                    }
                });
            });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.call_tab, 0, "Transcript");
            ui.selectable_value(&mut self.call_tab, 1, "Notes");
            ui.selectable_value(&mut self.call_tab, 2, "Setup");
        });
        ui.add_space(4.0);
        match self.call_tab {
            1 => {
                egui::ScrollArea::vertical()
                    .id_salt("call_notes_panel")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.call_notes_ui(ui));
            }
            2 => {
                egui::ScrollArea::vertical()
                    .id_salt("call_setup_panel")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.call_setup_ui(ui, ctx, idle));
            }
            _ => self.call_transcript_ui(ui),
        }
    }

    fn call_setup_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, idle: bool) {
        let muted = Color32::from_rgb(157, 172, 174);
        if self.call.is_some() {
            ui.columns(2, |cols| {
                cols[0].add(
                    egui::ProgressBar::new((self.call_levels.0 * 4.0).min(1.0))
                        .text("Your microphone"),
                );
                cols[1].add(
                    egui::ProgressBar::new((self.call_levels.1 * 4.0).min(1.0)).text("Call audio"),
                );
            });
            ui.add_space(12.0);
        }
        egui::CollapsingHeader::new("Audio setup")
            .default_open(false)
            .show(ui, |ui| {
        ui.add_space(8.0);
        let previous_mic = self.settings.microphone.clone();
        let previous_output = self.settings.output.clone();
        ui.add_enabled_ui(idle, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Microphone");
                egui::ComboBox::from_id_salt("call_mic")
                    .width(300.0)
                    .selected_text(
                        self.settings
                            .microphone
                            .as_deref()
                            .unwrap_or("Windows default"),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.settings.microphone, None, "Windows default");
                        for name in &self.microphones {
                            ui.selectable_value(
                                &mut self.settings.microphone,
                                Some(name.clone()),
                                name,
                            );
                        }
                    });
            });
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("Call output");
                egui::ComboBox::from_id_salt("call_output")
                    .width(300.0)
                    .selected_text(self.settings.output.as_deref().unwrap_or("Windows default"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.settings.output, None, "Windows default");
                        for name in &self.outputs {
                            ui.selectable_value(
                                &mut self.settings.output,
                                Some(name.clone()),
                                name,
                            );
                        }
                    });
                if ui.small_button("Refresh").clicked() {
                    self.outputs = call_capture::outputs();
                    self.microphones = audio::input_devices();
                }
            });
        });
        if previous_mic != self.settings.microphone || previous_output != self.settings.output {
            self.save();
        }
        ui.add_space(8.0);
        ui.label(RichText::new("Captures your microphone and all sound from this output. Use headphones to keep voices separate.").small().color(muted));
        });
        ui.label(
            RichText::new(format!(
                "{}  +  {}",
                self.settings
                    .microphone
                    .as_deref()
                    .unwrap_or("Default microphone"),
                self.settings
                    .output
                    .as_deref()
                    .unwrap_or("Default call output")
            ))
            .small()
            .color(muted),
        );
        ui.add_space(16.0);
        self.discord_ui(ui, ctx);
        ui.add_space(16.0);
        if !self.ready {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("Load a speech model to start capturing calls.").color(muted),
                );
                if ui.button("Open settings").clicked() {
                    self.page = 2;
                }
            });
            ui.add_space(12.0);
        }
        if !crate::speakers::path().is_file() {
            ui.label(RichText::new("One step before your first call").strong());
            ui.label(
                RichText::new("Download speaker recognition, 237 MB. Future calls work offline.")
                    .small()
                    .color(muted),
            );
            if ui
                .add_enabled(idle, egui::Button::new("Set up speaker recognition"))
                .clicked()
            {
                self.downloading = Some(0.0);
                let tx = self.event_tx.clone();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let mut last = Instant::now();
                    let result = model::download_speakers(|p| {
                        if last.elapsed() > Duration::from_millis(100) {
                            let _ = tx.send(Event::Progress(p));
                            ctx.request_repaint();
                            last = Instant::now();
                        }
                    });
                    let _ = tx.send(match result {
                        Ok(_) => Event::SpeakersDownloaded,
                        Err(e) => Event::Error(e.to_string()),
                    });
                    ctx.request_repaint();
                });
            }
        }
        if let Some(progress) = self.downloading {
            ui.add(egui::ProgressBar::new(progress).show_percentage());
        }
        let previous_names = self.speaker_names.clone();
        ui.collapsing("Name the speakers", |ui| {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Up to four remote voices. Check speaker labels before sharing.")
                    .small()
                    .color(muted),
            );
            ui.add_space(8.0);
            for (i, name) in self.speaker_names.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Speaker {}", i + 1)).color(ACCENT));
                    ui.add(
                        egui::TextEdit::singleline(name)
                            .hint_text("Add a name")
                            .desired_width(220.0),
                    );
                });
                ui.add_space(6.0);
            }
        });
        if previous_names != self.speaker_names {
            self.history_call_changed();
        }
    }

    fn call_notes_ui(&mut self, ui: &mut egui::Ui) {
        let muted = Color32::from_rgb(157, 172, 174);
        egui::Frame::new().inner_margin(0.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Meeting notes").size(20.0).strong());
                ui.add_space(12.0);
                let current = self
                    .call_notes
                    .as_ref()
                    .is_some_and(|notes| notes.is_current(&self.call_rows));
                let label = if self.call_notes.is_some() {
                    "Refresh notes"
                } else {
                    "Create notes"
                };
                if ui
                    .add_enabled(
                        !self.call_rows.is_empty() && !current,
                        egui::Button::new(label),
                    )
                    .clicked()
                {
                    self.call_notes = Some(crate::notes::Notes::build(&self.call_rows));
                    self.history_call_changed();
                }
                if let Some(notes) = &self.call_notes {
                    if ui.button("Copy notes").clicked() {
                        self.call_status = match platform::copy(&notes.text(&self.speaker_names)) {
                            Ok(()) => "Meeting notes copied".into(),
                            Err(error) => error.to_string(),
                        };
                    }
                    if !notes.is_current(&self.call_rows) {
                        ui.label(
                            RichText::new("New transcript available")
                                .small()
                                .color(ACCENT),
                        );
                    }
                }
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Highlights and possible actions, with the speaker and time attached.",
                )
                .small()
                .color(muted),
            );
            if let Some(notes) = &self.call_notes {
                egui::CollapsingHeader::new("Read meeting notes")
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(
                                "Excerpts from your transcript. Review actions before sharing.",
                            )
                            .small()
                            .color(muted),
                        );
                        for (title, quotes) in [
                            ("Highlights", &notes.highlights),
                            ("Possible actions", &notes.actions),
                        ] {
                            ui.add_space(8.0);
                            ui.label(RichText::new(title).strong());
                            if quotes.is_empty() {
                                ui.label("No excerpts selected. Read the full transcript below.");
                            }
                            for quote in quotes {
                                ui.label(
                                    RichText::new(quote.attribution(&self.speaker_names))
                                        .small()
                                        .color(ACCENT),
                                )
                                .on_hover_text(format!("Transcript section {}", quote.row + 1));
                                ui.label(&quote.text);
                                ui.add_space(12.0);
                            }
                        }
                    });
            }
        });
    }

    fn call_transcript_ui(&mut self, ui: &mut egui::Ui) {
        let muted = Color32::from_rgb(157, 172, 174);
        if !self.call_rows.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.call_search)
                        .hint_text("Find words or a speaker")
                        .desired_width(320.0),
                );
                if !self.call_search.is_empty() && ui.small_button("Clear search").clicked() {
                    self.call_search.clear();
                }
            });
        }
        let query = self.call_search.trim().to_lowercase();
        let matching: Vec<_> = self
            .call_rows
            .iter()
            .filter(|row| {
                query.is_empty()
                    || row.text.to_lowercase().contains(&query)
                    || calls::label(row, &self.speaker_names)
                        .to_lowercase()
                        .contains(&query)
            })
            .collect();
        if !query.is_empty() {
            ui.label(
                RichText::new(format!("{} matching sections", matching.len()))
                    .small()
                    .color(muted),
            );
        }
        if self.call_rows.is_empty() {
            ui.add_space(18.0);
            ui.label(
                RichText::new("Every voice, in one place.")
                    .size(24.0)
                    .color(muted),
            );
            ui.add_space(8.0);
            ui.label(RichText::new("Start a capture to see the conversation unfold with speaker labels and timestamps.").color(muted));
            ui.add_space(18.0);
        }
        egui::ScrollArea::vertical()
            .id_salt("call_rows")
            .max_height(ui.available_height())
            .auto_shrink([false, false])
            .stick_to_bottom(query.is_empty())
            .show(ui, |ui| {
                for row in matching {
                    ui.push_id((row.start_ms, row.microphone), |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(calls::label(row, &self.speaker_names))
                                    .color(ACCENT)
                                    .strong(),
                            );
                            ui.add_space(10.0);
                            ui.label(
                                RichText::new(format!(
                                    "{:02}:{:02}",
                                    row.start_ms / 60000,
                                    row.start_ms / 1000 % 60
                                ))
                                .small()
                                .monospace()
                                .color(muted),
                            );
                        });
                        ui.add_space(6.0);
                        ui.add(
                            egui::Label::new(RichText::new(&row.text).size(18.0)).selectable(true),
                        );
                        ui.add_space(22.0);
                    });
                }
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.receive();
        if self.poll_pending_install(ctx) {
            return;
        }
        self.poll_discord_launch();
        self.overlay(ctx);
        self.surface(ctx);
        ctx.request_repaint_after(Duration::from_millis(
            if self.recording.is_some() || self.busy || self.call.is_some() {
                50
            } else {
                150
            },
        ));
    }
}

impl Drop for App {
    fn drop(&mut self) {
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

    fn call_row(speaker: i32, start_ms: u64, text: &str) -> calls::Row {
        calls::Row {
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
        call_event(&mut app, calls::Update::Preview(Vec::new()));
        assert_eq!(app.call_rows[0].text, "Keep this.");
        assert_eq!(app.call_committed[0].text, "Keep this.");
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
    fn transcript_layout_keeps_complete_lines_above_the_recording_dock() {
        for (width, height, transcript) in [
            (
                1200.0,
                840.0,
                "First complete line.\nSecond complete line.\nThird complete line.\nFourth complete line.",
            ),
            (850.0, 620.0, "First complete line.\nSecond complete line."),
        ] {
            let (mut app, _) = app();
            app.text = transcript.into();
            let ctx = egui::Context::default();
            theme::configure(&ctx);
            let mut output = None;
            for _ in 0..3 {
                output = Some(ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, height),
                        )),
                        ..Default::default()
                    },
                    |ctx| app.surface(ctx),
                ));
            }
            let output = output.unwrap();
            let text = output
                .shapes
                .iter()
                .find_map(|shape| {
                    if let egui::epaint::Shape::Text(text) = &shape.shape
                        && text.galley.job.text == transcript
                    {
                        Some((shape.clip_rect, text))
                    } else {
                        None
                    }
                })
                .expect("The editable transcript must be rendered");
            let painted = text.1.galley.rect.translate(text.1.pos.to_vec2());
            assert!(
                text.0.contains_rect(painted),
                "Transcript clipped at {width}x{height}: {painted:?} outside {:?}",
                text.0
            );
            assert!(
                painted.bottom() < height - 151.0,
                "Transcript overlaps the recording dock"
            );
        }
    }

    #[test]
    fn shortcut_explains_unavailable_dictation_without_starting_audio() {
        let (mut app, _) = app();
        app.loading = true;
        app.shortcut(platform::test_target());
        assert!(app.overlay_message.contains("getting ready"));
        assert!(app.overlay_until.is_some());
        assert!(app.recording.is_none());
        app.loading = false;
        app.busy = true;
        app.shortcut(platform::test_target());
        assert!(app.overlay_message.contains("Finishing"));
        assert!(app.recording.is_none());
        app.busy = false;
        app.ready = false;
        app.shortcut(platform::test_target());
        assert!(app.overlay_message.contains("Settings"));
        assert!(app.recording.is_none());
    }

    pub(super) fn app() -> (App, Receiver<Command>) {
        let (commands, receiver) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let (_, hotkeys) = mpsc::channel();
        let (hotkey_tx, _) = mpsc::channel();
        let (integration_tx, _) = mpsc::channel();
        let (_, integration_rx) = mpsc::channel();
        (
            App {
                updates: Default::default(),
                pending_install: None,
                history: history_ui::State::default(),
                settings: Settings::default(),
                commands,
                events,
                event_tx,
                hotkeys,
                hotkey_tx,
                hotkey_draft: platform::Hotkey::default(),
                hotkey_pending: false,
                style_app: String::new(),
                style_draft: crate::writing_style::WritingStyle::Clean,
                style_message: String::new(),
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
                text: String::new(),
                raw: String::new(),
                elapsed: None,
                words_changed: 0,
                heard: String::new(),
                wanted: String::new(),
                wav_path: String::new(),
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
                correction_editor: Default::default(),
                macro_editor: Default::default(),
                call_notes: None,
                library_input: String::new(),
                meter: [0.0; 32],
                meter_at: Instant::now(),
                last_seconds: 0,
            },
            receiver,
        )
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
        assert!(!app.preview_inflight);
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
}
