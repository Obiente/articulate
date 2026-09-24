//! Local Assort classification with pinned built-in models and optional imports.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

pub mod builtin;
pub(crate) mod passages;

pub fn verify_builtin_models() -> Result<()> {
    builtin::verify_embedded()
}

const MAX_INPUT: usize = 2 * 1024 * 1024;
const MAX_OUTPUT: u64 = 1024 * 1024;
const MAX_SEGMENTS: usize = 10_000;
const MAX_HIGHLIGHTS: usize = 6;
const MAX_WORDS: usize = 120;
const MIN_IMPORTANCE: f32 = 0.65;
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Release-controlled metadata, added only after an Articulate held-out evaluation.
/// Hashes identify accepted artifacts; a model's `trained` declaration is insufficient.
struct ApprovedProfile {
    id: &'static str,
    manifest_sha256: &'static str,
    tokenizer_sha256: &'static str,
    limits_sha256: &'static str,
    pipeline_sha256: &'static str,
    evaluation_sha256: &'static str,
}

// Intentionally empty. Synthetic Assort demos are not an accepted Articulate model.
const APPROVED_PROFILES: &[ApprovedProfile] = &[];

/// Paths supplied by an explicit local package installation, never repository paths.
pub struct InstalledPackage {
    pub profile: String,
    /// Explicit opt-in for locally trained, unreviewed note suggestions only.
    pub local_preview: bool,
    pub checkpoint: PathBuf,
    pub tokenizer: PathBuf,
    pub limits: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    pub id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: Option<String>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<SegmentContext>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpeakerOrigin {
    Microphone,
    DiscordContext,
    Diarization,
    Uncertain,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SegmentContext {
    pub speaker_origin: SpeakerOrigin,
    pub overlapping_speech: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cues: Vec<crate::sensevoice::Cue>,
}

impl Segment {
    pub fn from_call_row(
        rows: &[crate::calls::Row],
        names: &[String],
        index: usize,
        id: String,
    ) -> Self {
        let row = &rows[index];
        let overlapping_speech = rows.iter().enumerate().any(|(other_index, other)| {
            index != other_index
                && !other.text.trim().is_empty()
                && row.start_ms < other.end_ms
                && other.start_ms < row.end_ms
                && crate::calls::label(row, names) != crate::calls::label(other, names)
        });
        let cues = row
            .cues
            .iter()
            .filter_map(|cue| {
                let start_ms = cue.start_ms.max(row.start_ms);
                let end_ms = cue.end_ms.min(row.end_ms);
                (start_ms < end_ms).then(|| crate::sensevoice::Cue {
                    start_ms,
                    end_ms,
                    label: cue.label.clone(),
                })
            })
            .collect();
        Self {
            id,
            start_ms: row.start_ms,
            end_ms: row.end_ms,
            speaker: Some(crate::calls::label(row, names)),
            text: row.text.clone(),
            context: Some(SegmentContext {
                speaker_origin: if row.microphone {
                    SpeakerOrigin::Microphone
                } else if row.discord.is_some() {
                    SpeakerOrigin::DiscordContext
                } else if row.speakers.iter().any(|speaker| *speaker > 0) {
                    SpeakerOrigin::Diarization
                } else {
                    SpeakerOrigin::Uncertain
                },
                overlapping_speech,
                cues,
            }),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transcript {
    pub id: String,
    pub title: String,
    pub goal: String,
    pub segments: Vec<Segment>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Decision,
    Action,
    KeyFact,
    Background,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Highlight {
    pub kind: Kind,
    /// Assort's 1 - P(background), not calibrated confidence in correctness.
    pub importance: f32,
    pub source: Segment,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub transcript_id: String,
    pub title: String,
    pub goal: String,
    pub word_count: usize,
    pub source_segments: usize,
    pub highlights: Vec<Highlight>,
}

#[cfg(test)]
pub struct Task {
    result: Receiver<Result<Summary, String>>,
    cancel: Arc<AtomicBool>,
}

#[cfg(test)]
impl Task {
    pub fn poll(&self) -> Option<Result<Summary, String>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                "The classifier stopped unexpectedly. Your transcript is unchanged.".into(),
            )),
        }
    }
}

#[cfg(test)]
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

struct RunningGuard;
impl Drop for RunningGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Release);
    }
}

/// A failed setup or pipe operation must not leave an unsupervised worker.
struct ChildGuard(std::process::Child);
impl std::ops::Deref for ChildGuard {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(windows)]
struct WorkerJob(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl WorkerJob {
    fn attach(child: &std::process::Child) -> Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            ensure!(
                !handle.is_null(),
                "Could not create the classification job: {}",
                std::io::Error::last_os_error()
            );
            let job = Self(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            limits.BasicLimitInformation.ActiveProcessLimit = 1;
            limits.ProcessMemoryLimit = 2 * 1024 * 1024 * 1024;
            ensure!(
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32
                ) != 0,
                "Could not apply classification limits: {}",
                std::io::Error::last_os_error()
            );
            ensure!(
                AssignProcessToJobObject(handle, child.as_raw_handle()) != 0,
                "Could not isolate the classification worker: {}",
                std::io::Error::last_os_error()
            );
            Ok(job)
        }
    }
}
#[cfg(windows)]
impl Drop for WorkerJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
pub fn start(package: InstalledPackage, transcript: Transcript) -> Result<Task> {
    let profile = APPROVED_PROFILES
        .iter()
        .find(|profile| profile.id == package.profile);
    ensure!(
        profile.is_some() || package.local_preview || builtin::contains(&package.profile),
        "No validated Articulate classifier is installed. Your transcript is unchanged."
    );
    validate_input(&transcript)?;
    ensure!(
        RUNNING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok(),
        "Another classification is already running."
    );
    let guard = RunningGuard;
    let (tx, result) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    thread::Builder::new()
        .name("assort-classification".into())
        .spawn(move || {
            let _guard = guard;
            let outcome =
                run(&package, profile, &transcript, &worker_cancel).map_err(|e| format!("{e:#}"));
            let _ = tx.send(outcome);
        })
        .context("Could not start local classification.")?;
    Ok(Task { result, cancel })
}

fn validate_input(transcript: &Transcript) -> Result<()> {
    ensure!(
        !transcript.id.trim().is_empty()
            && !transcript.title.trim().is_empty()
            && !transcript.goal.trim().is_empty(),
        "Transcript identity, title and goal are required."
    );
    ensure!(
        !transcript.segments.is_empty() && transcript.segments.len() <= MAX_SEGMENTS,
        "The transcript has an unsupported number of segments."
    );
    let mut ids = HashSet::new();
    let mut previous = 0;
    for segment in &transcript.segments {
        ensure!(
            !segment.id.trim().is_empty() && ids.insert(&segment.id),
            "Transcript segment identities must be unique."
        );
        ensure!(
            segment.start_ms >= previous && segment.end_ms >= segment.start_ms,
            "Transcript timestamps must be ordered and valid."
        );
        ensure!(
            !segment.text.trim().is_empty()
                && segment
                    .speaker
                    .as_ref()
                    .is_none_or(|name| !name.trim().is_empty()),
            "Transcript segments must contain text and valid speaker labels."
        );
        previous = segment.start_ms;
    }
    ensure!(
        serde_json::to_vec(transcript)?.len() <= MAX_INPUT,
        "This transcript is too large for local classification."
    );
    Ok(())
}

fn digest_file(path: &Path, max_bytes: u64) -> Result<String> {
    let mut file = File::open(path).context("A classification package file is missing.")?;
    ensure!(
        file.metadata()?.len() <= max_bytes,
        "A classification package file is too large."
    );
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut total = 0;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        ensure!(
            total <= max_bytes,
            "A classification package file changed size."
        );
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn verify_package(package: &InstalledPackage, profile: Option<&ApprovedProfile>) -> Result<()> {
    verify_task_package(package, profile, false)
}

fn verify_task_package(
    package: &InstalledPackage,
    profile: Option<&ApprovedProfile>,
    corrections: bool,
) -> Result<()> {
    let bundled = builtin::verify(package, corrections)?;
    if let Some(profile) = profile {
        ensure!(
            !profile.evaluation_sha256.is_empty(),
            "The classifier has no approved evaluation."
        );
        for (path, expected, limit) in [
            (
                package.checkpoint.join("manifest.json").as_path(),
                profile.manifest_sha256,
                65536,
            ),
            (
                package.tokenizer.as_path(),
                profile.tokenizer_sha256,
                32 * 1024 * 1024,
            ),
            (package.limits.as_path(), profile.limits_sha256, 65536),
            (
                package
                    .checkpoint
                    .join("transcript-pipeline.json")
                    .as_path(),
                profile.pipeline_sha256,
                65536,
            ),
        ] {
            ensure!(
                path.is_absolute(),
                "Classification package paths must be absolute."
            );
            ensure!(
                expected.len() == 64 && digest_file(path, limit)? == expected,
                "The classification package failed verification."
            );
        }
    }
    ensure!(
        package.local_preview || profile.is_some() || bundled,
        "Local preview requires explicit opt-in."
    );
    for path in [&package.checkpoint, &package.tokenizer, &package.limits] {
        ensure!(path.is_absolute(), "Classification paths must be absolute.");
    }
    if corrections {
        ensure!(
            package.local_preview || bundled,
            "Contextual correction models require explicit preview opt-in."
        );
        let contract: serde_json::Value = serde_json::from_slice(&bounded_read(
            File::open(package.checkpoint.join("articulate-task.json")).context(
                "Select a correction model trained by Articulate's current training recipe.",
            )?,
            65536,
            &AtomicBool::new(false),
        )?)?;
        ensure!(
            contract == correction_contract(),
            "This checkpoint was not trained for Articulate's correction task."
        );
    } else {
        let pipeline_file = File::open(package.checkpoint.join("transcript-pipeline.json"))
        .context("This checkpoint has no transcript training contract. Select a model produced by Assort's current transcript trainer.")?;
        let pipeline: serde_json::Value = serde_json::from_slice(&bounded_read(
            pipeline_file,
            65536,
            &AtomicBool::new(false),
        )?)?;
        ensure!(
            pipeline["version"] == 1
                && pipeline["category_ids"]
                    == serde_json::json!(["decision", "action", "key_fact", "background"])
                && pipeline["training_label_smoothing"]
                    .as_f64()
                    .is_some_and(|value| value.is_finite() && (0.0..1.0).contains(&value)),
            "This checkpoint was not trained for the supported transcript classification contract."
        );
    }
    let bytes = bounded_read(
        File::open(package.checkpoint.join("manifest.json"))?,
        65536,
        &AtomicBool::new(false),
    )?;
    let manifest: serde_json::Value = serde_json::from_slice(&bytes)?;
    ensure!(
        manifest["format_version"] == 1
            && manifest["architecture"] == "assort-candidate-scoring-v1"
            && manifest["burn_version"] == "0.21.0"
            && manifest["weights_status"] == "trained",
        "This classifier has incompatible or untrained weights."
    );
    let expected = manifest["weights_sha256"]
        .as_str()
        .context("The classifier is missing a weights digest.")?;
    ensure!(
        expected.len() == 64
            && digest_file(&package.checkpoint.join("weights.mpk"), 128 * 1024 * 1024)? == expected,
        "The classification weights failed verification."
    );
    ensure!(
        manifest["tokenizer"]["kind"] == "huggingface-json-v1"
            && manifest["tokenizer"]["pad_id"] == 0,
        "This model requires an unsupported tokenizer configuration."
    );
    let tokenizer_hash = manifest["tokenizer"]["fingerprint"]
        .as_str()
        .context("Missing tokenizer fingerprint.")?;
    ensure!(
        digest_file(&package.tokenizer, 32 * 1024 * 1024)? == tokenizer_hash,
        "The tokenizer does not match this model."
    );
    let bytes = bounded_read(File::open(&package.limits)?, 65536, &AtomicBool::new(false))?;
    let limits: serde_json::Value = serde_json::from_slice(&bytes)?;
    for (field, maximum) in [
        ("max_batch_size", 32),
        ("max_questions", 32),
        ("max_candidates", 32),
        ("max_state_tokens", 4096),
        ("max_question_tokens", 4096),
        ("max_candidate_tokens", 1024),
        ("max_padded_tokens", 1_048_576),
    ] {
        ensure!(
            limits[field]
                .as_u64()
                .is_some_and(|v| v > 0 && v <= maximum),
            "Unsupported classification limit: {field}"
        );
    }
    Ok(())
}

#[cfg(test)]
fn run(
    package: &InstalledPackage,
    profile: Option<&ApprovedProfile>,
    transcript: &Transcript,
    cancel: &AtomicBool,
) -> Result<Summary> {
    let output = run_process(
        package,
        profile,
        serde_json::to_vec(transcript)?,
        false,
        cancel,
    )?;
    validate_output(transcript, &output)
}

fn run_process(
    package: &InstalledPackage,
    profile: Option<&ApprovedProfile>,
    input: Vec<u8>,
    corrections: bool,
    cancel: &AtomicBool,
) -> Result<Vec<u8>> {
    verify_task_package(package, profile, corrections)?;
    ensure!(
        !cancel.load(Ordering::Relaxed),
        "Classification was cancelled."
    );
    let mut command = Command::new(
        std::env::current_exe().context("Could not find Articulate's bundled classifier.")?,
    );
    command
        .arg("--assort-worker")
        .args(corrections.then_some("--corrections"))
        .arg(
            profile
                .map(|profile| format!("--profile={}", profile.id))
                .unwrap_or_else(|| {
                    if builtin::contains(&package.profile) {
                        format!("--profile={}", package.profile)
                    } else {
                        "--local-preview".into()
                    }
                }),
        )
        .arg(&package.checkpoint)
        .arg(&package.tokenizer)
        .arg(&package.limits)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW for the local worker.
    }
    let mut child = ChildGuard(
        command
            .spawn()
            .context("Could not open the local Assort classifier.")?,
    );
    // The worker reads no input and loads no model before it is assigned to its job.
    // Closing Articulate closes this non-inheritable job handle and kills its worker.
    #[cfg(windows)]
    let _job = WorkerJob::attach(&child)?;
    let mut stdin = child
        .stdin
        .take()
        .context("Classifier input pipe is missing.")?;
    let stdout = child
        .stdout
        .take()
        .context("Classifier output pipe is missing.")?;
    let stderr = child
        .stderr
        .take()
        .context("Classifier diagnostic pipe is missing.")?;
    let overflow = Arc::new(AtomicBool::new(false));
    let out_overflow = overflow.clone();
    let err_overflow = overflow.clone();
    // Concurrent pipe readers avoid deadlocking a worker on a full output pipe.
    let writer = thread::spawn(move || stdin.write_all(&input));
    let reader = thread::spawn(move || bounded_read(stdout, MAX_OUTPUT, &out_overflow));
    let errors = thread::spawn(move || bounded_read(stderr, 65536, &err_overflow));
    let start = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Relaxed)
            || overflow.load(Ordering::Relaxed)
            || start.elapsed() >= Duration::from_secs(60)
        {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let written = writer
        .join()
        .map_err(|_| anyhow::anyhow!("Classifier input stopped."))?;
    let output = reader
        .join()
        .map_err(|_| anyhow::anyhow!("Classifier output stopped."))??;
    let _diagnostics = errors
        .join()
        .map_err(|_| anyhow::anyhow!("Classifier diagnostics stopped."))??;
    ensure!(
        status.is_some(),
        "Local classification stopped or timed out. Your transcript is unchanged."
    );
    written.context("Could not send the transcript to the classifier.")?;
    ensure!(
        status.unwrap().success(),
        "Assort could not classify this transcript. It may exceed the model's supported context. Your transcript is unchanged."
    );
    Ok(output)
}

/// Internal entry point of the same Articulate binary. The parent supervises
/// this worker so a slow or cancelled model cannot stall recording or the UI.
pub fn worker(args: &[String]) -> Result<()> {
    let corrections = args.first().is_some_and(|arg| arg == "--corrections");
    let package = worker_package(if corrections { &args[1..] } else { args })?;
    let bytes = bounded_read(
        std::io::stdin().lock(),
        MAX_INPUT as u64,
        &AtomicBool::new(false),
    )?;
    let output = if corrections {
        let request = serde_json::from_slice(&bytes)?;
        score_corrections(&package, &request)?
    } else {
        let transcript: Transcript = serde_json::from_slice(&bytes)?;
        classify_in_process(&package, &transcript)?
    };
    std::io::stdout().lock().write_all(&output)?;
    Ok(())
}

fn worker_package(args: &[String]) -> Result<InstalledPackage> {
    ensure!(
        args.len() == 4,
        "The classification worker requires a policy and three model paths."
    );
    let (profile, local_preview) = if args[0] == "--local-preview" {
        ("local-preview".to_owned(), true)
    } else {
        let id = args[0]
            .strip_prefix("--profile=")
            .context("Unsupported classification worker policy.")?;
        ensure!(
            APPROVED_PROFILES.iter().any(|profile| profile.id == id) || builtin::contains(id),
            "This classification profile is not approved."
        );
        (id.to_owned(), false)
    };
    let paths: Vec<PathBuf> = args[1..].iter().map(PathBuf::from).collect();
    ensure!(
        paths.iter().all(|path| path.is_absolute()),
        "Classification worker model paths must be absolute."
    );
    Ok(InstalledPackage {
        profile,
        local_preview,
        checkpoint: paths[0].clone(),
        tokenizer: paths[1].clone(),
        limits: paths[2].clone(),
    })
}

fn classify_in_process(package: &InstalledPackage, transcript: &Transcript) -> Result<Vec<u8>> {
    use assort_tokenizer::TextTokenizer;
    use burn::backend::Flex;
    validate_input(transcript)?;
    let profile = APPROVED_PROFILES
        .iter()
        .find(|profile| profile.id == package.profile);
    verify_package(package, profile)?;
    let tokenizer = assort_tokenizer::HfTokenizer::from_file(&package.tokenizer, 0)?;
    let (model, manifest) = assort_model::load_checkpoint::<Flex>(
        &package.checkpoint,
        &tokenizer.spec(),
        &Default::default(),
    )?;
    ensure!(
        manifest.weights_status == assort_model::WeightsStatus::Trained,
        "The classifier requires trained weights."
    );
    let limits: assort_data::BatchLimits = serde_json::from_slice(&bounded_read(
        File::open(&package.limits)?,
        65536,
        &AtomicBool::new(false),
    )?)?;
    let passages = passages::prepare(transcript, &tokenizer, &limits)?;
    let engine = assort_inference::Engine::new(model, tokenizer, limits)?;
    let input: assort_transcript::Transcript =
        serde_json::from_slice(&serde_json::to_vec(&passages)?)?;
    let scores = assort_transcript::score_transcript(&engine, &input)?;
    let mut summary = assort_transcript::select_summary(
        &input,
        &scores,
        &assort_transcript::SummaryOptions {
            max_words: MAX_WORDS,
            max_highlights: MAX_HIGHLIGHTS,
            min_importance: MIN_IMPORTANCE,
            ..Default::default()
        },
    )?;
    summary.source_segments = transcript.segments.len();
    let output = serde_json::to_vec(&summary)?;
    validate_output(transcript, &output)?;
    Ok(output)
}

pub struct CorrectionTask {
    result: Receiver<Result<crate::correction_context::Review, String>>,
    cancel: Arc<AtomicBool>,
}
impl CorrectionTask {
    pub fn poll(&self) -> Option<Result<crate::correction_context::Review, String>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                "The correction review stopped. Your text is unchanged.".into(),
            )),
        }
    }
}
impl Drop for CorrectionTask {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn start_corrections(
    package: InstalledPackage,
    request: crate::correction_context::Request,
) -> Result<CorrectionTask> {
    ensure!(
        package.local_preview || builtin::contains(&package.profile),
        "Enable the local correction preview first."
    );
    crate::correction_context::validate(&request)?;
    ensure!(
        RUNNING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok(),
        "Another classification is already running."
    );
    let guard = RunningGuard;
    let (tx, result) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    thread::Builder::new()
        .name("assort-correction-review".into())
        .spawn(move || {
            let _guard = guard;
            let result = (|| -> Result<_> {
                let bytes = run_process(
                    &package,
                    None,
                    serde_json::to_vec(&request)?,
                    true,
                    &worker_cancel,
                )?;
                crate::correction_context::review(request, &bytes)
            })();
            let _ = tx.send(result.map_err(|error| format!("{error:#}")));
        })?;
    Ok(CorrectionTask { result, cancel })
}

fn correction_contract() -> serde_json::Value {
    serde_json::json!({"version":1,"task":"corrections","question":{"id":"correction","text":crate::correction_context::QUESTION,"candidates":[
        {"id":"keep_original","description":crate::correction_context::KEEP},
        {"id":"replace","description":crate::correction_context::REPLACE}
    ]}})
}

fn score_corrections(
    package: &InstalledPackage,
    request: &crate::correction_context::Request,
) -> Result<Vec<u8>> {
    use assort_tokenizer::TextTokenizer;
    use burn::backend::Flex;
    crate::correction_context::validate(request)?;
    verify_task_package(package, None, true)?;
    let tokenizer = assort_tokenizer::HfTokenizer::from_file(&package.tokenizer, 0)?;
    let (model, manifest) = assort_model::load_checkpoint::<Flex>(
        &package.checkpoint,
        &tokenizer.spec(),
        &Default::default(),
    )?;
    ensure!(
        manifest.weights_status == assort_model::WeightsStatus::Trained,
        "The classifier requires trained weights."
    );
    let limits: assort_data::BatchLimits = serde_json::from_slice(&bounded_read(
        File::open(&package.limits)?,
        65536,
        &AtomicBool::new(false),
    )?)?;
    let engine = assort_inference::Engine::new(model, tokenizer, limits)?;
    let mut scores = Vec::new();
    // One bounded request at a time also supports checkpoints trained with a
    // small batch limit and avoids multiplying candidate attention memory.
    for (index, candidate) in request.candidates.iter().enumerate() {
        let responses = engine.evaluate(&[crate::correction_context::model_request(candidate)])?;
        let answer = &responses[0].answers[0];
        ensure!(
            answer.question_id == "correction"
                && answer.candidate_ids == ["keep_original", "replace"],
            "Unexpected correction candidate ordering."
        );
        scores.push(crate::correction_context::Score {
            candidate: index,
            replace_score: answer.distribution.probabilities()[1],
        });
    }
    let output = crate::correction_context::Output {
        request_hash: crate::correction_context::hash(request)?,
        scores,
    };
    let bytes = serde_json::to_vec(&output)?;
    crate::correction_context::review(request.clone(), &bytes)?;
    Ok(bytes)
}

fn bounded_read(reader: impl Read, limit: u64, overflow: &AtomicBool) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    reader.take(limit + 1).read_to_end(&mut output)?;
    if output.len() as u64 > limit {
        overflow.store(true, Ordering::Relaxed);
        anyhow::bail!("The classifier returned too much output.");
    }
    Ok(output)
}

fn validate_output(input: &Transcript, bytes: &[u8]) -> Result<Summary> {
    ensure!(
        bytes.len() as u64 <= MAX_OUTPUT,
        "The classifier returned too much output."
    );
    let summary: Summary =
        serde_json::from_slice(bytes).context("Assort returned an invalid summary.")?;
    ensure!(
        summary.transcript_id == input.id
            && summary.title == input.title
            && summary.goal == input.goal
            && summary.source_segments == input.segments.len(),
        "The summary does not belong to this transcript."
    );
    ensure!(
        summary.highlights.len() <= MAX_HIGHLIGHTS,
        "The summary exceeded its highlight budget."
    );
    let mut selected = HashSet::new();
    let mut previous = None;
    let mut words = 0;
    for highlight in &summary.highlights {
        let source = &highlight.source;
        let (index, range) = passages::locate(input, source)?;
        ensure!(
            selected.insert(&source.id)
                && previous
                    .is_none_or(|(row, end)| index > row || (index == row && range.start >= end)),
            "The summary changed, duplicated or reordered a source passage."
        );
        ensure!(
            highlight.kind != Kind::Background
                && highlight.importance.is_finite()
                && (MIN_IMPORTANCE..=1.0).contains(&highlight.importance),
            "The summary contains an unsupported classification."
        );
        previous = Some((index, range.end));
        words += source.text.split_whitespace().count();
    }
    ensure!(
        words == summary.word_count && words <= MAX_WORDS,
        "The summary exceeded its word budget."
    );
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_requires_explicit_policy_and_exact_absolute_path_arguments() {
        let base = std::env::temp_dir();
        let paths = [
            base.join("synthetic-checkpoint"),
            base.join("synthetic-tokenizer.json"),
            base.join("synthetic-limits.json"),
        ];
        let valid: Vec<String> = std::iter::once("--local-preview".to_owned())
            .chain(paths.iter().map(|path| path.to_string_lossy().into_owned()))
            .collect();
        let package = worker_package(&valid).unwrap();
        assert!(package.local_preview);
        assert_eq!(package.checkpoint, paths[0]);
        assert_eq!(package.tokenizer, paths[1]);
        assert_eq!(package.limits, paths[2]);
        assert!(worker_package(&[]).is_err());
        assert!(worker_package(&valid[1..]).is_err());
        for policy in [
            "--allow-random",
            "--profile=",
            "--profile=unreviewed",
            "preview",
        ] {
            let mut invalid = valid.clone();
            invalid[0] = policy.into();
            assert!(worker_package(&invalid).is_err());
        }
        let mut relative = valid.clone();
        relative[2] = "relative-tokenizer.json".into();
        assert!(worker_package(&relative).is_err());
        let mut extra = valid;
        extra.push("--allow-random".into());
        assert!(worker_package(&extra).is_err());
    }

    struct PackageFixture(PathBuf);
    impl PackageFixture {
        fn new() -> Self {
            let name = format!(
                "articulate-classification-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            let path = std::env::temp_dir().join(name);
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn package(&self) -> InstalledPackage {
            let weights = b"synthetic test weights, never loaded";
            let tokenizer = b"synthetic test tokenizer, never loaded";
            std::fs::write(self.0.join("weights.mpk"), weights).unwrap();
            std::fs::write(self.0.join("tokenizer.json"), tokenizer).unwrap();
            let manifest = serde_json::json!({"format_version":1,"architecture":"assort-candidate-scoring-v1","burn_version":"0.21.0","weights_status":"trained","weights_sha256":format!("{:x}",Sha256::digest(weights)),"tokenizer":{"kind":"huggingface-json-v1","pad_id":0,"fingerprint":format!("{:x}",Sha256::digest(tokenizer))}});
            std::fs::write(
                self.0.join("manifest.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            std::fs::write(self.0.join("transcript-pipeline.json"), br#"{"version":1,"category_ids":["decision","action","key_fact","background"],"training_label_smoothing":0.1}"#).unwrap();
            let limits = serde_json::json!({"max_batch_size":4,"max_questions":8,"max_candidates":4,"max_state_tokens":384,"max_question_tokens":128,"max_candidate_tokens":64,"max_padded_tokens":262144});
            std::fs::write(
                self.0.join("limits.json"),
                serde_json::to_vec(&limits).unwrap(),
            )
            .unwrap();
            InstalledPackage {
                profile: "local-preview".into(),
                local_preview: true,
                checkpoint: self.0.clone(),
                tokenizer: self.0.join("tokenizer.json"),
                limits: self.0.join("limits.json"),
            }
        }
    }
    impl Drop for PackageFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn transcript() -> Transcript {
        Transcript {
            id: "synthetic".into(),
            title: "Example".into(),
            goal: "Capture decisions".into(),
            segments: vec![Segment {
                id: "s1".into(),
                start_ms: 0,
                end_ms: 2000,
                speaker: Some("Speaker 1".into()),
                text: "We decided to postpone.".into(),
                context: None,
            }],
        }
    }
    fn output(input: &Transcript) -> serde_json::Value {
        serde_json::json!({"transcript_id": input.id,"title": input.title,"goal": input.goal,"word_count":4,"source_segments":1,"highlights":[{"kind":"decision","importance":0.9,"source":input.segments[0]}]})
    }
    #[test]
    fn legacy_note_task_reports_worker_failure_and_cancels_on_drop() {
        let (sender, result) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let task = Task {
            result,
            cancel: cancel.clone(),
        };
        assert!(task.poll().is_none());
        sender.send(Err("Synthetic worker failure".into())).unwrap();
        assert_eq!(
            task.poll().unwrap().unwrap_err(),
            "Synthetic worker failure"
        );
        drop(sender);
        assert!(task.poll().unwrap().is_err());
        drop(task);
        assert!(cancel.load(Ordering::Relaxed));
    }

    #[test]
    fn unapproved_demo_cannot_launch() {
        assert!(APPROVED_PROFILES.is_empty());
        let package = InstalledPackage {
            profile: "synthetic-demo".into(),
            local_preview: false,
            checkpoint: "unused".into(),
            tokenizer: "unused".into(),
            limits: "unused".into(),
        };
        assert!(start(package, transcript()).is_err());
    }
    #[test]
    fn preserves_exact_quotes_timestamps_and_speakers() {
        let input = transcript();
        assert!(validate_output(&input, &serde_json::to_vec(&output(&input)).unwrap()).is_ok());
        for (field, value) in [
            ("text", serde_json::json!("We decided to proceed.")),
            ("speaker", serde_json::json!("Invented name")),
            ("start_ms", serde_json::json!(100)),
            ("id", serde_json::json!("unknown")),
        ] {
            let mut summary = output(&input);
            summary["highlights"][0]["source"][field] = value;
            assert!(validate_output(&input, &serde_json::to_vec(&summary).unwrap()).is_err());
        }
    }
    #[test]
    fn excerpt_results_preserve_parent_ranges_and_reject_overlap() {
        let mut input = transcript();
        input.segments[0].text = "We agreed. I will send the notes.".into();
        let prepared = passages::prepare(
            &input,
            &assort_tokenizer::ByteTokenizer,
            &assort_transcript::transcript_limits(1),
        )
        .unwrap();
        let highlights: Vec<_> = prepared
            .segments
            .iter()
            .map(|source| serde_json::json!({"kind":"action", "importance":0.9, "source":source}))
            .collect();
        let mut summary = serde_json::json!({
            "transcript_id":input.id,"title":input.title,"goal":input.goal,
            "source_segments":1,"word_count":7,"highlights":highlights
        });
        assert!(validate_output(&input, &serde_json::to_vec(&summary).unwrap()).is_ok());
        summary["highlights"][1]["source"] = serde_json::to_value(&input.segments[0]).unwrap();
        summary["word_count"] = serde_json::json!(10);
        assert!(validate_output(&input, &serde_json::to_vec(&summary).unwrap()).is_err());
        summary["highlights"][1]["source"]["text"] =
            serde_json::json!("I will not send the notes.");
        assert!(validate_output(&input, &serde_json::to_vec(&summary).unwrap()).is_err());
    }
    #[test]
    fn rejects_wrong_session_duplicate_and_invalid_labels() {
        let input = transcript();
        for field in ["transcript_id", "goal", "title"] {
            let mut summary = output(&input);
            summary[field] = serde_json::json!("different");
            assert!(validate_output(&input, &serde_json::to_vec(&summary).unwrap()).is_err());
        }
        for kind in ["background", "rewrite", "command"] {
            let mut summary = output(&input);
            summary["highlights"][0]["kind"] = serde_json::json!(kind);
            assert!(validate_output(&input, &serde_json::to_vec(&summary).unwrap()).is_err());
        }
        let mut summary = output(&input);
        let duplicate = summary["highlights"][0].clone();
        summary["highlights"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(validate_output(&input, &serde_json::to_vec(&summary).unwrap()).is_err());
    }
    #[test]
    fn abstention_is_valid_and_limits_are_enforced() {
        let input = transcript();
        let mut summary = output(&input);
        summary["highlights"] = serde_json::json!([]);
        summary["word_count"] = serde_json::json!(0);
        assert!(
            validate_output(&input, &serde_json::to_vec(&summary).unwrap())
                .unwrap()
                .highlights
                .is_empty()
        );
        let overflow = AtomicBool::new(false);
        assert!(bounded_read(b"too much".as_slice(), 3, &overflow).is_err());
        assert!(overflow.load(Ordering::Relaxed));
        let mut bad = input;
        bad.segments[0].end_ms = 0;
        bad.segments[0].start_ms = 1;
        assert!(validate_input(&bad).is_err());
    }

    #[test]
    fn preview_still_rejects_random_and_mismatched_models() {
        let fixture = PackageFixture::new();
        let package = fixture.package();
        assert!(verify_package(&package, None).is_ok());
        std::fs::write(package.checkpoint.join("weights.mpk"), b"changed weights").unwrap();
        assert!(verify_package(&package, None).is_err());
        let package = fixture.package();
        std::fs::write(&package.tokenizer, b"different tokenizer").unwrap();
        assert!(verify_package(&package, None).is_err());
        let package = fixture.package();
        let path = package.checkpoint.join("manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["weights_status"] = serde_json::json!("random");
        std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(verify_package(&package, None).is_err());
    }

    #[test]
    fn preview_rejects_wrong_task_contract_and_excessive_context() {
        let fixture = PackageFixture::new();
        for contract in [
            serde_json::json!({"version":1,"category_ids":["keep_original","replace"],"training_label_smoothing":0.1}),
            serde_json::json!({"version":2,"category_ids":["decision","action","key_fact","background"],"training_label_smoothing":0.1}),
            serde_json::json!({"version":1,"category_ids":["decision","action","key_fact","background"],"training_label_smoothing":1.0}),
        ] {
            let package = fixture.package();
            std::fs::write(
                package.checkpoint.join("transcript-pipeline.json"),
                serde_json::to_vec(&contract).unwrap(),
            )
            .unwrap();
            assert!(verify_package(&package, None).is_err());
        }
        for field in ["max_state_tokens", "max_padded_tokens", "max_batch_size"] {
            let package = fixture.package();
            let mut limits: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&package.limits).unwrap()).unwrap();
            limits[field] = serde_json::json!(u64::MAX);
            std::fs::write(&package.limits, serde_json::to_vec(&limits).unwrap()).unwrap();
            assert!(verify_package(&package, None).is_err());
        }
    }

    #[test]
    fn correction_models_require_the_exact_training_recipe_contract() {
        let fixture = PackageFixture::new();
        let package = fixture.package();
        assert!(verify_task_package(&package, None, true).is_err());
        let path = package.checkpoint.join("articulate-task.json");
        std::fs::write(&path, serde_json::to_vec(&correction_contract()).unwrap()).unwrap();
        assert!(verify_task_package(&package, None, true).is_ok());
        let mut invalid = correction_contract();
        invalid["question"]["candidates"][1]["description"] = serde_json::json!("Rewrite anything");
        std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(verify_task_package(&package, None, true).is_err());
    }

    #[test]
    #[ignore = "Requires trained Assort model paths; runs bundled inference on synthetic text only"]
    fn bundled_assort_synthetic_smoke() {
        let package = InstalledPackage {
            profile: "local-preview".into(),
            local_preview: true,
            checkpoint: std::env::var_os("ARTICULATE_ASSORT_CHECKPOINT")
                .expect("Set ARTICULATE_ASSORT_CHECKPOINT")
                .into(),
            tokenizer: std::env::var_os("ARTICULATE_ASSORT_TOKENIZER")
                .expect("Set ARTICULATE_ASSORT_TOKENIZER")
                .into(),
            limits: std::env::var_os("ARTICULATE_ASSORT_LIMITS")
                .expect("Set ARTICULATE_ASSORT_LIMITS")
                .into(),
        };
        let input = transcript();
        let bytes = classify_in_process(&package, &input).unwrap();
        validate_output(&input, &bytes).unwrap();
        // A grouped speaker turn can be much longer than the question window.
        // Exercise real tokenization and inference without truncating the turn.
        let mut long = input;
        long.segments[0].text =
            "The installer is ready. I will send the report tomorrow. ".repeat(40);
        let bytes = classify_in_process(&package, &long).unwrap();
        let result = validate_output(&long, &bytes).unwrap();
        assert_eq!(result.source_segments, 1);
        for highlight in result.highlights {
            passages::locate(&long, &highlight.source).unwrap();
            assert!(highlight.source.text.len() < long.segments[0].text.len());
        }
    }

    #[test]
    #[ignore = "Requires an explicitly selected trained correction checkpoint; synthetic text only"]
    fn bundled_assort_correction_smoke() {
        let package = InstalledPackage {
            profile: "local-preview".into(),
            local_preview: true,
            checkpoint: std::env::var_os("ARTICULATE_ASSORT_CORRECTION_CHECKPOINT")
                .expect("Set correction checkpoint")
                .into(),
            tokenizer: std::env::var_os("ARTICULATE_ASSORT_CORRECTION_TOKENIZER")
                .expect("Set correction tokenizer")
                .into(),
            limits: std::env::var_os("ARTICULATE_ASSORT_CORRECTION_LIMITS")
                .expect("Set correction limits")
                .into(),
        };
        let request = crate::correction_context::prepare(
            "Write type script in this report.",
            &[crate::dictionary::validate("type script", "TypeScript").unwrap()],
            Some("notes.exe"),
        )
        .unwrap();
        let bytes = score_corrections(&package, &request).unwrap();
        assert_eq!(
            crate::correction_context::review(request, &bytes)
                .unwrap()
                .scores
                .len(),
            1
        );
    }
}
