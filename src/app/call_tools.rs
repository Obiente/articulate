//! Calls and speakers in the controller: speaker corrections, known people,
//! interruption markers, retained audio and playback, audio import and
//! re-transcription.
use super::*;
use crate::{
    audio_store, file_pipeline,
    history::{Kind, Session, TranscriptVersion},
    people, playback,
    speaker_edits::{self, Edit, LiveRule, Target},
};
use std::sync::{Arc, Mutex};

/// A file offered for import by the open dialog or a drop on the window,
/// handed from the IPC or window thread to the controller.
static OFFERED: Mutex<Option<PathBuf>> = Mutex::new(None);
static DRAGGING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Offer files for import; the first supported one is kept. Returns whether
/// one was accepted.
pub(crate) fn offer_files(paths: &[PathBuf]) -> bool {
    DRAGGING.store(false, Ordering::Relaxed);
    let Some(path) = paths.iter().find(|path| crate::media::supported(path)) else {
        return false;
    };
    *OFFERED.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.clone());
    true
}

/// Files are being dragged over the window.
pub(crate) fn set_dragging(dragging: bool) {
    DRAGGING.store(dragging, Ordering::Relaxed);
}

/// Ask for an audio file with the system dialog. Runs outside the controller.
pub(crate) fn choose_file() -> Result<(), String> {
    match open_dialog().map_err(|error| error.to_string())? {
        Some(path) if crate::media::supported(&path) => {
            offer_files(&[path]);
            Ok(())
        }
        Some(_) => Err("Choose a WAV, MP3, M4A, FLAC, Ogg, MP4 or MKV file.".into()),
        None => Ok(()),
    }
}

#[cfg(windows)]
fn open_dialog() -> anyhow::Result<Option<PathBuf>> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let patterns = crate::media::EXTENSIONS
        .iter()
        .map(|extension| format!("*.{extension}"))
        .collect::<Vec<_>>()
        .join(";");
    let filter: Vec<u16> = format!("Audio and video files\0{patterns}\0All files\0*.*\0\0")
        .encode_utf16()
        .collect();
    let title: Vec<u16> = "Import audio".encode_utf16().chain(Some(0)).collect();
    let mut name = vec![0u16; 32768];
    // SAFETY: every buffer outlives the call and is NUL-terminated.
    unsafe {
        let mut dialog: OPENFILENAMEW = std::mem::zeroed();
        dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        dialog.lpstrFile = name.as_mut_ptr();
        dialog.nMaxFile = name.len() as u32;
        dialog.lpstrFilter = filter.as_ptr();
        dialog.lpstrTitle = title.as_ptr();
        dialog.Flags = OFN_PATHMUSTEXIST | OFN_FILEMUSTEXIST | OFN_NOCHANGEDIR;
        if GetOpenFileNameW(&mut dialog) == 0 {
            let error = CommDlgExtendedError();
            anyhow::ensure!(error == 0, "Could not open the file dialog ({error})");
            return Ok(None);
        }
    }
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    Ok(Some(PathBuf::from(std::ffi::OsString::from_wide(
        &name[..end],
    ))))
}

#[cfg(not(windows))]
fn open_dialog() -> anyhow::Result<Option<PathBuf>> {
    anyhow::bail!("Importing audio is not available on this platform.")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ImportKind {
    Call,
    Dictation,
}

/// Work for the speech worker: import a file or rebuild a transcript from
/// retained audio.
pub(super) enum FileJob {
    Import {
        path: PathBuf,
        kind: ImportKind,
        session: Box<Session>,
        /// Where to keep the audio, when the user chose to.
        keep: Option<(PathBuf, bool)>,
        cpu: bool,
    },
    Rebuild {
        session: Box<Session>,
        audio: PathBuf,
        model: String,
        cpu: bool,
    },
}

pub(super) enum FileEvent {
    Progress(f32, &'static str),
    Finished(Result<Box<Session>, String>),
}

/// Import progress, shown in the renderer.
#[derive(Default)]
pub(super) struct ImportState {
    /// The offered file waiting for the user to choose how to import it.
    pub pending: Option<PathBuf>,
    pub working: bool,
    /// Which session a running re-transcription belongs to.
    pub rebuilding: Option<String>,
    pub file_name: Option<String>,
    pub progress: Option<f32>,
    pub stage: &'static str,
    pub message: String,
    pub failed: bool,
    /// The saved session of the most recent import, to open it.
    pub session_id: Option<String>,
    pub cancel: Option<CancelToken>,
}

pub(super) struct State {
    pub people: people::Directory,
    pub history_dir: Option<PathBuf>,
    /// Corrections made while recording also apply to later turns.
    pub live_rules: Vec<LiveRule>,
    pub recorder: Option<Arc<audio_store::Recorder>>,
    pub import: ImportState,
    pub player: Option<playback::Player>,
    pub playback_error: Option<String>,
    /// The next capture's choice to keep audio, from call setup.
    pub keep_audio: Option<bool>,
    people_saved: Option<Instant>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            people: people::Directory::in_memory(),
            history_dir: None,
            live_rules: Vec::new(),
            recorder: None,
            import: ImportState::default(),
            player: None,
            playback_error: None,
            keep_audio: None,
            people_saved: None,
        }
    }
}

impl State {
    pub fn start(encrypt: bool) -> Self {
        Self {
            people: people::Directory::load(people::Directory::path(), encrypt),
            history_dir: Some(crate::history::directory()),
            ..Default::default()
        }
    }

    pub fn audio_path(&self, id: &str) -> Option<PathBuf> {
        self.history_dir
            .as_ref()
            .map(|directory| audio_store::path(directory, id))
    }
}

/// One speaker correction requested by the renderer.
pub(crate) enum SpeakerOp {
    Rename {
        speaker: String,
        name: String,
    },
    Merge {
        from: String,
        into: String,
    },
    Reassign {
        rows: Vec<(usize, u64)>,
        target: Target,
    },
    Undo,
}

fn file_title(path: &std::path::Path) -> String {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let title: String = stem.split_whitespace().collect::<Vec<_>>().join(" ");
    let title: String = title
        .chars()
        .filter(|c| !c.is_control())
        .take(160)
        .collect();
    if title.is_empty() {
        "Imported recording".into()
    } else {
        title
    }
}

/// Run a file job on the speech worker. Progress is reported through `update`.
pub(super) fn run_file_job(
    engine: &mut Engine,
    job: FileJob,
    cancel: &CancelToken,
    update: &mut dyn FnMut(FileEvent),
) -> anyhow::Result<Box<Session>> {
    match job {
        FileJob::Import {
            path,
            kind,
            mut session,
            keep,
            cpu,
        } => {
            let stop = std::sync::atomic::AtomicBool::new(false);
            let recorder = match &keep {
                Some((audio, protect)) => Some(audio_store::Recorder::start(
                    audio.clone(),
                    vec!["recording".into()],
                    *protect,
                )?),
                None => None,
            };
            let mut dictation = file_pipeline::Dictation::new(true);
            let (tracker, _) = if kind == ImportKind::Call {
                update(FileEvent::Progress(0.0, "Loading speaker identification"));
                file_pipeline::Call::speakers(cpu)
            } else {
                (None, None)
            };
            let mut call = file_pipeline::Call::new(tracker);
            let mut rows = Vec::new();
            let mut decoded = 0usize;
            let fraction = std::cell::Cell::new(0.0f32);
            update(FileEvent::Progress(0.0, "Transcribing"));
            let result = crate::media::stream_file(
                &path,
                crate::media::Layout::Mono,
                &stop,
                |progress| fraction.set(progress),
                |chunk| {
                    anyhow::ensure!(!cancel.is_cancelled(), "Import cancelled");
                    let pcm = chunk.into_iter().next().unwrap_or_default();
                    decoded += pcm.len();
                    if let Some(recorder) = &recorder {
                        recorder.push(0, pcm.clone());
                    }
                    match kind {
                        ImportKind::Call => {
                            calls::append_rows(&mut rows, call.push(engine, &[], &pcm)?);
                        }
                        ImportKind::Dictation => dictation.push(engine, &pcm)?,
                    }
                    update(FileEvent::Progress(
                        fraction.get().min(0.99),
                        "Transcribing",
                    ));
                    Ok(())
                },
            );
            if let Err(error) = result {
                if let Some(recorder) = recorder {
                    let _ = recorder.finish();
                }
                if let Some((audio, _)) = &keep {
                    let _ = std::fs::remove_file(audio);
                }
                return Err(error);
            }
            update(FileEvent::Progress(0.99, "Finishing"));
            let duration_ms = decoded as u64 / 16;
            match kind {
                ImportKind::Call => {
                    calls::append_rows(&mut rows, call.finish(engine)?);
                    session.rows = rows;
                    session.text = calls::text(&session.rows, &session.speaker_names);
                }
                ImportKind::Dictation => {
                    session.text = dictation.finish(engine)?;
                    session.original.clone_from(&session.text);
                    session.metrics.audio_duration_ms = Some(duration_ms);
                    session.metrics.recognized_words =
                        Some(session.text.split_whitespace().count() as u64);
                }
            }
            if let Some(recorder) = recorder {
                session.audio = Some(recorder.finish()?);
            }
            session.title = file_title(&path);
            session.title_is_manual = true;
            Ok(session)
        }
        FileJob::Rebuild {
            mut session,
            audio,
            model,
            cpu,
        } => {
            let info = session
                .audio
                .clone()
                .ok_or_else(|| anyhow::anyhow!("This recording has no retained audio"))?;
            let duration = info.duration_ms.max(1);
            let previous = TranscriptVersion::of(&session, &model);
            let microphone_channel = info.channels.iter().position(|c| c == "microphone");
            let remote_channel = info.channels.iter().position(|c| c != "microphone").or(
                if microphone_channel.is_none() {
                    Some(0)
                } else {
                    None
                },
            );
            match session.kind {
                Kind::Call => {
                    update(FileEvent::Progress(0.0, "Loading speaker identification"));
                    let (tracker, _) = file_pipeline::Call::speakers(cpu);
                    let mut call = file_pipeline::Call::new(tracker);
                    let mut rows = Vec::new();
                    audio_store::read(&audio, 0, |start, channels| {
                        anyhow::ensure!(!cancel.is_cancelled(), "Transcription cancelled");
                        let pick = |index: Option<usize>| {
                            index
                                .and_then(|index| channels.get(index))
                                .cloned()
                                .unwrap_or_default()
                        };
                        let microphone = pick(microphone_channel);
                        let remote = pick(remote_channel);
                        calls::append_rows(&mut rows, call.push(engine, &microphone, &remote)?);
                        update(FileEvent::Progress(
                            (start as f32 / 16.0 / duration as f32).min(0.99),
                            "Transcribing",
                        ));
                        Ok(true)
                    })?;
                    calls::append_rows(&mut rows, call.finish(engine)?);
                    session.rows = rows;
                    session.speaker_names = calls::empty_speaker_names();
                    session.text = calls::text(&session.rows, &session.speaker_names);
                    session.notes = session
                        .notes
                        .as_ref()
                        .map(|_| crate::notes::Notes::build(&session.rows));
                }
                _ => {
                    let mut dictation = file_pipeline::Dictation::new(true);
                    audio_store::read(&audio, 0, |start, channels| {
                        anyhow::ensure!(!cancel.is_cancelled(), "Transcription cancelled");
                        // A spoken note or dictation is one track; mix any others in.
                        let mut mono = channels.first().cloned().unwrap_or_default();
                        for other in channels.iter().skip(1) {
                            for (target, sample) in mono.iter_mut().zip(other) {
                                *target += sample;
                            }
                        }
                        dictation.push(engine, &mono)?;
                        update(FileEvent::Progress(
                            (start as f32 / 16.0 / duration as f32).min(0.99),
                            "Transcribing",
                        ));
                        Ok(true)
                    })?;
                    session.text = dictation.finish(engine)?;
                    session.original.clone_from(&session.text);
                }
            }
            // Corrections referred to the replaced rows.
            session.speaker_edits.clear();
            session.transcript_versions.push(previous);
            let excess = session
                .transcript_versions
                .len()
                .saturating_sub(crate::history::MAX_TRANSCRIPT_VERSIONS);
            session.transcript_versions.drain(..excess);
            Ok(session)
        }
    }
}

impl App {
    /// Whether `id` is the call being recorded now.
    fn calls_is_live(&self, id: &str) -> bool {
        self.call.is_some() && self.history.call.as_ref().is_some_and(|s| s.id == id)
    }

    /// Store a changed saved session. A finished call stays the current call
    /// until another capture starts, and later saves of the current call
    /// rebuild it from the controller's rows, so those follow the change.
    fn calls_store(&mut self, session: Session) -> Result<(), String> {
        let worker = self
            .history
            .worker
            .as_ref()
            .ok_or("Local history is unavailable.")?;
        worker.save(session.clone());
        self.brain.remember(&session);
        if self.call.is_none()
            && self
                .history
                .call
                .as_ref()
                .is_some_and(|current| current.id == session.id)
        {
            self.call_rows.clone_from(&session.rows);
            self.call_committed.clone_from(&session.rows);
            self.speaker_names.clone_from(&session.speaker_names);
            self.call_notes.clone_from(&session.notes);
            self.history.call = Some(session.clone());
            self.history.call_dirty = None;
        }
        if self
            .history
            .selected
            .as_ref()
            .is_some_and(|selected| selected.id == session.id)
        {
            self.history.selected = Some(session);
        }
        Ok(())
    }

    /// Prepare rows arriving from capture: chosen participant names and the
    /// corrections made during this call.
    pub(super) fn calls_prepare_rows(&mut self, rows: &mut [calls::Row]) {
        if self.is_note_capture() {
            return;
        }
        for speaker in rows
            .iter()
            .filter_map(|row| row.discord.as_ref())
            .flat_map(|attribution| attribution.speakers.iter())
        {
            self.call_tools.people.observe(speaker);
        }
        self.call_tools.people.apply(rows);
        speaker_edits::apply_rules(rows, &self.call_tools.live_rules, &self.speaker_names);
    }

    pub(super) fn calls_interruption(&mut self, gap: calls::Interruption) {
        if self.call.is_none() {
            return;
        }
        let Some(session) = &mut self.history.call else {
            return;
        };
        calls::interruptions::record(&mut session.interruptions, gap);
        if let Some(selected) = &mut self.history.selected
            && selected.id == session.id
        {
            selected.interruptions.clone_from(&session.interruptions);
        }
        self.history_call_changed();
    }

    /// Keep the recording's audio when the user chose to. Returns the
    /// recorder to hand to capture.
    pub(super) fn calls_start_recorder(
        &mut self,
        session: &mut Session,
        channels: Vec<String>,
    ) -> Option<Arc<audio_store::Recorder>> {
        let path = self.call_tools.audio_path(&session.id)?;
        match audio_store::Recorder::start(path, channels.clone(), self.settings.encrypt_at_rest) {
            Ok(recorder) => {
                session.audio = Some(audio_store::Info {
                    format: audio_store::FORMAT.into(),
                    duration_ms: 0,
                    bytes: 0,
                    channels,
                    incomplete: false,
                });
                let recorder = Arc::new(recorder);
                self.call_tools.recorder = Some(recorder.clone());
                Some(recorder)
            }
            Err(error) => {
                log::warn!("Could not keep call audio: {error:#}");
                None
            }
        }
    }

    /// Finish retained audio and open interruptions when capture ends.
    pub(super) fn calls_capture_finished(&mut self) {
        self.call_tools.live_rules.clear();
        let end_ms = self
            .call
            .as_ref()
            .map_or(0, |control| (control.end_seconds() * 1000.0) as u64);
        let info = self
            .call_tools
            .recorder
            .take()
            .map(|recorder| recorder.finish());
        if let Some(session) = &mut self.history.call {
            calls::interruptions::close(&mut session.interruptions, end_ms);
            match info {
                Some(Ok(info)) if info.duration_ms > 0 => session.audio = Some(info),
                Some(Ok(_)) => session.audio = None,
                Some(Err(error)) => {
                    log::warn!("Could not finish call audio: {error:#}");
                    session.audio = None;
                }
                None => {}
            }
            if let Some(selected) = &mut self.history.selected
                && selected.id == session.id
            {
                selected.audio.clone_from(&session.audio);
                selected.interruptions.clone_from(&session.interruptions);
            }
        }
    }

    /// Save known people now and then, and pick up offered files.
    pub(super) fn calls_tick(&mut self) {
        if let Some(path) = OFFERED.lock().unwrap_or_else(|e| e.into_inner()).take()
            && !self.call_tools.import.working
        {
            let import = &mut self.call_tools.import;
            import.file_name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
            import.pending = Some(path);
            import.failed = false;
            import.message.clear();
            import.progress = None;
            import.session_id = None;
        }
        if self.call_tools.people.dirty()
            && self
                .call_tools
                .people_saved
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(5))
        {
            self.call_tools.people_saved = Some(Instant::now());
            if let Err(error) = self.call_tools.people.save() {
                log::warn!("{error:#}");
            }
        }
        if let Some(player) = &self.call_tools.player
            && !player.playing()
        {
            self.call_tools.playback_error = player.error();
            self.call_tools.player = None;
        }
    }

    pub(crate) fn calls_dragging(&self) -> bool {
        DRAGGING.load(Ordering::Relaxed)
    }

    /// Apply a speaker correction to a live or saved conversation.
    pub(crate) fn calls_speaker_edit(&mut self, id: &str, op: SpeakerOp) -> Result<(), String> {
        if self.calls_is_live(id) {
            if self.is_note_capture() {
                return Err("Spoken notes have one speaker.".into());
            }
            return self.calls_live_edit(op);
        }
        let mut session = self
            .history
            .selected
            .as_ref()
            .filter(|session| session.id == id)
            .cloned()
            .ok_or("Open the conversation before correcting speakers.")?;
        if session.kind != Kind::Call || session.rows.is_empty() {
            return Err("Only conversation transcripts have speakers to correct.".into());
        }
        if let Some(reason) = self.desktop_edit_blocked(&session) {
            return Err(reason.into());
        }
        if self.call_tools.import.rebuilding.as_deref() == Some(id) {
            return Err("Wait for re-transcription to finish.".into());
        }
        let mut rows = std::mem::take(&mut session.rows);
        let mut names = std::mem::take(&mut session.speaker_names);
        let result = self.calls_apply(&mut rows, &mut names, &mut session.speaker_edits, op);
        session.rows = rows;
        session.speaker_names = names;
        result?;
        speaker_edits::refresh_derived(&mut session);
        self.calls_store(session)
    }

    fn calls_live_edit(&mut self, op: SpeakerOp) -> Result<(), String> {
        let committed = self.call_committed.len();
        if let SpeakerOp::Reassign { rows, .. } = &op
            && rows.iter().any(|(index, _)| *index >= committed)
        {
            return Err("Wait for these words to be finalized before reassigning them.".into());
        }
        let mut session_edits = self
            .history
            .call
            .as_mut()
            .map(|session| std::mem::take(&mut session.speaker_edits))
            .unwrap_or_default();
        let mut rows = std::mem::take(&mut self.call_committed);
        let mut names = std::mem::take(&mut self.speaker_names);
        let result = self.calls_apply(&mut rows, &mut names, &mut session_edits, op);
        self.call_committed = rows;
        self.speaker_names = names;
        if let Some(session) = &mut self.history.call {
            session.speaker_edits = session_edits;
        }
        result?;
        // Mirror the committed identities into the displayed rows; later,
        // provisional rows follow the live rules.
        for (shown, committed) in self.call_rows.iter_mut().zip(&self.call_committed) {
            if shown.start_ms == committed.start_ms {
                shown.microphone = committed.microphone;
                shown.speakers.clone_from(&committed.speakers);
                shown.discord.clone_from(&committed.discord);
            }
        }
        let tail = self.call_committed.len().min(self.call_rows.len());
        speaker_edits::apply_rules(
            &mut self.call_rows[tail..],
            &self.call_tools.live_rules,
            &self.speaker_names,
        );
        self.refresh_call_notes();
        self.history_save_call();
        Ok(())
    }

    /// Apply one operation, recording or popping its undo entry.
    fn calls_apply(
        &mut self,
        rows: &mut [calls::Row],
        names: &mut Vec<String>,
        edits: &mut Vec<Edit>,
        op: SpeakerOp,
    ) -> Result<(), String> {
        let text = |error: anyhow::Error| error.to_string();
        let mut edit = match op {
            SpeakerOp::Undo => {
                let edit = edits.pop().ok_or("There is nothing to undo.")?;
                speaker_edits::undo(rows, names, &edit);
                if let Some(rule) = &edit.rule
                    && let Some(position) =
                        self.call_tools.live_rules.iter().rposition(|r| r == rule)
                {
                    self.call_tools.live_rules.remove(position);
                }
                if let Some((user_id, previous)) = edit.registry.clone() {
                    self.call_tools.people.restore(&user_id, previous);
                }
                return Ok(());
            }
            SpeakerOp::Rename { speaker, name } => {
                let roster = speaker_edits::roster(rows, names);
                let current = roster
                    .iter()
                    .find(|s| s.key == speaker)
                    .map(|s| s.voice.clone());
                let mut edit = speaker_edits::rename(rows, names, &speaker, &name).map_err(text)?;
                let name = speaker_edits::clean_name(&name).map_err(text)?;
                match current {
                    Some(speaker_edits::Voice::Named(named)) => {
                        if let Some(previous) =
                            self.call_tools.people.name_participant(&named, &name)
                        {
                            let user_id = if named.id.is_empty() {
                                named
                                    .avatar
                                    .as_ref()
                                    .map(|a| a.user_id.clone())
                                    .unwrap_or_default()
                            } else {
                                named.id.clone()
                            };
                            edit.registry = Some((user_id, previous));
                        } else {
                            self.call_tools.people.remember_name(&name);
                        }
                    }
                    _ => self.call_tools.people.remember_name(&name),
                }
                edit
            }
            SpeakerOp::Merge { from, into } => {
                speaker_edits::merge(rows, names, &from, &into).map_err(text)?
            }
            SpeakerOp::Reassign {
                rows: selection,
                target,
            } => {
                if let Target::New(name) = &target {
                    let name = speaker_edits::clean_name(name).map_err(text)?;
                    self.call_tools.people.remember_name(&name);
                }
                speaker_edits::reassign(rows, names, &selection, &target).map_err(text)?
            }
        };
        if self.call.is_some()
            && let Some(rule) = edit.rule.clone()
        {
            self.call_tools.live_rules.push(rule);
        } else {
            edit.rule = None;
        }
        speaker_edits::record(edits, edit);
        Ok(())
    }

    /// Start importing the offered file.
    pub(crate) fn calls_import_start(
        &mut self,
        kind: ImportKind,
        keep_audio: bool,
    ) -> Result<(), String> {
        if self.call_tools.import.working {
            return Err("An import is already running.".into());
        }
        if self.recording.is_some() || self.call.is_some() || self.busy {
            return Err("Finish the current recording before importing audio.".into());
        }
        if !self.ready {
            return Err("Load a speech model before importing audio.".into());
        }
        let path = self
            .call_tools
            .import
            .pending
            .clone()
            .ok_or("Choose an audio file to import.")?;
        if !path.is_file() {
            return Err("That file is no longer available.".into());
        }
        let mut session = Session::new(match kind {
            ImportKind::Call => Kind::Call,
            ImportKind::Dictation => Kind::Dictation,
        });
        let keep = if keep_audio {
            Some((
                self.call_tools
                    .audio_path(&session.id)
                    .ok_or("Local history is unavailable.")?,
                self.settings.encrypt_at_rest,
            ))
        } else {
            None
        };
        session.title = file_title(&path);
        let cancel = CancelToken::new();
        self.commands
            .send(Command::File(
                FileJob::Import {
                    path,
                    kind,
                    session: Box::new(session),
                    keep,
                    cpu: self.settings.cpu,
                },
                cancel.clone(),
            ))
            .map_err(|_| "The speech worker stopped. Restart the app.")?;
        let import = &mut self.call_tools.import;
        import.pending = None;
        import.working = true;
        import.failed = false;
        import.progress = Some(0.0);
        import.stage = "Preparing";
        import.message = "Importing…".into();
        import.cancel = Some(cancel);
        import.session_id = None;
        self.busy = true;
        Ok(())
    }

    pub(crate) fn calls_import_dismiss(&mut self) {
        let import = &mut self.call_tools.import;
        if !import.working {
            *import = ImportState::default();
        }
    }

    pub(crate) fn calls_import_cancel(&mut self) {
        if let Some(cancel) = &self.call_tools.import.cancel {
            cancel.cancel();
            self.call_tools.import.message = "Cancelling…".into();
        }
    }

    /// Rebuild a transcript from its retained audio with the loaded model.
    pub(crate) fn calls_rebuild(&mut self, id: &str) -> Result<(), String> {
        if self.call_tools.import.working {
            return Err("Wait for the current import or re-transcription to finish.".into());
        }
        if self.recording.is_some() || self.call.is_some() || self.busy || !self.ready {
            return Err("Finish recording and load a speech model first.".into());
        }
        let session = self
            .history
            .selected
            .as_ref()
            .filter(|session| session.id == id)
            .cloned()
            .ok_or("Open the recording before re-transcribing it.")?;
        if let Some(reason) = self.desktop_edit_blocked(&session) {
            return Err(reason.into());
        }
        if session.audio.is_none() {
            return Err("This recording has no retained audio.".into());
        }
        let audio = self
            .call_tools
            .audio_path(id)
            .filter(|path| path.is_file())
            .ok_or("The retained audio for this recording is missing.")?;
        self.history_save_personal_notes();
        let cancel = CancelToken::new();
        let model = std::path::Path::new(&self.settings.model_path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.commands
            .send(Command::File(
                FileJob::Rebuild {
                    session: Box::new(session),
                    audio,
                    model,
                    cpu: self.settings.cpu,
                },
                cancel.clone(),
            ))
            .map_err(|_| "The speech worker stopped. Restart the app.")?;
        let import = &mut self.call_tools.import;
        import.working = true;
        import.failed = false;
        import.rebuilding = Some(id.to_owned());
        import.progress = Some(0.0);
        import.stage = "Preparing";
        import.message = "Re-transcribing…".into();
        import.cancel = Some(cancel);
        self.busy = true;
        Ok(())
    }

    /// Swap the current transcript with a kept earlier version.
    pub(crate) fn calls_restore_version(&mut self, id: &str, index: usize) -> Result<(), String> {
        let mut session = self
            .history
            .selected
            .as_ref()
            .filter(|session| session.id == id)
            .cloned()
            .ok_or("Open the recording first.")?;
        if let Some(reason) = self.desktop_edit_blocked(&session) {
            return Err(reason.into());
        }
        if self.call_tools.import.rebuilding.as_deref() == Some(id) {
            return Err("Wait for re-transcription to finish.".into());
        }
        if index >= session.transcript_versions.len() {
            return Err("That version is no longer available.".into());
        }
        let restored = session.transcript_versions.remove(index);
        let current = TranscriptVersion::of(&session, "");
        session.text = restored.text;
        session.original = restored.original;
        session.rows = restored.rows;
        session.speaker_names = restored.speaker_names;
        if session.speaker_names.is_empty() {
            session.speaker_names = calls::empty_speaker_names();
        }
        session.speaker_edits.clear();
        session.notes = session
            .notes
            .as_ref()
            .map(|_| crate::notes::Notes::build(&session.rows));
        session.transcript_versions.push(current);
        if session.kind == Kind::Call {
            speaker_edits::refresh_derived(&mut session);
        }
        self.calls_store(session)
    }

    /// Delete retained audio but keep the transcript.
    pub(crate) fn calls_delete_audio(&mut self, id: &str) -> Result<(), String> {
        if self.calls_is_live(id) || self.call_tools.import.rebuilding.as_deref() == Some(id) {
            return Err("Wait for recording and transcription to finish.".into());
        }
        let mut session = self
            .history
            .selected
            .as_ref()
            .filter(|session| session.id == id)
            .cloned()
            .ok_or("Open the recording first.")?;
        if let Some(player) = &mut self.call_tools.player
            && player.session_id == id
        {
            player.stop();
            self.call_tools.player = None;
        }
        if let Some(path) = self.call_tools.audio_path(id) {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("Could not delete the audio: {error}")),
            }
        }
        session.audio = None;
        self.calls_store(session)
    }

    pub(crate) fn calls_play(&mut self, id: &str, from_ms: u64) -> Result<(), String> {
        if let Some(mut player) = self.call_tools.player.take() {
            player.stop();
        }
        self.call_tools.playback_error = None;
        let session = self
            .history
            .selected
            .as_ref()
            .filter(|session| session.id == id)
            .ok_or("Open the recording first.")?;
        let info = session
            .audio
            .as_ref()
            .ok_or("This recording has no retained audio.")?;
        if self.calls_is_live(id) {
            return Err("Audio can be played after recording finishes.".into());
        }
        let path = self
            .call_tools
            .audio_path(id)
            .filter(|path| path.is_file())
            .ok_or("The retained audio for this recording is missing.")?;
        let from_ms = from_ms.min(info.duration_ms.saturating_sub(1));
        let player =
            playback::Player::start(id.to_owned(), path, from_ms, self.settings.output.clone())
                .map_err(|error| format!("{error:#}"))?;
        self.call_tools.player = Some(player);
        Ok(())
    }

    pub(crate) fn calls_stop_playback(&mut self) {
        if let Some(mut player) = self.call_tools.player.take() {
            player.stop();
        }
    }

    /// A finished file job: save the new or rebuilt session.
    pub(super) fn calls_file_event(&mut self, event: FileEvent) {
        match event {
            FileEvent::Progress(progress, stage) => {
                self.call_tools.import.progress = Some(progress);
                self.call_tools.import.stage = stage;
            }
            FileEvent::Finished(result) => {
                self.busy = false;
                let rebuilding = self.call_tools.import.rebuilding.take();
                let import = &mut self.call_tools.import;
                import.working = false;
                import.cancel = None;
                import.progress = None;
                match result {
                    Ok(session) => {
                        import.failed = false;
                        import.message = if rebuilding.is_some() {
                            "Transcript rebuilt. The previous version is kept.".into()
                        } else {
                            "Import finished".into()
                        };
                        import.session_id = Some(session.id.clone());
                        if let Err(error) = self.calls_store(*session) {
                            let import = &mut self.call_tools.import;
                            import.failed = true;
                            import.message = error;
                        } else if let Some(worker) = &self.history.worker {
                            worker.list();
                        }
                    }
                    Err(error) => {
                        import.failed = true;
                        import.message = error;
                    }
                }
            }
        }
    }

    pub(crate) fn calls_people_save(&mut self, id: Option<&str>, name: &str) -> Result<(), String> {
        self.call_tools
            .people
            .save_person(id, name)
            .map_err(|error| error.to_string())?;
        self.call_tools
            .people
            .save()
            .map_err(|error| error.to_string())
    }

    pub(crate) fn calls_people_delete(&mut self, id: Option<&str>) -> Result<(), String> {
        match id {
            Some(id) => {
                if !self.call_tools.people.remove(id) {
                    return Err("This person is no longer in the list.".into());
                }
            }
            None => self.call_tools.people.clear(),
        }
        self.call_tools
            .people
            .save()
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::app;

    fn row(start_ms: u64, speakers: &[i32], text: &str) -> calls::Row {
        calls::Row {
            cues: Vec::new(),
            start_ms,
            end_ms: start_ms + 1000,
            microphone: false,
            speakers: speakers.to_vec(),
            discord: None,
            text: text.into(),
        }
    }

    #[test]
    fn saved_corrections_update_text_and_undo_restores_labels() {
        let (mut app, _) = app();
        let mut session = Session::new(Kind::Call);
        session.rows = vec![row(0, &[1], "Hello."), row(2000, &[2], "Hi.")];
        session.text = calls::text(&session.rows, &session.speaker_names);
        let id = session.id.clone();
        app.history.selected = Some(session);
        // Without a history worker the save fails, but the edit is checked first.
        let rename = SpeakerOp::Rename {
            speaker: "s2".into(),
            name: "Morgan".into(),
        };
        assert_eq!(
            app.calls_speaker_edit(&id, rename),
            Err("Local history is unavailable.".into())
        );
        app.history.worker = Some(crate::history::Worker::test_directory(
            std::env::temp_dir().join(format!(
                "articulate-call-tools-{}",
                crate::history::now_ms()
            )),
        ));
        app.calls_speaker_edit(
            &id,
            SpeakerOp::Rename {
                speaker: "s2".into(),
                name: "Morgan".into(),
            },
        )
        .unwrap();
        let saved = app.history.selected.as_ref().unwrap();
        assert!(saved.text.contains("Morgan: Hi."));
        assert_eq!(saved.speaker_edits.len(), 1);
        assert_eq!(app.call_tools.people.list()[0].name, "Morgan");
        app.calls_speaker_edit(
            &id,
            SpeakerOp::Reassign {
                rows: vec![(0, 0)],
                target: Target::Existing("s2".into()),
            },
        )
        .unwrap();
        assert!(
            app.history
                .selected
                .as_ref()
                .unwrap()
                .text
                .starts_with("[00:00] Morgan: Hello.")
        );
        app.calls_speaker_edit(&id, SpeakerOp::Undo).unwrap();
        app.calls_speaker_edit(&id, SpeakerOp::Undo).unwrap();
        let saved = app.history.selected.as_ref().unwrap();
        assert!(saved.text.contains("Speaker 2: Hi.") && saved.speaker_edits.is_empty());
        assert!(app.calls_speaker_edit(&id, SpeakerOp::Undo).is_err());
    }

    #[test]
    fn live_merges_cover_later_turns_and_undo_removes_the_rule() {
        let (mut app, _) = app();
        let mut session = Session::new(Kind::Call);
        let id = session.id.clone();
        session.rows.clear();
        app.history.call = Some(session);
        app.call = Some(call_capture::Control::new());
        app.call_committed = vec![row(0, &[1], "One."), row(2000, &[2], "Two.")];
        app.call_rows = app.call_committed.clone();
        app.call_rows.push(row(4000, &[2], "Draft."));
        app.calls_speaker_edit(
            &id,
            SpeakerOp::Merge {
                from: "s2".into(),
                into: "s1".into(),
            },
        )
        .unwrap();
        assert_eq!(app.call_committed[1].speakers, [1]);
        assert_eq!(app.call_rows[2].speakers, [1], "provisional rows follow");
        let mut later = vec![row(9000, &[2], "Later.")];
        app.calls_prepare_rows(&mut later);
        assert_eq!(later[0].speakers, [1]);
        // Provisional rows cannot be reassigned.
        assert!(
            app.calls_speaker_edit(
                &id,
                SpeakerOp::Reassign {
                    rows: vec![(2, 4000)],
                    target: Target::Existing("s1".into()),
                },
            )
            .is_err()
        );
        app.calls_speaker_edit(&id, SpeakerOp::Undo).unwrap();
        assert_eq!(app.call_committed[1].speakers, [2]);
        let mut later = vec![row(9000, &[2], "Later.")];
        app.calls_prepare_rows(&mut later);
        assert_eq!(later[0].speakers, [2]);
    }

    #[test]
    fn corrections_to_a_finished_call_survive_later_saves_of_that_call() {
        let (mut app, _) = app();
        app.history.worker = Some(crate::history::Worker::test_directory(
            std::env::temp_dir().join(format!("articulate-call-sync-{}", crate::history::now_ms())),
        ));
        let mut session = Session::new(Kind::Call);
        session.rows = vec![row(0, &[1], "Hello.")];
        let id = session.id.clone();
        app.call_rows.clone_from(&session.rows);
        app.call_committed.clone_from(&session.rows);
        app.history.call = Some(session.clone());
        app.history.selected = Some(session);
        app.calls_speaker_edit(
            &id,
            SpeakerOp::Rename {
                speaker: "s1".into(),
                name: "Morgan".into(),
            },
        )
        .unwrap();
        // A notes save on the finished call rebuilds it from controller rows.
        app.history_save_call();
        let saved = app.history.selected.as_ref().unwrap();
        assert_eq!(saved.speaker_names[0], "Morgan");
        assert!(saved.text.contains("Morgan: Hello."));
        assert_eq!(saved.speaker_edits.len(), 1);
    }

    #[test]
    fn interruptions_belong_to_the_recording_and_close_when_it_ends() {
        let (mut app, _) = app();
        app.history.call = Some(Session::new(Kind::Call));
        let gap = |ongoing| {
            let mut gap = calls::Interruption::new(
                1000,
                2000,
                calls::interruptions::Source::Microphone,
                calls::interruptions::Cause::DeviceLost,
            );
            gap.ongoing = ongoing;
            gap
        };
        app.calls_interruption(gap(true));
        assert!(
            app.history.call.as_ref().unwrap().interruptions.is_empty(),
            "no capture"
        );
        app.call = Some(call_capture::Control::new());
        app.calls_interruption(gap(true));
        app.calls_interruption(gap(true));
        assert_eq!(app.history.call.as_ref().unwrap().interruptions.len(), 1);
        app.calls_capture_finished();
        let stored = &app.history.call.as_ref().unwrap().interruptions[0];
        assert!(!stored.ongoing);
        assert!(stored.end_ms >= 2000);
    }

    #[test]
    fn offered_files_wait_for_a_choice_and_unsupported_ones_are_ignored() {
        let (mut app, _) = app();
        assert!(!offer_files(&[PathBuf::from("notes.txt")]));
        assert!(offer_files(&[
            PathBuf::from("notes.txt"),
            PathBuf::from("meeting.m4a")
        ]));
        app.calls_tick();
        assert_eq!(
            app.call_tools.import.file_name.as_deref(),
            Some("meeting.m4a")
        );
        assert!(
            app.calls_import_start(ImportKind::Call, false)
                .unwrap_err()
                .contains("no longer available")
        );
        app.calls_import_dismiss();
        assert!(app.call_tools.import.pending.is_none());
    }
}
