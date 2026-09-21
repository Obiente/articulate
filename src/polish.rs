//! Optional, local dictation editing. Every result is a reviewable draft.
mod guard;
mod install;
pub(crate) mod runtime;
pub(crate) mod speech;
mod transfer;
mod transport;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TryRecvError},
};
use std::time::{Duration, Instant};

pub(crate) fn gpu_available() -> bool {
    install::gpu_available()
}
#[cfg(test)]
use install::installed;
pub use install::profile_installed;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelProfile {
    Polish,
    Summary,
}
pub const MAX_WORDS: usize = 200;
pub const MAX_BYTES: usize = 1600;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Style {
    #[default]
    Clear,
    Professional,
    Casual,
}

#[derive(Clone)]
pub struct Request {
    pub original: String,
    pub source: String,
    pub style: Style,
    pub protected: Vec<String>,
}

impl Request {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.source.trim().is_empty(),
            "Finish a dictation before polishing it."
        );
        ensure!(
            self.source.len() <= MAX_BYTES && self.source.split_whitespace().count() <= MAX_WORDS,
            "Polish supports up to 200 words and 1,600 bytes at a time. Shorten this passage first."
        );
        ensure!(
            self.original.len() <= 64_000
                && self.protected.len() <= 2_000
                && self.protected.iter().all(|s| s.len() <= 4_000),
            "This passage contains too much protected text."
        );
        ensure!(
            !self
                .source
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')),
            "This passage contains unsupported control characters."
        );
        Ok(())
    }
}

pub struct Preview {
    pub original: String,
    pub source: String,
    pub text: String,
    pub blocked: Option<String>,
    pub cleanup_only: bool,
    pub elapsed_ms: u64,
}

fn preview(request: &Request, candidate: String, elapsed_ms: u64) -> Preview {
    let blocked = guard::check(&request.source, &candidate, &request.protected)
        .err()
        .map(|error| error.to_string());
    let cleaned = speech::clean(&request.source, &request.protected);
    let cleanup_only = blocked.is_some() && cleaned != request.source;
    Preview {
        original: request.original.clone(),
        source: request.source.clone(),
        text: if blocked.is_some() {
            cleaned
        } else {
            candidate
        },
        blocked,
        cleanup_only,
        elapsed_ms,
    }
}

pub enum Event {
    Progress {
        stage: String,
        fraction: Option<f32>,
    },
    Installed,
    Complete(Preview),
    Failed(String),
    Cancelled,
}

pub struct Job {
    cancel: Arc<AtomicBool>,
    events: Receiver<Event>,
}
impl Job {
    pub fn try_recv(&self) -> std::result::Result<Event, TryRecvError> {
        self.events.try_recv()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct Task {
    request: Request,
    cancel: Arc<AtomicBool>,
    events: mpsc::Sender<Event>,
}

/// At most one waiting request. No runtime or model is loaded until requested.
#[derive(Default)]
pub struct Worker {
    commands: Option<SyncSender<Task>>,
}
impl Worker {
    pub fn start(&mut self, request: Request) -> Result<Job> {
        request.validate()?;
        if self.commands.is_none() {
            let (tx, rx) = mpsc::sync_channel::<Task>(1);
            std::thread::Builder::new()
                .name("dictation-polish".into())
                .spawn(move || {
                    let mut server: Option<runtime::Server> = None;
                    loop {
                        let task = match rx.recv_timeout(Duration::from_secs(60)) {
                            Ok(task) => task,
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                server = None;
                                continue;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        };
                        let start = Instant::now();
                        let result = (|| -> Result<Preview> {
                            check_cancel(&task.cancel)?;
                            if server.is_none() {
                                progress(&task.events, "Preparing local editor", None);
                                server = Some(runtime::Server::start(&task.cancel)?);
                            }
                            check_cancel(&task.cancel)?;
                            progress(&task.events, "Polishing on this computer", None);
                            let candidate = server
                                .as_mut()
                                .unwrap()
                                .polish(&task.request, &task.cancel)?;
                            check_cancel(&task.cancel)?;
                            Ok(preview(
                                &task.request,
                                candidate,
                                start.elapsed().as_millis() as u64,
                            ))
                        })();
                        let event = match result {
                            _ if task.cancel.load(Ordering::Acquire) => {
                                server = None;
                                Event::Cancelled
                            }
                            Ok(preview) => Event::Complete(preview),
                            Err(error) => {
                                server = None;
                                Event::Failed(error.to_string())
                            }
                        };
                        let _ = task.events.send(event);
                    }
                })?;
            self.commands = Some(tx);
        }
        let (events, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.commands
            .as_ref()
            .unwrap()
            .try_send(Task {
                request,
                cancel: cancel.clone(),
                events,
            })
            .map_err(|_| {
                anyhow::anyhow!("The local editor is still busy. Cancel its current draft first.")
            })?;
        Ok(Job { cancel, events: rx })
    }
}

pub fn download() -> Job {
    download_profile(ModelProfile::Polish)
}
pub fn download_profile(profile: ModelProfile) -> Job {
    let (events, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let task_cancel = cancel.clone();
    std::thread::spawn(move || {
        let result = install::download_profile(profile, &task_cancel, &events);
        let event = if task_cancel.load(Ordering::Acquire) {
            Event::Cancelled
        } else {
            match result {
                Ok(()) => Event::Installed,
                Err(e) => Event::Failed(e.to_string()),
            }
        };
        let _ = events.send(event);
    });
    Job { cancel, events: rx }
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Acquire), "Polish cancelled.");
    Ok(())
}
fn progress(events: &mpsc::Sender<Event>, stage: &str, fraction: Option<f32>) {
    let _ = events.send(Event::Progress {
        stage: stage.into(),
        fraction,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejected_rewriting_keeps_useful_speech_cleanup_as_a_reviewable_preview() {
        let source = "I was checking up on the um on the invoice. It hasn't been paid.";
        let request = Request {
            original: source.into(),
            source: source.into(),
            style: Style::Professional,
            protected: vec![],
        };
        let result = preview(&request, "The invoice has been paid.".into(), 1);
        assert!(result.blocked.is_some());
        assert!(result.cleanup_only);
        assert_eq!(
            result.text,
            "I was checking up on the invoice. It hasn't been paid."
        );
        assert_eq!(result.original, source);
        assert_eq!(result.source, source);
        let request = Request {
            original: "Do not send it.".into(),
            source: "Do not send it.".into(),
            ..request
        };
        let result = preview(&request, "Do send it.".into(), 1);
        assert!(result.blocked.is_some());
        assert!(!result.cleanup_only);
        assert_eq!(result.text, request.source);
    }

    #[test]
    #[ignore = "Requires an explicitly prepared isolated model cache; only reads model assets"]
    fn managed_speech_cleanup_regressions() {
        let directory =
            std::env::var_os("ARTICULATE_POLISH_TEST_DIR").expect("Set an isolated test directory");
        assert!(std::path::Path::new(&directory).is_absolute());
        let mut worker = Worker::default();
        for (source, expected) in [
            (
                "Hey, how are you doing? I was just checking up on the um on the invoice. It hasn't been paid, just like the other three past monthly invoices. Please let me know when you're able to pay.",
                "on the invoice",
            ),
            (
                "I I need to need to send the invoice. I need to send the invoice.",
                "send the invoice",
            ),
            (
                "Please schedule it for Tuesday, sorry, Thursday at 3, I mean 4 pm.",
                "Thursday",
            ),
            (
                "Send the invoice to Alice, I mean send the invoice to Bob.",
                "Bob",
            ),
        ] {
            let job = worker
                .start(Request {
                    original: source.into(),
                    source: source.into(),
                    style: Style::Professional,
                    protected: vec![],
                })
                .unwrap();
            loop {
                match job.events.recv_timeout(Duration::from_secs(90)).unwrap() {
                    Event::Complete(result) => {
                        assert!(
                            result.blocked.is_none() || result.cleanup_only,
                            "{:?}",
                            result.blocked
                        );
                        assert_ne!(result.text, source);
                        assert!(result.text.contains(expected), "{}", result.text);
                        assert_eq!(result.text.matches(expected).count(), 1, "{}", result.text);
                        assert!(!result.text.contains(" um "));
                        assert_eq!(result.original, source);
                        println!(
                            "elapsed_ms={} cleanup_only={} text={}",
                            result.elapsed_ms, result.cleanup_only, result.text
                        );
                        break;
                    }
                    Event::Failed(error) => panic!("{error}"),
                    Event::Cancelled => panic!("Cancelled"),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn bounded_requests_and_cancellation() {
        let request = Request {
            original: "original".into(),
            source: "Hello Casey.".into(),
            style: Style::Clear,
            protected: vec![],
        };
        assert!(request.validate().is_ok());
        assert!(
            Request {
                source: "hello ".repeat(201),
                ..request.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            Request {
                source: "hello\0".into(),
                ..request
            }
            .validate()
            .is_err()
        );
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = Job {
            cancel: cancel.clone(),
            events: rx,
        };
        drop(job);
        drop(tx);
        assert!(check_cancel(&cancel).is_err());
    }

    #[test]
    #[ignore = "Downloads the pinned summary model into an explicitly selected isolated cache"]
    fn download_summary_model() {
        let root =
            std::env::var_os("ARTICULATE_POLISH_TEST_DIR").expect("Set an isolated model cache");
        assert!(std::path::Path::new(&root).is_absolute());
        let job = download_profile(ModelProfile::Summary);
        loop {
            match job.events.recv_timeout(Duration::from_secs(120)).unwrap() {
                Event::Progress { stage, fraction } => {
                    println!("{stage}: {:?}", fraction.map(|v| (v * 100.0) as u32))
                }
                Event::Installed => break,
                Event::Failed(error) => panic!("{error}"),
                Event::Cancelled => panic!("Cancelled"),
                Event::Complete(_) => unreachable!(),
            }
        }
        install::verify_profile(ModelProfile::Summary, &AtomicBool::new(false)).unwrap();
    }

    #[test]
    #[ignore = "Requires explicitly prepared, ignored ARTICULATE_POLISH_TEST_DIR with pinned model and runtime.zip"]
    fn managed_cpu_editor_smoke() {
        let directory =
            std::env::var_os("ARTICULATE_POLISH_TEST_DIR").expect("Set an isolated test directory");
        assert!(std::path::Path::new(&directory).is_absolute());
        let (tx, _rx) = mpsc::channel();
        install::download(&Arc::new(AtomicBool::new(false)), &tx).unwrap();
        let mut worker = Worker::default();
        for (index, (style, source)) in [
            (
                Style::Clear,
                "The server are ready and the logs is available.",
            ),
            (
                Style::Clear,
                "I need the invoice. I need the invoice. Please send it today.",
            ),
            (
                Style::Professional,
                "hello casey can you send the report by friday please",
            ),
            (Style::Casual, "Please send the report tomorrow."),
        ]
        .into_iter()
        .enumerate()
        {
            let job = worker
                .start(Request {
                    original: source.into(),
                    source: source.into(),
                    style,
                    protected: vec![],
                })
                .unwrap();
            let started = Instant::now();
            loop {
                assert!(
                    started.elapsed() < Duration::from_secs(90),
                    "Managed editor timed out"
                );
                match job.try_recv() {
                    Ok(Event::Complete(preview)) => {
                        assert!(preview.blocked.is_none(), "{:?}", preview.blocked);
                        assert_eq!(preview.original, source);
                        if index == 0 {
                            assert_eq!(
                                preview.text,
                                "The server is ready and the logs are available."
                            );
                        }
                        println!(
                            "synthetic_case={index} elapsed_ms={} changed={}",
                            preview.elapsed_ms,
                            preview.text != source
                        );
                        break;
                    }
                    Ok(Event::Failed(error)) => panic!("{error}"),
                    Ok(Event::Cancelled) => panic!("Unexpected cancellation"),
                    Err(TryRecvError::Disconnected) => panic!("Worker disconnected"),
                    _ => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        }
        let job = worker
            .start(Request {
                original: "Hello.".into(),
                source: "Hello.".into(),
                style: Style::Clear,
                protected: vec![],
            })
            .unwrap();
        let start = Instant::now();
        let mut cancel_requested = false;
        loop {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "Cancellation was not responsive"
            );
            match job.try_recv() {
                Ok(Event::Progress { stage, .. }) if stage == "Polishing on this computer" => {
                    job.cancel();
                    cancel_requested = true;
                }
                Ok(Event::Cancelled) => break,
                Ok(Event::Complete(_)) => panic!("Cancelled draft was returned"),
                _ => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        assert!(cancel_requested);
        drop(job);
        drop(worker);
        // Repair a missing runtime DLL from the verified cached archive.
        let missing = install::root().join("runtime").join("ggml-cpu-x64.dll");
        std::fs::remove_file(&missing).unwrap();
        assert!(!installed());
        install::download(&Arc::new(AtomicBool::new(false)), &tx).unwrap();
        assert!(installed());
        install::verify_profile(ModelProfile::Polish, &AtomicBool::new(false)).unwrap();
    }
}
