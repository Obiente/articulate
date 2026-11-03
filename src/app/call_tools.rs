//! Calls and speakers in the controller: speaker corrections, known people,
//! interruption markers, retained audio and playback, audio import and
//! re-transcription.
use super::*;
use crate::{
    audio_store, file_pipeline,
    history::{Kind, MigrationReport, Session, TranscriptVersion},
    people, playback,
    speaker_edits::{self, Edit, LiveRule, RowBefore, Target},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

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
        /// Keeps the audio when the user chose to. It is published only when
        /// the import succeeds; the controller holds it too, so it follows
        /// encryption changes made while the import runs.
        recorder: Option<Arc<audio_store::Recorder>>,
        cpu: bool,
        /// The saved transcription language; see `call_language`.
        language: Option<String>,
    },
    Rebuild {
        session: Box<Session>,
        audio: PathBuf,
        cpu: bool,
        language: Option<String>,
    },
}

/// How a file job treats the saved transcription language. Conversations,
/// like live calls and spoken notes, detect the language of each utterance
/// and use the saved language only to break ties (`Some`); a dictation keeps
/// the language it is set to (`None`).
fn call_language(job: &FileJob) -> Option<Option<String>> {
    match job {
        FileJob::Import { kind, language, .. } => {
            (*kind == ImportKind::Call).then(|| language.clone())
        }
        FileJob::Rebuild {
            session, language, ..
        } => (session.kind != Kind::Dictation).then(|| language.clone()),
    }
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
    /// What that re-transcription read: the transcript revision and the model.
    pub rebuild_from: Option<(String, String)>,
    pub file_name: Option<String>,
    pub progress: Option<f32>,
    pub stage: &'static str,
    pub message: String,
    pub failed: bool,
    /// The saved session of the most recent import, to open it.
    pub session_id: Option<String>,
    pub cancel: Option<CancelToken>,
    /// The running import's retained audio.
    pub recorder: Option<Arc<audio_store::Recorder>>,
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
    /// An encryption change's result, held until recordings in progress
    /// have converted the audio they already wrote.
    protection_report: Option<Result<MigrationReport, String>>,
    /// Recordings that finished without converting all their audio.
    protection_failures: usize,
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
            protection_report: None,
            protection_failures: 0,
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

    /// Recorders writing audio now.
    fn recorders(&self) -> impl Iterator<Item = &Arc<audio_store::Recorder>> {
        self.recorder.iter().chain(self.import.recorder.iter())
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

fn digest(hash: impl FnOnce(&mut std::hash::DefaultHasher)) -> String {
    use std::hash::Hasher;
    let mut hasher = std::hash::DefaultHasher::new();
    hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Identifies the current transcript of a session: its text, original,
/// speaker names and rows. Changes whenever any of them does.
pub(crate) fn transcript_revision(session: &Session) -> String {
    use std::hash::Hash;
    digest(|hasher| {
        session.text.hash(hasher);
        session.original.hash(hasher);
        session.speaker_names.hash(hasher);
        session.rows.len().hash(hasher);
        for row in &session.rows {
            (
                row.start_ms,
                row.end_ms,
                row.microphone,
                &row.speakers,
                &row.text,
            )
                .hash(hasher);
            for speaker in row.discord.iter().flat_map(|named| &named.speakers) {
                speaker.name.hash(hasher);
            }
        }
    })
}

/// A stable identity for an earlier transcript, unlike its position, which
/// changes when a version is restored or another is added.
pub(crate) fn version_id(version: &TranscriptVersion) -> String {
    use std::hash::Hash;
    digest(|hasher| {
        (
            version.replaced_ms,
            &version.model,
            &version.text,
            &version.original,
            &version.speaker_names,
            version.rows.len(),
        )
            .hash(hasher);
    })
}

/// Keep `recorder`'s audio only if `work` succeeds. Cancellation, a failed
/// decode and a failed final transcription all remove the recording, so no
/// audio outlives an import that saves nothing.
fn keeping<T>(
    recorder: Option<&audio_store::Recorder>,
    work: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<(T, Option<audio_store::Info>)> {
    let value = match work() {
        Ok(value) => value,
        Err(error) => {
            if let Some(recorder) = recorder {
                recorder.discard();
            }
            return Err(error);
        }
    };
    let info = recorder.map(audio_store::Recorder::finish).transpose()?;
    Ok((value, info))
}

/// Run a file job on the speech worker. Progress is reported through `update`.
pub(super) fn run_file_job(
    engine: &mut Engine,
    job: FileJob,
    cancel: &CancelToken,
    update: &mut dyn FnMut(FileEvent),
) -> anyhow::Result<Box<Session>> {
    match call_language(&job) {
        Some(language) => engine.with_call_preference(language, |engine| {
            run_file_job_with(engine, job, cancel, update)
        }),
        None => run_file_job_with(engine, job, cancel, update),
    }
}

fn run_file_job_with(
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
            recorder,
            cpu,
            language: _,
        } => {
            // Probing and decoding stop with the job, not only between chunks.
            let token = cancel.clone();
            let stop: crate::media::Cancel = Arc::new(move || token.is_cancelled());
            let recorder = recorder.as_deref();
            let ((), audio) = keeping(recorder, || {
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
                crate::media::stream_file(
                    &path,
                    crate::media::Layout::Mono,
                    &stop,
                    |progress| fraction.set(progress),
                    |chunk| {
                        anyhow::ensure!(!cancel.is_cancelled(), "Import cancelled");
                        let pcm = chunk.into_iter().next().unwrap_or_default();
                        decoded += pcm.len();
                        if let Some(recorder) = recorder {
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
                )?;
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
                anyhow::ensure!(!cancel.is_cancelled(), "Import cancelled");
                Ok(())
            })?;
            session.audio = audio;
            session.title = file_title(&path);
            session.title_is_manual = true;
            Ok(session)
        }
        FileJob::Rebuild {
            mut session,
            audio,
            cpu,
            language: _,
        } => {
            // Only the transcript is rebuilt here; `merge_rebuilt` installs
            // it into the latest copy of the session.
            let info = session
                .audio
                .clone()
                .ok_or_else(|| anyhow::anyhow!("This recording has no retained audio"))?;
            let duration = info.duration_ms.max(1);
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
            Ok(session)
        }
    }
}

/// Install the transcript of `rebuilt` into `latest`, the newest copy of the
/// same session. Only transcript fields change; the title, notes, personal
/// notes and filing in `latest` stay. If the transcript changed after the
/// re-transcription read it (`started_from` is no longer its revision), it
/// is kept, and the rebuilt transcript becomes an earlier version instead.
/// Returns whether the rebuilt transcript is now the current one.
pub(super) fn merge_rebuilt(
    latest: &mut Session,
    rebuilt: &Session,
    started_from: &str,
    model: &str,
) -> bool {
    let current = transcript_revision(latest) == started_from;
    let replaced = if current {
        TranscriptVersion::of(latest, model)
    } else {
        TranscriptVersion::of(rebuilt, model)
    };
    latest.transcript_versions.push(replaced);
    let excess = latest
        .transcript_versions
        .len()
        .saturating_sub(crate::history::MAX_TRANSCRIPT_VERSIONS);
    latest.transcript_versions.drain(..excess);
    if current {
        latest.text.clone_from(&rebuilt.text);
        latest.original.clone_from(&rebuilt.original);
        latest.rows.clone_from(&rebuilt.rows);
        latest.speaker_names.clone_from(&rebuilt.speaker_names);
        // Corrections referred to the replaced rows.
        latest.speaker_edits.clear();
        latest.notes = latest
            .notes
            .as_ref()
            .map(|_| crate::notes::Notes::build(&latest.rows));
        if latest.kind == Kind::Call {
            speaker_edits::refresh_derived(latest);
        }
    }
    current
}

/// How a committed live row came to have its speaker, by start time.
enum Mark<'a> {
    /// A correction changed it; it never absorbs later speech.
    Edited,
    /// A live rule re-attributed it on arrival, with this provenance.
    Arrived(&'a str),
}

fn live_marks(edits: &[Edit]) -> HashMap<u64, Mark<'_>> {
    let mut marks = HashMap::new();
    for before in edits.iter().flat_map(|edit| &edit.rows) {
        match &before.arrival {
            Some(tag) => {
                marks.entry(before.start_ms).or_insert(Mark::Arrived(tag));
            }
            None => {
                marks.insert(before.start_ms, Mark::Edited);
            }
        }
    }
    marks
}

/// Append live rows so a turn only continues a turn of the same provenance:
/// a row a correction changed keeps its boundary, so undo never hands later
/// speech to the old speaker, and rows a live rule re-attributed only
/// continue rows re-attributed the same way. `arriving` holds the provenance
/// of incoming re-attributed rows by start time.
fn append_live(
    transcript: &mut Vec<calls::Row>,
    rows: Vec<calls::Row>,
    edits: &[Edit],
    arriving: &HashMap<u64, String>,
) {
    let marks = live_marks(edits);
    calls::append_rows_where(transcript, rows, |last, row| {
        let incoming = arriving.get(&row.start_ms).map(String::as_str);
        match marks.get(&last.start_ms) {
            Some(Mark::Edited) => false,
            Some(Mark::Arrived(tag)) => incoming == Some(*tag),
            None => incoming == arriving.get(&last.start_ms).map(String::as_str),
        }
    });
}

/// A row a live rule re-attributed as it arrived.
struct Arrival {
    start_ms: u64,
    /// The rules that changed it and the speaker it arrived with.
    tag: String,
    /// Per rule, the row's identity just before that rule.
    inverses: Vec<(u64, RowBefore)>,
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

    /// Rows arriving from capture get the chosen participant names and the
    /// corrections made during this call. Returns the rows a live rule
    /// re-attributed.
    fn calls_prepare(&mut self, rows: &mut [calls::Row]) -> Vec<Arrival> {
        if self.is_note_capture() {
            return Vec::new();
        }
        for speaker in rows
            .iter()
            .filter_map(|row| row.discord.as_ref())
            .flat_map(|attribution| attribution.speakers.iter())
        {
            self.call_tools.people.observe(speaker);
        }
        self.call_tools.people.apply(rows);
        let mut arrivals = Vec::new();
        for row in rows {
            let arrived_as = speaker_edits::row_keys(row).join("+");
            let inverses = speaker_edits::apply_rules_traced(
                row,
                &self.call_tools.live_rules,
                &self.speaker_names,
            );
            if inverses.is_empty() {
                continue;
            }
            let rules: Vec<String> = inverses.iter().map(|(id, _)| id.to_string()).collect();
            arrivals.push(Arrival {
                start_ms: row.start_ms,
                tag: format!("{}|{arrived_as}", rules.join(",")),
                inverses,
            });
        }
        arrivals
    }

    /// Commit rows from capture to the live transcript. Rows a live rule
    /// re-attributed are added to the inverse of that rule's correction, so
    /// undoing it restores them too.
    pub(super) fn calls_commit_rows(&mut self, mut rows: Vec<calls::Row>) {
        if self.is_note_capture() {
            // Keep source identities and timestamps immutable so live note
            // updates can reuse earlier model sections.
            self.call_committed.extend(rows);
            return;
        }
        let arrivals = self.calls_prepare(&mut rows);
        let tags: HashMap<u64, String> = arrivals
            .iter()
            .map(|arrival| (arrival.start_ms, arrival.tag.clone()))
            .collect();
        let edits = self
            .history
            .call
            .as_ref()
            .map_or(&[][..], |session| session.speaker_edits.as_slice());
        append_live(&mut self.call_committed, rows, edits, &tags);
        let Some(session) = &mut self.history.call else {
            return;
        };
        for arrival in arrivals {
            // A row that continued one of the same provenance is covered by it.
            let Some(index) = self
                .call_committed
                .iter()
                .rposition(|row| row.start_ms == arrival.start_ms)
            else {
                continue;
            };
            for (rule, before) in arrival.inverses {
                if let Some(edit) = session
                    .speaker_edits
                    .iter_mut()
                    .find(|edit| edit.rule.as_ref().is_some_and(|live| live.id == rule))
                {
                    edit.rows.push(RowBefore {
                        index,
                        arrival: Some(arrival.tag.clone()),
                        ..before
                    });
                }
            }
        }
    }

    /// Show provisional rows from capture after the committed transcript.
    pub(super) fn calls_preview_rows(&mut self, mut rows: Vec<calls::Row>) {
        let arrivals = self.calls_prepare(&mut rows);
        self.call_rows.clone_from(&self.call_committed);
        if self.is_note_capture() {
            calls::append_rows(&mut self.call_rows, rows);
            return;
        }
        let tags: HashMap<u64, String> = arrivals
            .into_iter()
            .map(|arrival| (arrival.start_ms, arrival.tag))
            .collect();
        let edits = self
            .history
            .call
            .as_ref()
            .map_or(&[][..], |session| session.speaker_edits.as_slice());
        append_live(&mut self.call_rows, rows, edits, &tags);
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

    /// Note a finished recorder whose audio kept part of an old protection.
    fn calls_finished_recorder(&mut self, recorder: &audio_store::Recorder) {
        if let audio_store::Protection::Failed(error) = recorder.protection() {
            log::warn!("Could not convert kept audio: {error}");
            self.call_tools.protection_failures += 1;
        }
    }

    /// Finish retained audio and open interruptions when capture ends.
    pub(super) fn calls_capture_finished(&mut self) {
        self.call_tools.live_rules.clear();
        let end_ms = self
            .call
            .as_ref()
            .map_or(0, |control| (control.end_seconds() * 1000.0) as u64);
        let recorder = self.call_tools.recorder.take();
        let info = recorder.as_ref().map(|recorder| recorder.finish());
        if let Some(recorder) = &recorder {
            self.calls_finished_recorder(recorder);
        }
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

    /// Encryption at rest is changing: later saves of known people and
    /// recordings in progress, including what they already wrote, use the
    /// new protection. Called before the history migration starts.
    pub(super) fn calls_set_protection(&mut self, protect: bool) {
        self.call_tools.people.set_protect(protect);
        for recorder in self.call_tools.recorders() {
            recorder.set_protection(protect);
        }
    }

    /// The history migration finished (also once at startup). Known people
    /// are converted, judged from their files, and the result is reported
    /// only once recordings in progress have converted their audio too, with
    /// every failure counted.
    pub(super) fn calls_protection_changed(&mut self, result: Result<MigrationReport, String>) {
        self.call_tools
            .people
            .set_protect(self.settings.encrypt_at_rest);
        let people = self.call_tools.people.migrate();
        if people.failed > 0 {
            log::warn!("Could not convert {} known people files", people.failed);
        }
        self.call_tools.protection_report = Some(result.map(|mut report| {
            report.changed += people.changed;
            report.failed += people.failed;
            report
        }));
        self.calls_release_protection();
    }

    fn calls_release_protection(&mut self) {
        if self.call_tools.protection_report.is_none() {
            return;
        }
        let mut failed = 0;
        for recorder in self.call_tools.recorders() {
            match recorder.protection() {
                audio_store::Protection::Settled => {}
                audio_store::Protection::Pending => return,
                audio_store::Protection::Failed(error) => {
                    log::warn!("Could not convert kept audio: {error}");
                    failed += 1;
                }
            }
        }
        failed += std::mem::take(&mut self.call_tools.protection_failures);
        let result = self
            .call_tools
            .protection_report
            .take()
            .expect("checked above")
            .map(|mut report| {
                report.failed += failed;
                report
            });
        self.history.storage.encryption_working = false;
        self.history.protection = Some(result);
    }

    /// Save known people now and then, and pick up offered files.
    pub(super) fn calls_tick(&mut self) {
        // An offer made during an import waits until it finishes.
        if !self.call_tools.import.working
            && let Some(path) = OFFERED.lock().unwrap_or_else(|e| e.into_inner()).take()
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
        self.calls_release_protection();
    }

    /// A file import or re-transcription is using the speech worker.
    /// Capture must not start, and only its own cancel stops it.
    pub(super) fn calls_file_job_running(&self) -> bool {
        self.call_tools.import.working
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
        let result = self.calls_apply(&mut rows, &mut names, &mut session.speaker_edits, op, false);
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
        let result = self.calls_apply(&mut rows, &mut names, &mut session_edits, op, true);
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
        // Generated notes cite rows by position; give them the new labels.
        if let Some(session) = &mut self.history.call {
            speaker_edits::refresh_derived(session);
            self.history_call_changed();
        }
        Ok(())
    }

    /// Apply one operation, recording or popping its undo entry. Only an
    /// edit of the call being recorded (`live`) adds or removes the rules
    /// later turns of that call follow; corrections to any other
    /// conversation never reach them.
    fn calls_apply(
        &mut self,
        rows: &mut [calls::Row],
        names: &mut Vec<String>,
        edits: &mut Vec<Edit>,
        op: SpeakerOp,
        live: bool,
    ) -> Result<(), String> {
        let text = |error: anyhow::Error| error.to_string();
        let mut edit = match op {
            SpeakerOp::Undo => {
                let edit = edits.pop().ok_or("There is nothing to undo.")?;
                speaker_edits::undo(rows, names, &edit);
                if live
                    && let Some(rule) = &edit.rule
                    && let Some(position) = self
                        .call_tools
                        .live_rules
                        .iter()
                        .rposition(|live_rule| live_rule.id == rule.id)
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
        if live && let Some(rule) = edit.rule.clone() {
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
        let recorder = if keep_audio {
            let audio = self
                .call_tools
                .audio_path(&session.id)
                .ok_or("Local history is unavailable.")?;
            let recorder = audio_store::Recorder::start(
                audio,
                vec!["recording".into()],
                self.settings.encrypt_at_rest,
            )
            .map_err(|error| format!("{error:#}"))?;
            Some(Arc::new(recorder))
        } else {
            None
        };
        session.title = file_title(&path);
        let cancel = CancelToken::new();
        let job = FileJob::Import {
            path,
            kind,
            session: Box::new(session),
            recorder: recorder.clone(),
            cpu: self.settings.cpu,
            language: self.calls_saved_language(),
        };
        if self
            .commands
            .send(Command::File(job, cancel.clone()))
            .is_err()
        {
            if let Some(recorder) = &recorder {
                recorder.discard();
            }
            return Err("The speech worker stopped. Restart the app.".into());
        }
        let import = &mut self.call_tools.import;
        import.pending = None;
        import.working = true;
        import.failed = false;
        import.progress = Some(0.0);
        import.stage = "Preparing";
        import.message = "Importing…".into();
        import.cancel = Some(cancel);
        import.session_id = None;
        import.recorder = recorder;
        self.busy = true;
        Ok(())
    }

    /// The saved transcription language, as live calls receive it.
    fn calls_saved_language(&self) -> Option<String> {
        (!self.settings.transcription_language.is_empty())
            .then(|| self.settings.transcription_language.clone())
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
        let revision = transcript_revision(&session);
        self.commands
            .send(Command::File(
                FileJob::Rebuild {
                    session: Box::new(session),
                    audio,
                    cpu: self.settings.cpu,
                    language: self.calls_saved_language(),
                },
                cancel.clone(),
            ))
            .map_err(|_| "The speech worker stopped. Restart the app.")?;
        let import = &mut self.call_tools.import;
        import.working = true;
        import.failed = false;
        import.rebuilding = Some(id.to_owned());
        import.rebuild_from = Some((revision, model));
        import.progress = Some(0.0);
        import.stage = "Preparing";
        import.message = "Re-transcribing…".into();
        import.cancel = Some(cancel);
        self.busy = true;
        Ok(())
    }

    /// Swap the current transcript with a kept earlier version. `version`
    /// is the version's `version_id`, and `expected` the `transcript_revision`
    /// the renderer showed, so a repeated or stale request changes nothing.
    pub(crate) fn calls_restore_version(
        &mut self,
        id: &str,
        version: &str,
        expected: &str,
    ) -> Result<(), String> {
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
        if transcript_revision(&session) != expected {
            return Err("The transcript changed. Review the earlier transcripts again.".into());
        }
        let index = session
            .transcript_versions
            .iter()
            .position(|kept| version_id(kept) == version)
            .ok_or("That version is no longer available.")?;
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

    /// Delete retained audio but keep the transcript. `confirm_id` and
    /// `bytes` repeat the session and audio size the confirmation showed.
    pub(crate) fn calls_delete_audio(
        &mut self,
        id: &str,
        confirm_id: &str,
        bytes: u64,
    ) -> Result<(), String> {
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
        let audio = session
            .audio
            .as_ref()
            .ok_or("This recording has no kept audio.")?;
        if confirm_id != id || audio.bytes != bytes {
            return Err("The kept audio changed. Confirm deleting it again.".into());
        }
        if let Some(player) = &mut self.call_tools.player
            && player.session_id == id
        {
            player.stop();
            self.call_tools.player = None;
        }
        if let Some(directory) = &self.call_tools.history_dir {
            audio_store::remove(directory, id)
                .map_err(|error| format!("Could not delete the audio: {error}"))?;
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

    /// A finished file job: save the new session, or install a rebuilt
    /// transcript into the latest copy of its session.
    pub(super) fn calls_file_event(&mut self, event: FileEvent) {
        match event {
            FileEvent::Progress(progress, stage) => {
                self.call_tools.import.progress = Some(progress);
                self.call_tools.import.stage = stage;
            }
            FileEvent::Finished(result) => {
                self.busy = false;
                let import = &mut self.call_tools.import;
                let rebuilding = import.rebuilding.take();
                let rebuild_from = import.rebuild_from.take();
                let recorder = import.recorder.take();
                import.working = false;
                import.cancel = None;
                import.progress = None;
                let outcome = match result {
                    Err(error) => Err(error),
                    Ok(rebuilt) => match (rebuilding, rebuild_from) {
                        (Some(id), Some((revision, model))) => self
                            .calls_install_rebuilt(&id, *rebuilt, &revision, &model)
                            .map(|message| (id, message)),
                        _ => {
                            let id = rebuilt.id.clone();
                            self.calls_store(*rebuilt)
                                .map(|()| (id, "Import finished".to_owned()))
                        }
                    },
                };
                if let Some(recorder) = &recorder {
                    match &outcome {
                        // Nothing was saved, so nothing of it is kept.
                        Err(_) => recorder.discard(),
                        Ok(_) => self.calls_finished_recorder(recorder),
                    }
                }
                let import = &mut self.call_tools.import;
                match outcome {
                    Ok((id, message)) => {
                        import.failed = false;
                        import.message = message;
                        import.session_id = Some(id);
                        if let Some(worker) = &self.history.worker {
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

    /// Install a rebuilt transcript with `merge_rebuilt`: into the open or
    /// current copy when there is one (they hold the newest edits and save
    /// as usual), otherwise into the stored copy on the history worker,
    /// after any saves already queued.
    fn calls_install_rebuilt(
        &mut self,
        id: &str,
        rebuilt: Session,
        started_from: &str,
        model: &str,
    ) -> Result<String, String> {
        let open = self
            .history
            .selected
            .as_ref()
            .filter(|session| session.id == id)
            .or_else(|| {
                self.history
                    .call
                    .as_ref()
                    .filter(|session| session.id == id && self.call.is_none())
            })
            .cloned();
        if let Some(mut latest) = open {
            let current = merge_rebuilt(&mut latest, &rebuilt, started_from, model);
            self.calls_store(latest)?;
            return Ok(if current {
                "Transcript rebuilt. The previous version is kept.".into()
            } else {
                "The transcript changed while it was re-transcribed, so it was kept. The new transcript is under Earlier transcripts.".into()
            });
        }
        let worker = self
            .history
            .worker
            .as_ref()
            .ok_or("Local history is unavailable.")?;
        let (started_from, model) = (started_from.to_owned(), model.to_owned());
        worker.update(
            id.to_owned(),
            Box::new(move |latest| {
                merge_rebuilt(latest, &rebuilt, &started_from, &model);
                Ok(())
            }),
        );
        Ok("Re-transcription finished. Earlier transcripts are kept.".into())
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

    pub(crate) fn calls_people_delete(&mut self, id: &str) -> Result<(), String> {
        if !self.call_tools.people.remove(id) {
            return Err("This person is no longer in the list.".into());
        }
        self.call_tools
            .people
            .save()
            .map_err(|error| error.to_string())
    }

    /// Forget everyone. `count` repeats how many people the confirmation
    /// showed.
    pub(crate) fn calls_people_clear(&mut self, count: usize) -> Result<(), String> {
        if count != self.call_tools.people.list().len() {
            return Err("The list of people changed. Review it and confirm again.".into());
        }
        self.call_tools
            .people
            .forget_everyone()
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

    fn folder(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "articulate-call-tools-{label}-{}-{}",
            std::process::id(),
            crate::history::now_ms()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn action(value: serde_json::Value) -> crate::app::desktop::Action {
        serde_json::from_value(value).expect("a valid renderer action")
    }

    /// Wait for the history worker to finish what it was given.
    fn settle(app: &App) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while app.history.worker.as_ref().unwrap().saves_settled() != Ok(true) {
            assert!(Instant::now() < deadline, "history did not settle");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A live call with two committed turns.
    fn live_call(app: &mut App) -> String {
        let session = Session::new(Kind::Call);
        let id = session.id.clone();
        app.history.call = Some(session);
        app.call = Some(call_capture::Control::new());
        app.call_committed = vec![row(0, &[1], "One."), row(2000, &[2], "Two.")];
        app.call_rows = app.call_committed.clone();
        id
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
        app.history.worker = Some(crate::history::Worker::test_directory(folder("saved")));
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
        let id = live_call(&mut app);
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
        app.calls_preview_rows(vec![row(9000, &[2], "Later.")]);
        assert_eq!(app.call_rows.last().unwrap().speakers, [1]);
        // Provisional rows cannot be reassigned.
        assert!(
            app.calls_speaker_edit(
                &id,
                SpeakerOp::Reassign {
                    rows: vec![(2, 9000)],
                    target: Target::Existing("s1".into()),
                },
            )
            .is_err()
        );
        app.calls_speaker_edit(&id, SpeakerOp::Undo).unwrap();
        assert_eq!(app.call_committed[1].speakers, [2]);
        assert!(app.call_tools.live_rules.is_empty());
        app.calls_preview_rows(vec![row(9000, &[2], "Later.")]);
        assert_eq!(app.call_rows.last().unwrap().speakers, [2]);
    }

    #[test]
    fn live_undo_keeps_later_speech_with_its_own_speaker() {
        let (mut app, _) = app();
        let id = live_call(&mut app);
        app.calls_speaker_edit(
            &id,
            SpeakerOp::Merge {
                from: "s2".into(),
                into: "s1".into(),
            },
        )
        .unwrap();
        // Genuine speech from the merged-into speaker right after the
        // corrected turn starts a turn of its own.
        app.calls_commit_rows(vec![row(3100, &[1], "Three.")]);
        assert_eq!(app.call_committed.len(), 3);
        assert_eq!(app.call_committed[1].text, "Two.");
        // Speech from the merged speaker is re-attributed on arrival, kept
        // apart from genuine speech, and continues only its own kind.
        app.calls_commit_rows(vec![row(4200, &[2], "Four.")]);
        app.calls_commit_rows(vec![row(5300, &[2], "Five.")]);
        assert_eq!(app.call_committed.len(), 4);
        assert_eq!(app.call_committed[3].speakers, [1]);
        assert_eq!(app.call_committed[3].text, "Four. Five.");
        app.calls_speaker_edit(&id, SpeakerOp::Undo).unwrap();
        let speakers: Vec<_> = app
            .call_committed
            .iter()
            .map(|row| (row.text.as_str(), row.speakers.clone()))
            .collect();
        assert_eq!(
            speakers,
            [
                ("One.", vec![1]),
                ("Two.", vec![2]),
                ("Three.", vec![1]),
                ("Four. Five.", vec![2]),
            ]
        );
        // Without corrections, turns continue as before.
        app.calls_commit_rows(vec![row(6400, &[2], "Six.")]);
        assert_eq!(app.call_committed[3].text, "Four. Five. Six.");
    }

    #[test]
    fn saved_conversation_corrections_never_reach_another_live_call() {
        let (mut app, _) = app();
        app.history.worker = Some(crate::history::Worker::test_directory(folder("isolated")));
        live_call(&mut app);
        let mut saved = Session::new(Kind::Call);
        saved.rows = vec![row(0, &[1], "Saved one."), row(2000, &[2], "Saved two.")];
        let saved_id = saved.id.clone();
        app.history.selected = Some(saved);
        app.calls_speaker_edit(
            &saved_id,
            SpeakerOp::Merge {
                from: "s2".into(),
                into: "s1".into(),
            },
        )
        .unwrap();
        assert!(app.call_tools.live_rules.is_empty());
        app.calls_commit_rows(vec![row(9000, &[2], "Live speech.")]);
        assert_eq!(app.call_committed.last().unwrap().speakers, [2]);
        assert!(
            app.history.selected.as_ref().unwrap().speaker_edits[0]
                .rule
                .is_none()
        );
        app.calls_speaker_edit(&saved_id, SpeakerOp::Undo).unwrap();
        assert!(app.call_tools.live_rules.is_empty());
    }

    #[test]
    fn corrections_to_a_finished_call_survive_later_saves_of_that_call() {
        let (mut app, _) = app();
        app.history.worker = Some(crate::history::Worker::test_directory(folder("sync")));
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

    #[test]
    fn a_failed_or_cancelled_import_keeps_no_audio() {
        let directory = folder("orphan");
        let audio = audio_store::path(&directory, &Session::new(Kind::Call).id);
        let recorder =
            audio_store::Recorder::start(audio.clone(), vec!["recording".into()], false).unwrap();
        // The final transcription fails after all audio was written.
        let result = keeping(Some(&recorder), || -> anyhow::Result<()> {
            recorder.push(0, vec![0.25; 40 * 16_000]);
            anyhow::bail!("Transcription cancelled")
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
        // A successful import publishes its audio.
        let recorder =
            audio_store::Recorder::start(audio.clone(), vec!["recording".into()], false).unwrap();
        let ((), info) = keeping(Some(&recorder), || {
            recorder.push(0, vec![0.25; 16_000]);
            Ok(())
        })
        .unwrap();
        assert_eq!(info.unwrap().duration_ms, 1000);
        assert!(audio.exists());
        // If the session cannot be saved, the controller discards it.
        let (mut app, _) = app();
        app.call_tools.import.working = true;
        app.call_tools.import.recorder = Some(Arc::new(recorder));
        let session = Session::new(Kind::Dictation);
        app.calls_file_event(FileEvent::Finished(Ok(Box::new(session))));
        assert!(app.call_tools.import.failed);
        assert!(!audio.exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn conversations_from_files_use_call_language_handling() {
        let import = |kind| FileJob::Import {
            path: PathBuf::from("meeting.m4a"),
            kind,
            session: Box::new(Session::new(Kind::Call)),
            recorder: None,
            cpu: true,
            language: Some("en".into()),
        };
        let rebuild = |kind| FileJob::Rebuild {
            session: Box::new(Session::new(kind)),
            audio: PathBuf::from("audio"),
            cpu: true,
            language: None,
        };
        assert_eq!(
            call_language(&import(ImportKind::Call)),
            Some(Some("en".into()))
        );
        assert_eq!(call_language(&import(ImportKind::Dictation)), None);
        assert_eq!(call_language(&rebuild(Kind::Call)), Some(None));
        assert_eq!(call_language(&rebuild(Kind::Note)), Some(None));
        assert_eq!(call_language(&rebuild(Kind::Dictation)), None);
        let (mut app, commands) = app();
        app.settings.transcription_language = "fr".into();
        app.ready = true;
        let directory = folder("language");
        let file = directory.join("meeting.wav");
        std::fs::write(&file, b"RIFF").unwrap();
        app.call_tools.import.pending = Some(file);
        app.calls_import_start(ImportKind::Call, false).unwrap();
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::File(FileJob::Import { language: Some(language), .. }, _)) if language == "fr"
        ));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_rebuild_changes_only_the_transcript_of_the_latest_copy() {
        let mut started = Session::new(Kind::Call);
        started.title = "Original title".into();
        started.rows = vec![row(0, &[1], "Old words.")];
        started.text = calls::text(&started.rows, &started.speaker_names);
        let revision = transcript_revision(&started);
        let mut rebuilt = started.clone();
        rebuilt.rows = vec![row(0, &[2], "New words.")];
        rebuilt.text = calls::text(&rebuilt.rows, &rebuilt.speaker_names);
        // Edited while the re-transcription ran.
        let mut latest = started.clone();
        latest.title = "Renamed meanwhile".into();
        latest.personal_notes = "Typed meanwhile.".into();
        assert!(merge_rebuilt(&mut latest, &rebuilt, &revision, "model"));
        assert_eq!(latest.title, "Renamed meanwhile");
        assert_eq!(latest.personal_notes, "Typed meanwhile.");
        assert!(latest.text.contains("New words."));
        assert_eq!(latest.transcript_versions.len(), 1);
        assert!(latest.transcript_versions[0].text.contains("Old words."));
        // A transcript that changed meanwhile is kept; the rebuilt one waits
        // under earlier transcripts.
        let mut changed = started.clone();
        changed.text = "Edited meanwhile.".into();
        assert!(!merge_rebuilt(&mut changed, &rebuilt, &revision, "model"));
        assert_eq!(changed.text, "Edited meanwhile.");
        assert!(changed.transcript_versions[0].text.contains("New words."));
        // Closed meanwhile: the stored copy is updated on the history worker.
        let (mut app, _) = app();
        app.history.worker = Some(crate::history::Worker::test_directory(folder("rebuild")));
        let mut stored = started.clone();
        stored.title = "Saved meanwhile".into();
        app.history.worker.as_ref().unwrap().save(stored);
        settle(&app);
        app.call_tools.import.working = true;
        app.call_tools.import.rebuilding = Some(started.id.clone());
        app.call_tools.import.rebuild_from = Some((revision, "model".into()));
        app.calls_file_event(FileEvent::Finished(Ok(Box::new(rebuilt))));
        assert!(
            !app.call_tools.import.failed,
            "{}",
            app.call_tools.import.message
        );
        settle(&app);
        let history = app.history.worker.as_ref().unwrap();
        history.load(started.id.clone());
        settle(&app);
        let loaded = history
            .drain()
            .into_iter()
            .find_map(|event| match event {
                crate::history::Event::Loaded(session) => Some(session),
                _ => None,
            })
            .unwrap();
        assert_eq!(loaded.title, "Saved meanwhile");
        assert!(loaded.text.contains("New words."));
        assert_eq!(loaded.transcript_versions.len(), 1);
    }

    #[test]
    fn restoring_names_a_version_and_a_repeated_request_changes_nothing() {
        let (mut app, _) = app();
        app.history.worker = Some(crate::history::Worker::test_directory(folder("restore")));
        let mut session = Session::new(Kind::Dictation);
        session.text = "Current.".into();
        for text in ["First.", "Second."] {
            let mut version = TranscriptVersion::of(&session, "");
            version.text = text.into();
            version.replaced_ms += session.transcript_versions.len() as u64;
            session.transcript_versions.push(version);
        }
        let id = session.id.clone();
        let first = version_id(&session.transcript_versions[0]);
        let expected = transcript_revision(&session);
        app.history.selected = Some(session);
        app.calls_restore_version(&id, &first, &expected).unwrap();
        assert_eq!(app.history.selected.as_ref().unwrap().text, "First.");
        // A second click on the same button: the version and the transcript
        // it would replace are both gone, so nothing else is restored.
        assert!(app.calls_restore_version(&id, &first, &expected).is_err());
        let current = transcript_revision(app.history.selected.as_ref().unwrap());
        assert!(app.calls_restore_version(&id, &first, &current).is_err());
        assert_eq!(app.history.selected.as_ref().unwrap().text, "First.");
        let versions = &app.history.selected.as_ref().unwrap().transcript_versions;
        let texts: Vec<_> = versions.iter().map(|v| v.text.as_str()).collect();
        assert_eq!(texts, ["Second.", "Current."]);
    }

    #[test]
    fn destructive_actions_need_a_matching_confirmation() {
        let (mut app, _) = app();
        let directory = folder("confirm");
        app.history.worker = Some(crate::history::Worker::test_directory(directory.clone()));
        app.call_tools.history_dir = Some(directory.clone());
        let mut session = Session::new(Kind::Call);
        let id = session.id.clone();
        let audio = audio_store::path(&directory, &id);
        let recorder =
            audio_store::Recorder::start(audio.clone(), vec!["recording".into()], false).unwrap();
        recorder.push(0, vec![0.1; 16_000]);
        let info = recorder.finish().unwrap();
        let bytes = info.bytes;
        session.audio = Some(info);
        app.history.selected = Some(session);
        let delete = |confirm_id: &str, bytes: u64| {
            action(serde_json::json!({
                "type": "audio_delete",
                "id": id,
                "confirm": { "id": confirm_id, "bytes": bytes },
            }))
        };
        assert!(
            serde_json::from_value::<crate::app::desktop::Action>(
                serde_json::json!({"type": "audio_delete", "id": id})
            )
            .is_err(),
            "a confirmation is required"
        );
        assert!(app.desktop_action(delete("other", bytes)).is_err());
        assert!(app.desktop_action(delete(&id, bytes + 1)).is_err());
        assert!(audio.exists());
        app.desktop_action(delete(&id, bytes)).unwrap();
        assert!(!audio.exists());
        assert!(app.history.selected.as_ref().unwrap().audio.is_none());
        app.call_tools.people.remember_name("Morgan");
        app.call_tools.people.remember_name("Casey");
        let clear = |count: usize| {
            action(serde_json::json!({"type": "people_clear", "confirm": {"count": count}}))
        };
        assert!(app.desktop_action(clear(1)).is_err());
        assert_eq!(app.call_tools.people.list().len(), 2);
        app.desktop_action(clear(2)).unwrap();
        assert!(app.call_tools.people.list().is_empty());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn dictation_cancel_and_capture_cannot_touch_a_running_file_job() {
        let (mut app, _) = app();
        app.ready = true;
        app.call_tools.import.working = true;
        app.call_tools.import.rebuilding = Some(Session::new(Kind::Call).id);
        app.busy = true;
        let job = CancelToken::new();
        app.call_tools.import.cancel = Some(job.clone());
        assert!(
            app.desktop_action(action(serde_json::json!({"type": "dictation_cancel"})))
                .is_err()
        );
        assert!(app.busy && !job.is_cancelled());
        // Even if the shared flag is cleared, capture checks the job itself.
        app.cancel_dictation();
        assert!(app.busy);
        app.busy = false;
        assert!(
            app.desktop_action(action(serde_json::json!({"type": "dictation_start"})))
                .is_err()
        );
        app.toggle(None);
        assert!(app.recording.is_none());
        assert!(app.calls_file_job_running());
    }

    #[test]
    fn encryption_results_wait_for_recordings_and_count_people_failures() {
        let (mut app, _) = app();
        let directory = folder("protect");
        // A people file that cannot be read or rewritten.
        std::fs::create_dir_all(directory.join("people.json")).unwrap();
        app.call_tools.people = people::at(&directory, false);
        let audio = audio_store::path(&directory, &Session::new(Kind::Call).id);
        let recorder =
            audio_store::Recorder::start(audio.clone(), vec!["recording".into()], false).unwrap();
        recorder.push(0, vec![0.1; 31 * 16_000]);
        app.call_tools.recorder = Some(Arc::new(recorder));
        app.settings.encrypt_at_rest = true;
        app.history.storage.encryption_working = true;
        app.calls_set_protection(true);
        app.calls_protection_changed(Ok(MigrationReport {
            protect: true,
            changed: 3,
            failed: 0,
        }));
        let deadline = Instant::now() + Duration::from_secs(20);
        while app.history.protection.is_none() {
            assert!(app.history.storage.encryption_working);
            assert!(Instant::now() < deadline, "the recording never converted");
            std::thread::sleep(Duration::from_millis(20));
            app.calls_tick();
        }
        let report = app.history.protection.clone().unwrap().unwrap();
        assert_eq!(report.failed, 1, "the people file: {report:?}");
        assert!(!app.history.storage.encryption_working);
        let recorder = app.call_tools.recorder.take().unwrap();
        recorder.finish().unwrap();
        assert_eq!(recorder.protection(), audio_store::Protection::Settled);
        let bytes = std::fs::read(&audio).unwrap();
        assert!(crate::history::protect::is_sealed(&bytes[19 + 4..]));
        let _ = std::fs::remove_dir_all(directory);
    }
}
