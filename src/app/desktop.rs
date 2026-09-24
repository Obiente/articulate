//! Narrow local renderer bridge. The native controller owns all capture and persistence.
use super::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, mpsc::SyncSender};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    HistoryRefresh,
    HistoryOpen {
        id: String,
    },
    HistoryClose,
    NoteNew,
    NoteCaptureStart,
    NoteCaptureStop,
    SourceMove {
        id: String,
        topic_id: Option<String>,
        expected_topic_id: Option<String>,
    },
    SourceFile {
        id: String,
    },
    SessionPatch {
        id: String,
        title: Option<String>,
        personal_notes: Option<String>,
    },
    SessionDelete {
        id: String,
    },
    HistoryRetry,
    DictationStart,
    DictationStop,
    DictationCancel,
    DictationEdit {
        id: String,
        expected_text: String,
        text: String,
        correction: Option<super::transcript_editor::Spelling>,
    },
    CallStart {
        #[serde(default)]
        source: CallSource,
        #[serde(default)]
        language_preference: Option<String>,
        #[serde(default)]
        audio_context: Option<bool>,
    },
    CallStop,
    Copy {
        text: String,
    },
    Export {
        text: String,
        extension: String,
    },
    DictionarySave {
        heard: String,
        wanted: String,
        app: Option<String>,
        cues: Option<String>,
        index: Option<usize>,
        expected_heard: Option<String>,
        expected_entry: Option<Value>,
    },
    DictionaryDelete {
        index: usize,
        expected_heard: String,
        expected_entry: Option<Value>,
    },
    MacroSave {
        trigger: String,
        expansion: String,
        app: Option<String>,
        index: Option<usize>,
        expected_trigger: Option<String>,
        expected_macro: Option<Value>,
    },
    MacroDelete {
        index: usize,
        expected_trigger: String,
        expected_macro: Option<Value>,
    },
    SettingsPatch {
        cpu: Option<bool>,
        model_path: Option<String>,
        transcription_language: Option<String>,
        clean_speech: Option<bool>,
        insert: Option<bool>,
        live_insert: Option<bool>,
        learn_corrections: Option<bool>,
        audio_feedback: Option<bool>,
        audio_context: Option<bool>,
        microphone: Option<String>,
        output: Option<String>,
        hotkey_mode: Option<platform::HotkeyMode>,
        hotkey: Option<platform::Hotkey>,
        quick_note_hotkey: Option<platform::Hotkey>,
        discord_auto_connect: Option<bool>,
        discord_auto_transcribe: Option<bool>,
        vencord_auto_update: Option<bool>,
    },
    InsightsRefresh,
    ModelDownload,
    ModelLoad,
    NotesGenerate {
        id: String,
    },
    SummaryDownload,
    SummaryDownloadCancel,
    CleanupDownload,
    CleanupDownloadCancel,
    ContextDownload,
    SpeakersDownload,
    VerifierDownload,
    DiscordConnect {
        companion: bool,
    },
    DiscordDisconnect,
    DiscordRetry,
    DiscordRelaunch,
    CompanionDetect,
    CompanionInstall,
    CompanionOpenInstaller,
    UpdateCheck,
    UpdateDownload,
    UpdateInstall,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CallSource {
    #[default]
    System,
    Discord,
}

enum Request {
    Action(Action, Sender<Result<(), String>>),
    Shutdown(Sender<Result<(), String>>),
}

pub(crate) struct Bridge {
    sender: SyncSender<Request>,
    snapshot: Arc<Mutex<Value>>,
    frontend_seen: std::sync::atomic::AtomicBool,
    startup_failed: Arc<std::sync::atomic::AtomicBool>,
}

impl Bridge {
    pub(crate) fn start() -> Result<Self, String> {
        let (sender, requests) = mpsc::sync_channel(32);
        let settings = json!({"clean_speech":true,"insert":false,"live_insert":true,"learn_corrections":true,
            "audio_feedback":true,"microphone":null,"output":null,"hotkey":platform::Hotkey::default(),
            "quick_note_hotkey":default_quick_note_hotkey(),
            "hotkey_mode":"Hold","discord_auto_connect":true,"discord_auto_transcribe":false,"discord_companion":false});
        let mut initial = json!({
            "ready":false,"loading":true,"busy":false,"status":"Starting Articulate",
            "recording":false,"call_recording":false,"call_status":"Starting Articulate","seconds":0,
            "text":"","original":"","call_rows":[],"history":[],"selected":null,"call_session":null,
            "notes":{"ready":false,"working":false,"status":"","progress":null,"downloading":false,"download_status":""},
            "downloading":null,"history_loading":true,"history_error":null,"saving":false,
            "dictionary":[],"macros":[],"microphones":[],"outputs":[],"insights":null,
            "settings":settings
        });
        initial["note_recording"] = json!(false);
        initial["note_session"] = Value::Null;
        initial["note_rows"] = json!([]);
        initial["note_status"] = json!("");
        let snapshot = Arc::new(Mutex::new(initial));
        let state = snapshot.clone();
        let startup_failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let initialization_failed = startup_failed.clone();
        std::thread::Builder::new()
            .name("articulate-controller".into())
            .spawn(move || {
                transcribe_cpp::disable_logging();
                if let Err(error) = transcribe_cpp::init_backends_default() {
                    let mut snapshot = state.lock().unwrap_or_else(|e| e.into_inner());
                    snapshot["loading"] = json!(false);
                    snapshot["status"] = json!(format!("Could not initialize speech: {error}"));
                    initialization_failed.store(true, Ordering::Release);
                    return;
                }
                let mut app = App::new();
                if let Some(worker) = &app.history.worker {
                    worker.insights();
                }
                let mut published = Instant::now() - Duration::from_secs(1);
                loop {
                    app.receive();
                    app.poll_discord_launch();
                    if published.elapsed() >= Duration::from_millis(250) {
                        *state.lock().unwrap_or_else(|e| e.into_inner()) = app.desktop_snapshot();
                        published = Instant::now();
                    }
                    match requests.recv_timeout(Duration::from_millis(50)) {
                        Ok(Request::Action(action, reply)) => {
                            let result = app.desktop_action(action);
                            if let Err(error) = &result {
                                app.status.clone_from(error);
                            }
                            *state.lock().unwrap_or_else(|e| e.into_inner()) =
                                app.desktop_snapshot();
                            let _ = reply.send(result);
                        }
                        Ok(Request::Shutdown(reply)) => {
                            let result = app.desktop_shutdown();
                            let quit = result.is_ok();
                            if let Err(error) = &result {
                                app.status.clone_from(error);
                            }
                            *state.lock().unwrap_or_else(|e| e.into_inner()) =
                                app.desktop_snapshot();
                            let _ = reply.send(result);
                            if quit {
                                break;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            sender,
            snapshot,
            frontend_seen: std::sync::atomic::AtomicBool::new(false),
            startup_failed,
        })
    }

    pub(crate) fn snapshot(&self) -> Value {
        self.frontend_seen.store(true, Ordering::Relaxed);
        self.snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn has_frontend(&self) -> bool {
        self.frontend_seen.load(Ordering::Relaxed)
    }

    pub(crate) fn overlay(&self) -> Value {
        self.snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get("overlay")
            .cloned()
            .unwrap_or_else(
                || json!({"visible":false,"recording":false,"finishing":false,"message":""}),
            )
    }

    pub(crate) fn action(&self, action: Action) -> Result<(), String> {
        // The native save dialog may remain open while recording continues.
        // It must not suspend the controller's capture and autosave loop.
        let action = match action {
            Action::Export { text, extension } => return export_text(&text, &extension),
            action => action,
        };
        let (tx, rx) = mpsc::channel();
        self.sender
            .try_send(Request::Action(action, tx))
            .map_err(|_| "Articulate is busy. Try again.".to_owned())?;
        rx.recv()
            .map_err(|_| "The application controller stopped.".to_owned())?
    }

    pub(crate) fn shutdown(&self) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        match self.sender.try_send(Request::Shutdown(tx)) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Disconnected(_)) => return self.stopped_controller_close(),
            Err(mpsc::TrySendError::Full(_)) => {
                return Err("Articulate is busy. Try closing again.".into());
            }
        }
        rx.recv()
            .unwrap_or_else(|_| self.stopped_controller_close())
    }

    fn stopped_controller_close(&self) -> Result<(), String> {
        if self.startup_failed.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err("The application controller stopped. Unsaved work could not be verified.".into())
        }
    }
}

fn export_text(text: &str, extension: &str) -> Result<(), String> {
    if text.len() > 16 * 1024 * 1024 || !["txt", "md", "srt", "vtt"].contains(&extension) {
        return Err("Unsupported export.".into());
    }
    crate::export_file::save(text, extension).map_err(|error| error.to_string())?;
    Ok(())
}

fn rows(rows: &[calls::Row], names: &[String]) -> Vec<Value> {
    rows.iter()
        .map(|row| {
            json!({"start_ms": row.start_ms, "end_ms": row.end_ms,
        "speaker": calls::label(row, names), "speaker_labels": calls::speaker_labels(row, names),
        "text": row.text, "cues": row.cues, "provisional": false})
        })
        .collect()
}

fn session_view(session: &crate::history::Session) -> Value {
    json!({"id": session.id, "title": session.title, "kind": session.kind,
        "text": session.text, "original": session.original, "personal_notes": session.personal_notes,
        "audio_packets_lost": session.audio_packets_lost,
        "rows": rows(&session.rows, &session.speaker_names), "created_ms": session.created_ms,
        "updated_ms": session.updated_ms,"topic_id":session.topic_id,"is_collection":session.is_collection,"can_receive_sources":session.can_receive_sources(),
        "sources":session.sources.iter().map(|source|json!({"id":source.id,"title":source.title,"kind":source.kind,"created_ms":source.created_ms,"preview":source.text.chars().take(160).collect::<String>()})).collect::<Vec<_>>()})
}

impl App {
    fn desktop_snapshot(&self) -> Value {
        let note_capture = self.is_note_capture();
        let note_recording = note_capture && self.call.is_some();
        let note_stopping = note_recording
            && self
                .call
                .as_ref()
                .is_some_and(|c| c.stop_ns.load(Ordering::SeqCst) != 0);
        let note_listening =
            note_recording && !note_stopping && self.capture_feedback.is_recording();
        let overlay_recording =
            (self.target.is_some() && self.recording.is_some()) || note_listening;
        let overlay_finishing = (self.target.is_some() && self.busy) || note_stopping;
        let overlay_visible = overlay_recording
            || overlay_finishing
            || self
                .overlay_until
                .is_some_and(|until| until > Instant::now());
        let overlay_message = if note_listening {
            format!(
                "Recording a note · {} to finish",
                self.settings.quick_note_hotkey.label()
            )
        } else if overlay_recording {
            self.shortcut_instruction()
        } else if overlay_finishing {
            if note_stopping {
                "Recording stopped. Finishing your spoken note…"
            } else {
                "Finishing your transcript…"
            }
            .into()
        } else {
            self.overlay_message.clone()
        };
        let mut call_rows = rows(&self.call_rows, &self.speaker_names);
        for (index, row) in call_rows.iter_mut().enumerate() {
            row["provisional"] = json!(self.call_row_is_provisional(index));
        }
        let note_session = self
            .history
            .call
            .as_ref()
            .filter(|_| note_capture)
            .map(|session| {
                let mut view = session_view(session);
                view["rows"] = json!(call_rows);
                view["text"] = json!(calls::text(&self.call_rows, &self.speaker_names));
                view
            });
        let insights = self.insights.report.as_ref().map(|r| json!({
            "words":r.words,"sessions":r.sessions,"words_per_minute":r.words_per_minute,
            "current_streak":r.current_streak,"longest_streak":r.longest_streak,"active_days":r.active_days,
            "dictionary_replacements":r.dictionary_replacements,"cleanup_edits":r.cleanup_edits,
            "apps":r.apps.iter().map(|a|json!({"app":a.app,"words":a.words,"sessions":a.sessions})).collect::<Vec<_>>(),
            "days":r.days.iter().map(|d|json!({"date":d.date,"words":d.words,"sessions":d.sessions})).collect::<Vec<_>>()
        }));
        let settings = json!({
            "clean_speech":self.settings.clean_speech,"insert":self.settings.insert,"live_insert":self.settings.live_insert,
            "cpu":self.settings.cpu,"transcription_language":self.settings.transcription_language,"audio_context":self.settings.audio_context,
            "learn_corrections":self.settings.learn_corrections,"audio_feedback":self.settings.audio_feedback,
            "microphone":self.settings.microphone,"output":self.settings.output,"hotkey":self.settings.hotkey,
            "quick_note_hotkey":self.settings.quick_note_hotkey,"quick_note_hotkey_pending":self.quick_note_hotkey_pending,"quick_note_shortcut_error":self.quick_note_shortcut_error,
            "hotkey_mode":self.settings.hotkey_mode,"discord_auto_connect":self.settings.discord_auto_connect,
            "discord_auto_transcribe":self.settings.discord_auto_transcribe,"discord_companion":self.settings.discord_companion
            ,"vencord_auto_update":self.settings.vencord_auto_update
        });
        let mut snapshot = json!({
            "ready":self.ready,"loading":self.loading,"busy":self.busy,"status":self.status,
            "overlay":{"visible":overlay_visible,"recording":overlay_recording,"finishing":overlay_finishing,"message":overlay_message},
            "recording":self.recording.is_some(),"call_recording":self.call.is_some() && !note_capture,"call_status":self.call_status,
            "call_cues_enabled":self.call_cues_enabled && self.history.call.as_ref().is_some_and(|session| session.kind == crate::history::Kind::Call),
            "call_cues_status":if self.call_cues_enabled && self.history.call.as_ref().is_some_and(|session| session.kind == crate::history::Kind::Call) {self.context_status.as_str()} else {""},
            "seconds":self.recording.as_ref().map(|r|r.seconds() as u64).or_else(||self.call.as_ref().map(|c|c.end_seconds() as u64)).unwrap_or(self.last_seconds),
            "text":self.text,"original":self.raw,"call_rows":if note_capture {Vec::new()}else{call_rows},
            "history":self.history.items.iter().map(|s|json!({"id":s.id,"title":s.title,"kind":s.kind,"created_ms":s.created_ms,"updated_ms":s.updated_ms,"preview":s.preview,"duration_ms":s.duration_ms,"topic_id":s.topic_id,"is_collection":s.is_collection,"can_receive_sources":s.can_receive_sources})).collect::<Vec<_>>(),
            "selected":self.history.selected.as_ref().map(session_view),
            "dictation_session":self.history.dictation.as_ref().filter(|_| !self.history.dictation_deleted).map(session_view),
            "call_session":self.history.call.as_ref().filter(|_|!note_capture).map(session_view),
            "notes":self.desktop_notes_status(self.history.selected.as_ref().or(self.history.call.as_ref()).map(|s|s.id.as_str()).unwrap_or("")),
            "selected_notes":self.desktop_notes_status(self.history.selected.as_ref().map(|s|s.id.as_str()).unwrap_or("")),
            "call_notes":self.desktop_notes_status(self.history.call.as_ref().map(|s|s.id.as_str()).unwrap_or("")),
            "downloading":self.downloading,
            "history_loading":self.history.loading,"history_error":self.history.error,
            "saving":self.history.selected_dirty.is_some() || self.history.call_dirty.is_some() || self.history.dictation_dirty.is_some() || self.history.worker.as_ref().is_some_and(|w|w.saves_settled()==Ok(false)),
            "dictionary":self.settings.entries,"macros":self.settings.macros,
            "settings":settings,"microphones":self.microphones,"outputs":self.outputs,"insights":insights
        });
        snapshot["note_recording"] = json!(note_recording);
        snapshot["filing_status"] = json!(self.notetaker.status);
        snapshot["filing_working"] = json!(self.notetaker_routing_busy());
        snapshot["source_move_working"] =
            json!(self.notetaker.moving || !self.history.pending_deletions.is_empty());
        snapshot["summary_working"] = json!(self.brain_working());
        let cleanup = self.desktop_cleanup_status();
        let model_path = std::path::Path::new(&self.settings.model_path);
        let model_name = (model_path.is_absolute()
            && model_path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf")))
        .then(|| model_path.file_name().map(|name| name.to_string_lossy()))
        .flatten();
        snapshot["model_status"] = json!({"speech_installed":PathBuf::from(&self.settings.model_path).is_file(),"backend":self.backend,
            "model_name":model_name,"speech_languages":self.speech_languages,
            "model_custom":std::path::Path::new(&self.settings.model_path) != model::default_path(),
            "speakers_installed":crate::speakers::path().is_file(),"speakers_working":self.speakers_downloading.is_some(),
            "context_installed":crate::sensevoice::installed(),"context_progress":self.context_progress,"context_status":self.context_status,"context_supported":cfg!(all(windows, target_arch="x86_64")),
            "speakers_progress":self.speakers_downloading,"speakers_status":self.speakers_download_status,
            "verifier_installed":model::verifier_path().is_file(),"verifier_working":self.verifier_downloading.is_some(),
            "verifier_progress":self.verifier_downloading,"verifier_status":self.verifier_download_status,
            "cleanup_installed":cleanup["installed"],"cleanup_working":cleanup["working"],"cleanup_status":cleanup["status"],"cleanup_progress":cleanup["progress"]});
        snapshot["discord"] = self.desktop_discord_status();
        snapshot["capture_feedback"] = json!(self.capture_feedback);
        snapshot["updater"] = match &self.updater.status {
            crate::update::Status::Idle => json!({"state":"idle"}),
            crate::update::Status::Checking => json!({"state":"checking"}),
            crate::update::Status::Latest => json!({"state":"latest"}),
            crate::update::Status::Available(release) => {
                json!({"state":"available","version":release.version,"size":release.size})
            }
            crate::update::Status::Downloading { version, progress } => {
                json!({"state":"downloading","version":version,"progress":progress})
            }
            crate::update::Status::Ready(installer) => {
                json!({"state":"ready","version":installer.version})
            }
            crate::update::Status::Error(error) => json!({"state":"error","error":error}),
        };
        snapshot["note_rows"] = note_session
            .as_ref()
            .map(|s| s["rows"].clone())
            .unwrap_or_else(|| json!([]));
        snapshot["note_session"] = json!(note_session);
        snapshot["note_status"] = json!(if note_capture {
            self.call_status.as_str()
        } else {
            ""
        });
        snapshot["note_notes"] = self.desktop_notes_status(
            self.history
                .call
                .as_ref()
                .filter(|_| note_capture)
                .map(|s| s.id.as_str())
                .unwrap_or(""),
        );
        snapshot
    }

    fn desktop_action(&mut self, action: Action) -> Result<(), String> {
        use Action::*;
        match action {
            HistoryRefresh => {
                if let Some(w) = &self.history.worker {
                    w.list();
                }
            }
            HistoryOpen { id } => {
                if self.call.is_some() && self.history.call.as_ref().is_some_and(|s| s.id == id) {
                    self.history_save_personal_notes();
                    self.history.selected = self.history.call.clone();
                    self.history.open_requested = None;
                } else {
                    self.notetaker_open(id);
                }
            }
            HistoryClose => self.notetaker_hub(),
            NoteNew => self.notetaker_new_note(),
            NoteCaptureStart => self.notetaker_start_capture()?,
            NoteCaptureStop => self.notetaker_stop_capture()?,
            SourceMove {
                id,
                topic_id,
                expected_topic_id,
            } => {
                if self.call.is_some()
                    || self.recording.is_some()
                    || self.busy
                    || self.brain_working()
                    || self.notetaker_routing_busy()
                {
                    return Err(
                        "Wait for recording and notes to finish before moving this source.".into(),
                    );
                }
                self.history_save_personal_notes();
                self.notetaker_forget_filing(&id);
                let destination = topic_id
                    .map(crate::topics::Destination::Existing)
                    .unwrap_or(crate::topics::Destination::Unfiled);
                self.history
                    .worker
                    .as_ref()
                    .ok_or("Local history is unavailable.")?
                    .move_source(id, destination, expected_topic_id);
                self.notetaker.moving = true;
                self.notetaker.status = "Moving the recording and updating its notes…".into();
            }
            SourceFile { id } => {
                let session = self
                    .history
                    .selected
                    .as_ref()
                    .filter(|session| session.id == id)
                    .or_else(|| {
                        self.history
                            .call
                            .as_ref()
                            .filter(|session| session.id == id)
                    })
                    .cloned()
                    .ok_or("Open the recording before filing it.")?;
                self.notetaker_queue_filing(session);
            }
            HistoryRetry => self.history_retry(),
            SessionPatch {
                id,
                title,
                personal_notes,
            } => {
                if self.notetaker.moving || !self.history.pending_deletions.is_empty() {
                    return Err(
                        "The recording is moving. Try saving your edit again in a moment.".into(),
                    );
                }
                if self.history.pending_deletions.contains(&id) {
                    return Err("This document is being deleted.".into());
                }
                let mut session = self
                    .history
                    .call
                    .as_ref()
                    .filter(|s| s.id == id)
                    .or_else(|| self.history.dictation.as_ref().filter(|s| s.id == id))
                    .or_else(|| self.history.selected.as_ref().filter(|s| s.id == id))
                    .cloned()
                    .ok_or("Open the document before editing it.")?;
                if let Some(title) = title {
                    let title = title.trim();
                    if title.is_empty()
                        || title.chars().count() > 160
                        || title.chars().any(char::is_control)
                    {
                        return Err("Use a title from 1 to 160 characters.".into());
                    }
                    session.title = title.into();
                    session.title_is_manual = true;
                }
                if let Some(notes) = personal_notes {
                    if notes.len() > 8 * 1024 * 1024 {
                        return Err("This note is too long.".into());
                    }
                    session.personal_notes = notes;
                }
                self.brain.remember(&session);
                for current in [
                    &mut self.history.call,
                    &mut self.history.dictation,
                    &mut self.history.selected,
                ]
                .into_iter()
                .flatten()
                {
                    if current.id == id {
                        current.title.clone_from(&session.title);
                        current.title_is_manual = session.title_is_manual;
                        current.personal_notes.clone_from(&session.personal_notes);
                    }
                }
                if let Some(w) = &self.history.worker {
                    w.save(session);
                } else {
                    return Err("Local history is unavailable.".into());
                }
            }
            SessionDelete { id } => {
                if self.notetaker_routing_busy() {
                    return Err(
                        "Wait for topic filing to finish before deleting a recording.".into(),
                    );
                }
                if self.call.is_some() && self.history.call.as_ref().is_some_and(|s| s.id == id) {
                    return Err("Finish recording before deleting this call.".into());
                }
                self.history_save_personal_notes();
                if !self.history_delete(&id) {
                    return Err("Local history is unavailable.".into());
                }
            }
            DictationStart => {
                self.desktop_capture_idle()?;
                self.toggle(None);
                if self.recording.is_none() {
                    return Err(self.status.clone());
                }
            }
            DictationStop => {
                if self.recording.is_some() {
                    self.toggle(None);
                }
            }
            DictationEdit {
                id,
                expected_text,
                text,
                correction,
            } => {
                self.edit_saved_dictation(&id, &expected_text, text, correction)?;
            }
            DictationCancel => {
                if self.call.is_some() {
                    return Err("Finish the call using its recording control.".into());
                }
                self.cancel_dictation();
            }
            CallStart {
                source,
                language_preference,
                audio_context,
            } => {
                self.desktop_capture_idle()?;
                if language_preference.as_ref().is_some_and(|language| {
                    !self
                        .speech_languages
                        .iter()
                        .any(|supported| supported == language)
                }) {
                    return Err("Choose a supported call language preference.".into());
                }
                if audio_context == Some(true) && !crate::sensevoice::installed() {
                    return Err("Install sound and tone cues in Settings first.".into());
                }
                if !self.start_manual_call_capture(
                    matches!(source, CallSource::Discord),
                    language_preference,
                    audio_context,
                ) {
                    return Err(self.call_status.clone());
                }
            }
            CallStop => {
                self.discord_launch.automation.manual_finish();
                if let Some(c) = &self.call {
                    c.stop();
                }
            }
            Copy { text } => {
                if text.len() > 8 * 1024 * 1024 {
                    return Err("Text is too long.".into());
                }
                platform::copy(&text).map_err(|e| e.to_string())?;
            }
            Export { text, extension } => {
                export_text(&text, &extension)?;
            }
            DictionarySave {
                heard,
                wanted,
                app,
                cues,
                index,
                expected_heard,
                expected_entry,
            } => {
                let original = if let Some(index) = index {
                    Some(
                        self.settings
                            .entries
                            .get(index)
                            .filter(|e| {
                                Some(e.heard.as_str()) == expected_heard.as_deref()
                                    && serde_json::to_value(e).ok().as_ref()
                                        == expected_entry.as_ref()
                            })
                            .cloned()
                            .ok_or("Vocabulary changed. Refresh and try again.")?,
                    )
                } else {
                    None
                };
                let mut entry = dictionary::validate(&heard, &wanted).map_err(|e| e.to_string())?;
                entry.app = dictionary::app_scope(app.as_deref().unwrap_or(""))
                    .map_err(|e| e.to_string())?;
                entry.cues = dictionary::cue_words(cues.as_deref().unwrap_or(""))
                    .map_err(|e| e.to_string())?;
                if let Some(original) = &original {
                    entry.enabled = original.enabled;
                    entry.ignore_case = original.ignore_case;
                    entry.contexts.clone_from(&original.contexts);
                    if cues.is_none() {
                        entry.cues.clone_from(&original.cues);
                    }
                    if app.is_none() {
                        entry.app.clone_from(&original.app);
                    }
                }
                dictionary::save(&mut self.settings.entries, entry, original.as_ref());
                self.save_preferences().map_err(|e| e.to_string())?;
            }
            DictionaryDelete {
                index,
                expected_heard,
                expected_entry,
            } => {
                if self.settings.entries.get(index).is_none_or(|e| {
                    e.heard != expected_heard
                        || serde_json::to_value(e).ok().as_ref() != expected_entry.as_ref()
                }) {
                    return Err("Vocabulary changed. Refresh and try again.".into());
                }
                self.settings.entries.remove(index);
                self.save_preferences().map_err(|e| e.to_string())?;
            }
            MacroSave {
                trigger,
                expansion,
                app,
                index,
                expected_trigger,
                expected_macro,
            } => {
                let mut item = macros::validate(&trigger, &expansion, app.as_deref().unwrap_or(""))
                    .map_err(|e| e.to_string())?;
                if let Some(index) = index {
                    if self.settings.macros.get(index).is_none_or(|m| {
                        Some(m.trigger.as_str()) != expected_trigger.as_deref()
                            || serde_json::to_value(m).ok().as_ref() != expected_macro.as_ref()
                    }) {
                        return Err("Snippets changed. Refresh and try again.".into());
                    }
                    if self
                        .settings
                        .macros
                        .iter()
                        .enumerate()
                        .any(|(i, m)| i != index && m.trigger == item.trigger && m.app == item.app)
                    {
                        return Err("A snippet already uses this trigger in that app.".into());
                    }
                    item.enabled = self.settings.macros[index].enabled;
                    self.settings.macros[index] = item;
                } else if let Some(old) = self
                    .settings
                    .macros
                    .iter_mut()
                    .find(|m| m.trigger == item.trigger && m.app == item.app)
                {
                    item.enabled = old.enabled;
                    *old = item;
                } else {
                    self.settings.macros.push(item);
                }
                self.save_preferences().map_err(|e| e.to_string())?;
            }
            MacroDelete {
                index,
                expected_trigger,
                expected_macro,
            } => {
                if self.settings.macros.get(index).is_none_or(|m| {
                    m.trigger != expected_trigger
                        || serde_json::to_value(m).ok().as_ref() != expected_macro.as_ref()
                }) {
                    return Err("Snippets changed. Refresh and try again.".into());
                }
                self.settings.macros.remove(index);
                self.save_preferences().map_err(|e| e.to_string())?;
            }
            SettingsPatch {
                cpu,
                model_path,
                transcription_language,
                clean_speech,
                insert,
                live_insert,
                learn_corrections,
                audio_feedback,
                audio_context,
                microphone,
                output,
                hotkey_mode,
                hotkey,
                quick_note_hotkey,
                discord_auto_connect,
                discord_auto_transcribe,
                vencord_auto_update,
            } => {
                if self.recording.is_some() || self.call.is_some() || self.busy {
                    return Err("Finish recording before changing preferences.".into());
                }
                if (cpu.is_some() || model_path.is_some() || transcription_language.is_some())
                    && self.loading
                {
                    return Err("Wait for the current model to finish loading.".into());
                }
                if let Some(path) = &model_path {
                    let path = std::path::Path::new(path.trim());
                    if !path.is_absolute()
                        || !path.is_file()
                        || !path
                            .extension()
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"))
                    {
                        return Err(
                            "Choose an existing local GGUF model file using its full path.".into(),
                        );
                    }
                }
                if let Some(language) = &transcription_language
                    && !language.is_empty()
                    && !self.speech_languages.contains(language)
                {
                    return Err("Choose a language supported by the loaded speech model.".into());
                }
                let reload_model = transcription_language
                    .as_ref()
                    .is_some_and(|language| language != &self.settings.transcription_language)
                    || cpu.is_some_and(|value| value != self.settings.cpu)
                    || model_path
                        .as_ref()
                        .is_some_and(|path| path.trim() != self.settings.model_path);
                if let Some(requested) = &hotkey {
                    requested.validate()?;
                    if requested
                        == quick_note_hotkey
                            .as_ref()
                            .unwrap_or(&self.settings.quick_note_hotkey)
                    {
                        return Err(
                            "Choose different shortcuts for dictation and quick notes.".into()
                        );
                    }
                }
                if let Some(requested) = &quick_note_hotkey {
                    requested.validate()?;
                    if requested == hotkey.as_ref().unwrap_or(&self.settings.hotkey) {
                        return Err(
                            "Choose different shortcuts for dictation and quick notes.".into()
                        );
                    }
                }
                if let Some(hotkey) = quick_note_hotkey {
                    self.quick_note_hotkey_tx
                        .send(hotkey)
                        .map_err(|_| "The quick-note shortcut service stopped.")?;
                    self.quick_note_hotkey_pending = true;
                    self.quick_note_shortcut_error = None;
                }
                if let Some(hotkey) = hotkey {
                    hotkey.validate()?;
                    self.hotkey_tx
                        .send(hotkey.clone())
                        .map_err(|_| "The shortcut service stopped.")?;
                    self.hotkey_draft = hotkey;
                    self.hotkey_pending = true;
                }
                if let Some(v) = clean_speech {
                    self.settings.clean_speech = v;
                }
                if let Some(v) = insert {
                    self.settings.insert = v;
                }
                if let Some(v) = live_insert {
                    self.settings.live_insert = v;
                }
                if let Some(v) = learn_corrections {
                    self.desktop_set_learning(v);
                }
                if let Some(v) = audio_feedback {
                    self.settings.audio_feedback = v;
                }
                if let Some(v) = hotkey_mode {
                    self.settings.hotkey_mode = v;
                }
                if let Some(v) = discord_auto_connect {
                    self.settings.discord_auto_connect = v;
                }
                if let Some(v) = discord_auto_transcribe {
                    self.settings.discord_auto_transcribe = v;
                }
                if audio_context == Some(true) && !crate::sensevoice::installed() {
                    return Err("Install the audio context model first.".into());
                }
                if let Some(v) = audio_context {
                    self.settings.audio_context = v;
                }
                if let Some(v) = vencord_auto_update {
                    self.settings.vencord_auto_update = v;
                }
                if let Some(v) = microphone {
                    self.settings.microphone = (!v.is_empty()).then_some(v);
                }
                if let Some(v) = output {
                    self.settings.output = (!v.is_empty()).then_some(v);
                }
                if let Some(language) = transcription_language {
                    self.settings.transcription_language = language;
                }
                if let Some(cpu) = cpu {
                    self.settings.cpu = cpu;
                }
                if let Some(path) = model_path {
                    self.settings.model_path = path.trim().into();
                }
                self.save_preferences().map_err(|e| e.to_string())?;
                if reload_model && PathBuf::from(&self.settings.model_path).is_file() {
                    self.load();
                }
            }
            InsightsRefresh => {
                if let Some(w) = &self.history.worker {
                    w.insights();
                    self.insights.loading = true;
                }
            }
            ModelDownload => {
                if self.downloading.is_some()
                    || self.recording.is_some()
                    || self.call.is_some()
                    || self.busy
                    || self.loading
                {
                    return Err("Finish the current task before downloading a model.".into());
                }
                self.download();
            }
            ModelLoad => {
                if self.recording.is_some() || self.call.is_some() || self.busy || self.loading {
                    return Err("Finish the current task first.".into());
                }
                self.load();
            }
            NotesGenerate { id } => {
                if self.history.pending_deletions.contains(&id) {
                    return Err("This document is being deleted.".into());
                }
                let mut session = self
                    .history
                    .call
                    .as_ref()
                    .filter(|s| s.id == id)
                    .or_else(|| self.history.selected.as_ref().filter(|s| s.id == id))
                    .cloned()
                    .ok_or("Open the document before updating notes.")?;
                if self.call.is_some() && self.history.call.as_ref().is_some_and(|s| s.id == id) {
                    session.rows.clone_from(&self.call_committed);
                    session.text = calls::text(&session.rows, &session.speaker_names);
                }
                self.desktop_notes_generate(session)?;
            }
            SummaryDownload => self.desktop_summary_download()?,
            SummaryDownloadCancel => self.desktop_summary_download_cancel(),
            CleanupDownload => self.desktop_cleanup_download()?,
            CleanupDownloadCancel => self.desktop_cleanup_cancel(),
            ContextDownload => self.desktop_context_download()?,
            SpeakersDownload => self.desktop_speakers_download()?,
            VerifierDownload => self.desktop_verifier_download()?,
            DiscordConnect { companion } => self.desktop_discord_connect(companion)?,
            DiscordDisconnect => self.desktop_discord_disconnect()?,
            DiscordRetry => self.desktop_discord_retry()?,
            DiscordRelaunch => self.desktop_discord_relaunch()?,
            CompanionDetect => self.desktop_companion_action("detect")?,
            CompanionInstall => self.desktop_companion_action("install")?,
            CompanionOpenInstaller => self.desktop_companion_action("open_installer")?,
            UpdateCheck => self.updater.check(),
            UpdateDownload => {
                let crate::update::Status::Available(release) = &self.updater.status else {
                    return Err("Check for an available update first.".into());
                };
                self.updater.download(release.clone());
            }
            UpdateInstall => {
                let crate::update::Status::Ready(installer) = &self.updater.status else {
                    return Err("Download the update before installing it.".into());
                };
                let installer = installer.clone();
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

    fn desktop_set_learning(&mut self, enabled: bool) {
        self.settings.learn_corrections = enabled;
        if !enabled {
            let _ = self.integration_tx.send(integration::Action::StopLearning);
            self.edited_at = None;
            self.edit_baseline.clone_from(&self.text);
        }
    }

    fn desktop_context_download(&mut self) -> Result<(), String> {
        if self.context_progress.is_some() || self.call.is_some() || self.recording.is_some() {
            return Err("Finish recording or the current audio context download first.".into());
        }
        self.context_progress = Some(0.0);
        self.context_status = "Downloading audio context".into();
        let sender = self.event_tx.clone();
        std::thread::Builder::new()
            .name("audio-context-install".into())
            .spawn(move || {
                let mut last = Instant::now() - Duration::from_secs(1);
                let result = crate::sensevoice::install(|p| {
                    if last.elapsed() >= Duration::from_millis(100) {
                        let _ = sender.send(Event::ContextProgress(p));
                        last = Instant::now();
                    }
                })
                .map_err(|e| e.to_string());
                let _ = sender.send(Event::ContextInstalled(result));
            })
            .map_err(|e| {
                self.context_progress = None;
                e.to_string()
            })?;
        Ok(())
    }

    fn desktop_speakers_download(&mut self) -> Result<(), String> {
        if self.speakers_downloading.is_some() || self.call.is_some() || self.recording.is_some() {
            return Err("Finish the current recording or speaker download first.".into());
        }
        self.speakers_downloading = Some(0.0);
        self.speakers_download_status = "Downloading speaker identification…".into();
        let sender = self.event_tx.clone();
        std::thread::Builder::new()
            .name("speaker-model-download".into())
            .spawn(move || {
                let mut last = Instant::now() - Duration::from_secs(1);
                let result = model::download_speakers(|progress| {
                    if last.elapsed() >= Duration::from_millis(100) {
                        let _ = sender.send(Event::SpeakersProgress(progress));
                        last = Instant::now();
                    }
                });
                let _ = sender.send(match result {
                    Ok(_) => Event::SpeakersDownloaded,
                    Err(error) => Event::SpeakersDownloadFailed(error.to_string()),
                });
            })
            .map_err(|error| {
                self.speakers_downloading = None;
                error.to_string()
            })?;
        Ok(())
    }

    fn desktop_verifier_download(&mut self) -> Result<(), String> {
        if self.verifier_downloading.is_some()
            || self.call.is_some()
            || self.recording.is_some()
            || self.busy
            || self.loading
        {
            return Err("Finish the current recording or model download first.".into());
        }
        self.verifier_downloading = Some(0.0);
        self.verifier_download_status = "Downloading second speech model…".into();
        let sender = self.event_tx.clone();
        std::thread::Builder::new()
            .name("verifier-model-download".into())
            .spawn(move || {
                let mut last = Instant::now() - Duration::from_secs(1);
                let result = model::download_verifier(|progress| {
                    if last.elapsed() >= Duration::from_millis(100) {
                        let _ = sender.send(Event::VerifierProgress(progress));
                        last = Instant::now();
                    }
                });
                let _ = sender.send(match result {
                    Ok(_) => Event::VerifierDownloaded,
                    Err(error) => Event::VerifierDownloadFailed(error.to_string()),
                });
            })
            .map_err(|error| {
                self.verifier_downloading = None;
                error.to_string()
            })?;
        Ok(())
    }

    fn desktop_capture_idle(&self) -> Result<(), String> {
        if !self.ready
            || self.loading
            || self.busy
            || self.recording.is_some()
            || self.call.is_some()
            || self.downloading.is_some()
            || self.verifier_downloading.is_some()
            || self.preview_inflight
        {
            Err("Wait for the speech model and finish the current recording first.".into())
        } else {
            Ok(())
        }
    }

    fn desktop_shutdown(&mut self) -> Result<(), String> {
        if self.recording.is_some()
            || self.call.is_some()
            || self.busy
            || self.preview_inflight
            || self.integration_inflight
            || self.brain_working()
            || self.polish_working()
            || self.notetaker_routing_busy()
        {
            return Err(
                "Finish recording and wait for processing before closing Articulate.".into(),
            );
        }
        self.history_save_personal_notes();
        self.history_save_dictation();
        self.history_save_call();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.history_poll();
            if self.install_saves_settled()? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("Changes are still saving. Try closing again in a moment.".into());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_actions_require_ready_state_before_external_changes() {
        let (mut app, _) = super::super::tests::app();
        assert!(app.desktop_action(Action::UpdateDownload).is_err());
        assert!(app.desktop_action(Action::UpdateInstall).is_err());
        assert!(app.pending_install.is_none());
        app.call = Some(call_capture::Control::new());
        assert!(app.desktop_action(Action::DiscordRelaunch).is_err());
        assert!(app.desktop_action(Action::CompanionInstall).is_err());
        assert!(!app.settings.discord_companion);
    }

    #[test]
    fn invalid_model_selection_does_not_change_settings_or_expose_private_path() {
        let (mut app, _) = super::super::tests::app();
        let action: Action = serde_json::from_value(
            json!({"type":"settings_patch", "cpu":true,"model_path":"relative.gguf"}),
        )
        .unwrap();
        assert!(app.desktop_action(action).is_err());
        assert!(!app.settings.cpu);
        app.settings.model_path = std::env::temp_dir()
            .join("synthetic-private-model-folder")
            .join("speech.gguf")
            .to_string_lossy()
            .into_owned();
        let snapshot = app.desktop_snapshot();
        assert_eq!(snapshot["model_status"]["model_name"], "speech.gguf");
        assert!(
            !snapshot
                .to_string()
                .contains("synthetic-private-model-folder")
        );
    }

    #[test]
    fn moving_a_source_temporarily_protects_unsaved_editor_changes() {
        let (mut app, _) = super::super::tests::app();
        let mut note = crate::history::Session::new(crate::history::Kind::Note);
        note.personal_notes = "My existing words.".into();
        app.history.selected = Some(note.clone());
        app.notetaker.moving = true;
        assert!(
            app.desktop_action(Action::SessionPatch {
                id: note.id,
                title: None,
                personal_notes: Some("A concurrent edit.".into()),
            })
            .is_err()
        );
        assert_eq!(
            app.history.selected.as_ref().unwrap().personal_notes,
            "My existing words."
        );
        assert_eq!(app.desktop_snapshot()["source_move_working"], true);
        app.notetaker.moving = false;
        app.history
            .pending_deletions
            .insert("a-source-being-deleted".into());
        assert!(
            app.desktop_action(Action::SessionPatch {
                id: app.history.selected.as_ref().unwrap().id.clone(),
                title: None,
                personal_notes: Some("An edit while deleting a child source.".into()),
            })
            .is_err()
        );
        assert_eq!(
            app.history.selected.as_ref().unwrap().personal_notes,
            "My existing words."
        );
        assert_eq!(app.desktop_snapshot()["source_move_working"], true);
    }

    #[test]
    fn spoken_note_snapshot_stays_separate_from_calls_and_dictation() {
        let (mut app, _) = super::super::tests::app();
        app.history.call = Some(crate::history::Session::new(crate::history::Kind::Note));
        app.call = Some(call_capture::Control::new());
        app.call_rows.push(calls::Row {
            cues: Vec::new(),
            start_ms: 0,
            end_ms: 1000,
            microphone: true,
            speakers: Vec::new(),
            discord: None,
            text: "An evolving thought.".into(),
        });
        let state = app.desktop_snapshot();
        assert_eq!(state["note_recording"], true);
        assert_eq!(state["call_recording"], false);
        assert_eq!(state["recording"], false);
        assert_eq!(state["note_rows"][0]["text"], "An evolving thought.");
        assert!(state["call_rows"].as_array().unwrap().is_empty());
        assert!(state["call_session"].is_null());
        assert_eq!(state["note_session"]["kind"], "note");
    }

    #[test]
    fn transcription_language_is_validated_saved_and_sent_to_the_speech_worker() {
        let (mut app, commands) = super::super::tests::app();
        let model_path = model::data_dir().join("language-test.gguf");
        std::fs::create_dir_all(model::data_dir()).unwrap();
        std::fs::write(
            &model_path,
            b"synthetic model path; no inference worker in this test",
        )
        .unwrap();
        app.settings.model_path = model_path.to_string_lossy().into_owned();
        let change = |language: &str| {
            serde_json::from_value::<Action>(
                json!({"type":"settings_patch", "transcription_language":language}),
            )
            .unwrap()
        };
        app.desktop_action(change("en")).unwrap();
        assert_eq!(app.settings.transcription_language, "en");
        assert!(
            matches!(commands.try_recv().unwrap(), Command::Load(_, _, Some(language)) if language == "en")
        );
        let restored: Settings =
            serde_json::from_slice(&serde_json::to_vec(&app.settings).unwrap()).unwrap();
        assert_eq!(restored.transcription_language, "en");
        app.loading = false;
        assert!(app.desktop_action(change("invalid")).is_err());
        assert_eq!(app.settings.transcription_language, "en");
        app.desktop_action(change("")).unwrap();
        assert!(matches!(
            commands.try_recv().unwrap(),
            Command::Load(_, _, None)
        ));
        assert!(
            serde_json::from_str::<Settings>("{}")
                .unwrap()
                .transcription_language
                .is_empty()
        );
    }

    #[test]
    fn disabling_learning_stops_field_observation_and_discards_pending_local_edit() {
        let (mut app, _) = super::super::tests::app();
        let (sender, events) = mpsc::channel();
        app.integration_tx = sender;
        app.text = "Synthetic corrected text.".into();
        app.edit_baseline = "Earlier text.".into();
        app.edited_at = Some(Instant::now());
        app.desktop_set_learning(false);
        assert!(!app.settings.learn_corrections);
        assert!(matches!(
            events.try_recv().unwrap(),
            integration::Action::StopLearning
        ));
        assert!(app.edited_at.is_none());
        assert_eq!(app.edit_baseline, app.text);
        app.desktop_set_learning(true);
        assert!(app.edited_at.is_none());
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn failed_initialization_can_close_but_runtime_failure_cannot_skip_save_checks() {
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(receiver);
        let bridge = Bridge {
            sender,
            snapshot: Arc::new(Mutex::new(json!({}))),
            frontend_seen: std::sync::atomic::AtomicBool::new(true),
            startup_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        assert!(bridge.shutdown().is_err());
        bridge.startup_failed.store(true, Ordering::Release);
        assert!(bridge.shutdown().is_ok());
    }

    #[test]
    fn renderer_snapshot_excludes_pairing_key_and_model_path() {
        let (mut app, _) = super::super::tests::app();
        app.settings.discord_pairing_key = "synthetic-private-pairing-value".into();
        app.settings.model_path = "synthetic-private-model-path".into();
        let snapshot = app.desktop_snapshot();
        let serialized = snapshot.to_string();
        assert!(!serialized.contains("synthetic-private"));
        assert!(snapshot["settings"]["clean_speech"].is_boolean());
        assert!(snapshot["history"].is_array());
    }

    #[test]
    fn renderer_cannot_supply_arbitrary_settings_or_actions() {
        assert!(
            serde_json::from_value::<Action>(
                json!({"type":"settings_patch", "discord_pairing_key":"replace"})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<Action>(json!({"type":"run_shell", "command":"anything"}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<Action>(
                json!({"type":"settings_patch", "clean_speech":false})
            )
            .is_ok()
        );
    }

    #[test]
    fn cancel_dictation_cannot_cancel_a_call() {
        let (mut app, _) = super::super::tests::app();
        app.call = Some(call_capture::Control::new());
        let token = app.cancel.clone();
        assert!(app.desktop_action(Action::DictationCancel).is_err());
        assert!(!token.is_cancelled());
    }

    #[test]
    fn document_commands_patch_only_notes_and_title_and_persist() {
        let directory = std::env::temp_dir().join(format!(
            "articulate-desktop-contract-{}",
            crate::history::Session::new(crate::history::Kind::Note).id
        ));
        let (mut app, _) = super::super::tests::app();
        app.history.worker = Some(crate::history::Worker::test_directory(directory.clone()));
        let mut session = crate::history::Session::new(crate::history::Kind::Dictation);
        session.text = "Synthetic transcript remains unchanged.".into();
        session.original = "Synthetic original remains unchanged.".into();
        let id = session.id.clone();
        app.history.selected = Some(session);
        app.desktop_action(Action::SessionPatch {
            id: id.clone(),
            title: Some("Reviewed document".into()),
            personal_notes: Some("A deliberately edited note.".into()),
        })
        .unwrap();
        let mut current = crate::history::Session::new(crate::history::Kind::Call);
        let call_id = current.id.clone();
        current.text = "Earlier source.".into();
        app.history.selected = Some(current.clone());
        current.text = "The latest source includes the completed call.".into();
        app.history.call = Some(current);
        app.desktop_action(Action::SessionPatch {
            id: call_id.clone(),
            title: None,
            personal_notes: Some("Keep the latest source.".into()),
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app
            .history
            .worker
            .as_ref()
            .unwrap()
            .saves_settled()
            .unwrap()
        {
            assert!(Instant::now() < deadline, "Isolated history save timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(app);
        let history = crate::history::History::open(directory.clone()).unwrap();
        let saved = history.load(&id).unwrap();
        assert_eq!(saved.text, "Synthetic transcript remains unchanged.");
        assert_eq!(saved.original, "Synthetic original remains unchanged.");
        assert_eq!(saved.title, "Reviewed document");
        assert!(saved.title_is_manual);
        assert_eq!(saved.personal_notes, "A deliberately edited note.");
        let saved_call = history.load(&call_id).unwrap();
        assert_eq!(
            saved_call.text,
            "The latest source includes the completed call."
        );
        assert_eq!(saved_call.personal_notes, "Keep the latest source.");
        drop(history);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn indicator_visibility_expires_without_focus_or_raw_text() {
        let (mut app, _) = super::super::tests::app();
        app.raw = "Unrelated synthetic private transcript".into();
        assert_eq!(app.desktop_snapshot()["overlay"]["visible"], false);
        app.overlay_message = "Ready to copy".into();
        app.overlay_until = Some(Instant::now() + Duration::from_secs(1));
        let snapshot = app.desktop_snapshot();
        assert_eq!(snapshot["overlay"]["visible"], true);
        assert_eq!(snapshot["overlay"]["message"], "Ready to copy");
        assert!(!snapshot["overlay"].to_string().contains(&app.raw));
        app.overlay_until = Some(Instant::now() - Duration::from_secs(1));
        assert_eq!(app.desktop_snapshot()["overlay"]["visible"], false);
    }

    #[test]
    fn stale_editor_cannot_delete_same_phrase_in_a_different_scope() {
        let (mut app, _) = super::super::tests::app();
        let original = dictionary::validate("project", "Project").unwrap();
        let mut other = original.clone();
        other.app = Some("editor.exe".into());
        app.settings.entries = vec![other.clone()];
        let result = app.desktop_action(Action::DictionaryDelete {
            index: 0,
            expected_heard: original.heard.clone(),
            expected_entry: Some(serde_json::to_value(&original).unwrap()),
        });
        assert!(result.is_err());
        assert!(app.settings.entries[0] == other);
        let original = macros::validate("signature", "A synthetic signature", "").unwrap();
        let mut other = original.clone();
        other.app = Some("editor.exe".into());
        app.settings.macros = vec![other];
        let result = app.desktop_action(Action::MacroDelete {
            index: 0,
            expected_trigger: original.trigger.clone(),
            expected_macro: Some(serde_json::to_value(original).unwrap()),
        });
        assert!(result.is_err());
        assert_eq!(app.settings.macros[0].app.as_deref(), Some("editor.exe"));
    }
}
