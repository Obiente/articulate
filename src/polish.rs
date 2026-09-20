//! Optional, local dictation editing. Every result is a reviewable draft.
mod guard;
mod install;
mod runtime;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TryRecvError},
};
use std::time::{Duration, Instant};

pub use install::{MODEL_BYTES, MODEL_NAME, installed};
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
    pub elapsed_ms: u64,
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
                            let blocked = guard::check(
                                &task.request.source,
                                &candidate,
                                &task.request.protected,
                            )
                            .err()
                            .map(|e| e.to_string());
                            let text = if blocked.is_some() {
                                task.request.source.clone()
                            } else {
                                candidate
                            };
                            Ok(Preview {
                                original: task.request.original.clone(),
                                source: task.request.source.clone(),
                                text,
                                blocked,
                                elapsed_ms: start.elapsed().as_millis() as u64,
                            })
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
    let (events, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let task_cancel = cancel.clone();
    std::thread::spawn(move || {
        let result = install::download(&task_cancel, &events);
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
    #[ignore = "Requires explicitly prepared, ignored ARTICULATE_POLISH_TEST_DIR with pinned model and runtime.zip"]
    fn managed_cpu_editor_smoke() {
        let directory =
            std::env::var_os("ARTICULATE_POLISH_TEST_DIR").expect("Set an isolated test directory");
        assert!(std::path::Path::new(&directory).is_absolute());
        let (tx, _rx) = mpsc::channel();
        install::download(&AtomicBool::new(false), &tx).unwrap();
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
        install::download(&AtomicBool::new(false), &tx).unwrap();
        assert!(installed());
        install::verify(&AtomicBool::new(false)).unwrap();
    }
}
