//! The controller thread and the handle IPC commands use to reach it.
//!
//! The controller owns `App` on one thread. Each loop iteration, action and
//! close request runs inside `catch_unwind`: a panic stops normal work, is
//! logged and shown to the renderer, and leaves the window closable.

use super::Action;
use super::publish::{Changes, Pass, Published, StateUpdate, Tracker};
use super::view::{
    self, ControllerHealth, Overlay,
    shell::{ControllerSection, ControllerState},
};
use crate::app::App;
use std::{
    any::Any,
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError},
    },
    time::{Duration, Instant},
};

/// Live sections are compared at most this often.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(250);
/// The controller polls capture and worker channels at this interval.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// How long the window has to acknowledge a close request.
pub(crate) const CLOSE_ACK_TIMEOUT: Duration = Duration::from_secs(3);
/// An unanswered close request must be at least this old before another
/// request skips the window.
const ESCALATE_UNANSWERED_AFTER: Duration = Duration::from_secs(1);
/// After acknowledging, a window that has not closed within this time is
/// treated as stuck when the user asks to close again.
const ESCALATE_ACKNOWLEDGED_AFTER: Duration = Duration::from_secs(2);
/// Saving normally settles within five seconds; allow for a slow disk.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);

const UNAVAILABLE: &str =
    "Articulate stopped working and cannot do this. Close Articulate and open it again.";
const UNVERIFIED: &str =
    "Articulate stopped working, so your latest changes could not be verified as saved.";

enum Request {
    Action(Action, Sender<Result<(), String>>),
    /// A cached participant picture for a `bytes://discord-avatar/...` identifier.
    Avatar(String, Sender<Option<Vec<u8>>>),
    Shutdown(Sender<Result<(), String>>),
}

/// Receivers of published state. They run on the controller thread and must
/// not block.
pub(crate) struct Listeners {
    pub state: Box<dyn Fn(&StateUpdate) + Send>,
    pub overlay: Box<dyn Fn(&Overlay) + Send>,
}

struct Shared {
    published: Mutex<Published>,
    overlay: Mutex<Overlay>,
    health: Mutex<ControllerHealth>,
    /// Startup failed before any document could open; closing is safe.
    startup_failed: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn panic_message(panic: Box<dyn Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "an unexpected internal error".into())
}

/// Publishes controller state. Owned by the controller thread.
struct Publisher {
    tracker: Tracker,
    shared: Arc<Shared>,
    listeners: Listeners,
}

impl Publisher {
    fn send(&mut self, changes: Changes, app: Option<&App>) {
        if changes.is_empty() {
            return;
        }
        let update = lock(&self.shared.published).apply(changes);
        if update.is_empty() {
            return;
        }
        if update.contains("overlay")
            && let Some(app) = app
        {
            self.overlay(view::overlay(app));
        }
        (self.listeners.state)(&update);
    }

    fn overlay(&mut self, overlay: Overlay) {
        let mut current = lock(&self.shared.overlay);
        if *current != overlay {
            current.clone_from(&overlay);
            drop(current);
            (self.listeners.overlay)(&overlay);
        }
    }

    fn app(&mut self, app: &App, pass: Pass) {
        let mut collector = self.tracker.pass(pass);
        view::collect(app, &mut collector);
        let changes = collector.finish();
        self.send(changes, Some(app));
    }

    fn health(&mut self, health: ControllerHealth) {
        lock(&self.shared.health).clone_from(&health);
        let changes = self.tracker.single(
            "controller",
            &ControllerSection {
                controller: &health,
            },
        );
        self.send(changes, None);
    }

    fn startup_failed(&mut self, message: String) {
        log::error!("Articulate could not start: {message}");
        self.shared.startup_failed.store(true, Ordering::Release);
        let shell = view::shell::starting(&message, false);
        let changes = self.tracker.single("shell", &shell);
        self.send(changes, None);
        self.health(ControllerHealth::new(
            ControllerState::Failed,
            Some(message),
        ));
    }
}

enum Exit {
    Quit,
    Failed(String),
}

fn run(requests: Receiver<Request>, mut publisher: Publisher) {
    transcribe_cpp::disable_logging();
    if let Err(error) = transcribe_cpp::init_backends_default() {
        publisher.startup_failed(format!("Could not initialize speech: {error}"));
        return;
    }
    let mut app = match catch_unwind(App::new) {
        Ok(app) => app,
        Err(panic) => {
            publisher.startup_failed(format!(
                "Articulate could not start: {}",
                panic_message(panic)
            ));
            return;
        }
    };
    if let Some(worker) = &app.history.worker {
        worker.insights();
    }
    publisher.health(ControllerHealth::new(ControllerState::Running, None));
    log::info!("Application controller started");
    match running(&mut app, &requests, &mut publisher) {
        Exit::Quit => log::info!("Application controller finished"),
        Exit::Failed(message) => failed(&mut app, &requests, &mut publisher, message),
    }
}

fn reply_completed(
    reply: Sender<Result<(), String>>,
    result: Result<(), String>,
    shutting_down: bool,
) -> bool {
    let quit = shutting_down && result.is_ok();
    reply.send(result).is_ok() && quit
}

fn running(app: &mut App, requests: &Receiver<Request>, publisher: &mut Publisher) -> Exit {
    let mut next_publish = Instant::now();
    loop {
        let iteration = catch_unwind(AssertUnwindSafe(|| {
            app.receive();
            app.poll_discord_launch();
            let now = Instant::now();
            if now >= next_publish {
                publisher.app(app, Pass::Tick);
                next_publish = now + PUBLISH_INTERVAL;
            }
        }));
        if let Err(panic) = iteration {
            return Exit::Failed(panic_message(panic));
        }
        let (request, reply) = match requests.recv_timeout(POLL_INTERVAL) {
            Ok(Request::Action(action, reply)) => (Some(action), reply),
            Ok(Request::Avatar(uri, reply)) => {
                let bytes = catch_unwind(AssertUnwindSafe(|| app.desktop_discord_avatar(&uri)));
                match bytes {
                    Ok(bytes) => {
                        let _ = reply.send(bytes);
                        continue;
                    }
                    Err(panic) => {
                        let _ = reply.send(None);
                        return Exit::Failed(panic_message(panic));
                    }
                }
            }
            Ok(Request::Shutdown(reply)) => (None, reply),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return Exit::Quit,
        };
        let shutting_down = request.is_none();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let result = match request {
                Some(action) => app.desktop_action(action),
                None => app.desktop_shutdown(),
            };
            if let Err(error) = &result {
                app.status.clone_from(error);
            }
            publisher.app(app, Pass::Full);
            result
        }));
        match result {
            Ok(result) => {
                if reply_completed(reply, result, shutting_down) {
                    return Exit::Quit;
                }
            }
            Err(panic) => {
                let message = panic_message(panic);
                let _ = reply.send(Err(if shutting_down {
                    UNVERIFIED.into()
                } else {
                    UNAVAILABLE.into()
                }));
                return Exit::Failed(message);
            }
        }
    }
}

/// After a panic, answer requests without resuming normal work. A close
/// request still tries to save, so a clean close remains possible.
fn failed(app: &mut App, requests: &Receiver<Request>, publisher: &mut Publisher, message: String) {
    log::error!("The application controller stopped: {message}");
    let status = format!("Articulate stopped working: {message}");
    let _ = catch_unwind(AssertUnwindSafe(|| {
        app.status.clone_from(&status);
        publisher.app(app, Pass::Full);
    }));
    publisher.overlay(Overlay::default());
    publisher.health(ControllerHealth::new(
        ControllerState::Failed,
        Some(message),
    ));
    while let Ok(request) = requests.recv() {
        match request {
            Request::Action(_, reply) => {
                let _ = reply.send(Err(UNAVAILABLE.into()));
            }
            Request::Avatar(_, reply) => {
                let _ = reply.send(None);
            }
            Request::Shutdown(reply) => {
                match catch_unwind(AssertUnwindSafe(|| app.desktop_shutdown())) {
                    Ok(Ok(())) => {
                        log::info!("Saved changes after the controller stopped");
                        if reply.send(Ok(())).is_ok() {
                            return;
                        }
                    }
                    Ok(Err(error)) => {
                        let _ = reply.send(Err(format!("{UNVERIFIED} {error}")));
                    }
                    Err(_) => {
                        let _ = reply.send(Err(UNVERIFIED.into()));
                    }
                }
            }
        }
    }
}

/// How to handle a window close request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloseStep {
    /// Ask the window to save its edits, then wait for its acknowledgement.
    AskRenderer(u64),
    /// Save and close from native code without the window.
    Native,
    /// A close is already being handled.
    InProgress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Renderer { acknowledged: Option<Instant> },
    Native,
}

#[derive(Clone, Copy, Debug)]
struct PendingClose {
    id: u64,
    requested: Instant,
    stage: Stage,
}

/// Close handshake with the window. A request first goes to the window so it
/// can save unsent edits. An unanswered request falls back to native
/// shutdown, and so does a repeated request while the window seems stuck.
#[derive(Default, Debug)]
struct CloseState {
    next_id: u64,
    pending: Option<PendingClose>,
}

impl CloseState {
    fn request(&mut self, has_frontend: bool, now: Instant) -> CloseStep {
        let native = |id, requested| PendingClose {
            id,
            requested,
            stage: Stage::Native,
        };
        match self.pending {
            Some(PendingClose {
                stage: Stage::Native,
                ..
            }) => CloseStep::InProgress,
            None if !has_frontend => {
                self.next_id += 1;
                self.pending = Some(native(self.next_id, now));
                CloseStep::Native
            }
            None => {
                self.next_id += 1;
                self.pending = Some(PendingClose {
                    id: self.next_id,
                    requested: now,
                    stage: Stage::Renderer { acknowledged: None },
                });
                CloseStep::AskRenderer(self.next_id)
            }
            Some(PendingClose {
                id,
                requested,
                stage: Stage::Renderer { acknowledged },
            }) => {
                let stuck = match acknowledged {
                    None => now.duration_since(requested) >= ESCALATE_UNANSWERED_AFTER,
                    Some(at) => now.duration_since(at) >= ESCALATE_ACKNOWLEDGED_AFTER,
                };
                if stuck {
                    self.pending = Some(native(id, requested));
                    CloseStep::Native
                } else {
                    CloseStep::InProgress
                }
            }
        }
    }

    fn acknowledge(&mut self, id: u64, cancelled: bool, now: Instant) {
        if let Some(pending) = &mut self.pending
            && pending.id == id
            && matches!(pending.stage, Stage::Renderer { .. })
        {
            if cancelled {
                self.pending = None;
            } else {
                pending.stage = Stage::Renderer {
                    acknowledged: Some(now),
                };
            }
        }
    }

    /// Whether request `id` is still waiting for the window to answer.
    fn unanswered(&self, id: u64) -> bool {
        matches!(
            self.pending,
            Some(PendingClose {
                id: pending,
                stage: Stage::Renderer { acknowledged: None },
                ..
            }) if pending == id
        )
    }
}

/// The IPC-facing handle to the controller thread.
pub(crate) struct Bridge {
    sender: SyncSender<Request>,
    shared: Arc<Shared>,
    frontend_seen: AtomicBool,
    close: Mutex<CloseState>,
    close_answered: Condvar,
}

impl Bridge {
    pub(crate) fn start(listeners: Listeners) -> Result<Self, String> {
        let (sender, requests) = mpsc::sync_channel(32);
        let shared = Arc::new(Shared {
            published: Mutex::default(),
            overlay: Mutex::default(),
            health: Mutex::new(ControllerHealth::new(ControllerState::Starting, None)),
            startup_failed: AtomicBool::new(false),
        });
        let mut publisher = Publisher {
            tracker: Tracker::default(),
            shared: shared.clone(),
            listeners,
        };
        let changes = publisher
            .tracker
            .single("shell", &view::shell::starting("Starting Articulate", true));
        publisher.send(changes, None);
        publisher.health(ControllerHealth::new(ControllerState::Starting, None));
        std::thread::Builder::new()
            .name("articulate-controller".into())
            .spawn(move || run(requests, publisher))
            .map_err(|error| error.to_string())?;
        Ok(Self::with_channel(sender, shared))
    }

    fn with_channel(sender: SyncSender<Request>, shared: Arc<Shared>) -> Self {
        Self {
            sender,
            shared,
            frontend_seen: AtomicBool::new(false),
            close: Mutex::default(),
            close_answered: Condvar::new(),
        }
    }

    /// Sections newer than `since`, or all of them.
    pub(crate) fn snapshot(&self, since: Option<&HashMap<String, u64>>) -> StateUpdate {
        self.frontend_seen.store(true, Ordering::Relaxed);
        lock(&self.shared.published).since(since)
    }

    /// The newest section revision. A cheap check for missed events.
    pub(crate) fn revision(&self) -> HashMap<String, u64> {
        lock(&self.shared.published).revision()
    }

    pub(crate) fn setting_bool(&self, key: &str) -> bool {
        lock(&self.shared.published).setting_bool(key)
    }

    pub(crate) fn has_frontend(&self) -> bool {
        self.frontend_seen.load(Ordering::Relaxed)
    }

    pub(crate) fn overlay(&self) -> Overlay {
        lock(&self.shared.overlay).clone()
    }

    pub(crate) fn health(&self) -> ControllerHealth {
        lock(&self.shared.health).clone()
    }

    fn mark_unresponsive(&self, message: &str) {
        let mut health = lock(&self.shared.health);
        if !health.can_close_anyway {
            *health =
                ControllerHealth::new(ControllerState::Unresponsive, Some(message.to_owned()));
            lock(&self.shared.published).health(&ControllerSection {
                controller: &health,
            });
        }
    }

    /// Closing without verified saves is offered only once the controller
    /// failed or stopped answering.
    pub(crate) fn force_close_allowed(&self) -> bool {
        self.health().can_close_anyway
    }

    pub(crate) fn action(&self, action: Action) -> Result<(), String> {
        if let Some(result) = action.run_outside_controller() {
            return result;
        }
        let (reply, response) = mpsc::channel();
        self.sender
            .try_send(Request::Action(action, reply))
            .map_err(|error| match error {
                TrySendError::Full(_) => "Articulate is busy. Try again.".to_owned(),
                TrySendError::Disconnected(_) => UNAVAILABLE.to_owned(),
            })?;
        response
            .recv()
            .map_err(|_| "The application controller stopped.".to_owned())?
    }

    /// A cached participant picture, or `None` while it is not available.
    pub(crate) fn avatar(&self, uri: String) -> Option<Vec<u8>> {
        let (reply, response) = mpsc::channel();
        self.sender.try_send(Request::Avatar(uri, reply)).ok()?;
        response.recv().ok().flatten()
    }

    /// Ask the controller to save everything and stop.
    pub(crate) fn shutdown(&self) -> Result<(), String> {
        let (reply, response) = mpsc::channel();
        match self.sender.try_send(Request::Shutdown(reply)) {
            Ok(()) => {}
            Err(TrySendError::Disconnected(_)) => return self.stopped_controller_close(),
            Err(TrySendError::Full(_)) => {
                return Err("Articulate is busy. Try closing again.".into());
            }
        }
        match response.recv_timeout(SHUTDOWN_TIMEOUT) {
            Ok(result) => result,
            Err(RecvTimeoutError::Disconnected) => self.stopped_controller_close(),
            Err(RecvTimeoutError::Timeout) => {
                let message = "Articulate is not responding, so your latest changes could not be verified as saved.";
                log::error!("The controller did not answer a close request in time");
                self.mark_unresponsive(message);
                Err(message.into())
            }
        }
    }

    fn stopped_controller_close(&self) -> Result<(), String> {
        if self.shared.startup_failed.load(Ordering::Acquire) {
            Ok(())
        } else {
            let message = "The application controller stopped. Unsaved work could not be verified.";
            self.mark_unresponsive(message);
            Err(message.into())
        }
    }

    pub(crate) fn request_close(&self) -> CloseStep {
        lock(&self.close).request(self.has_frontend(), Instant::now())
    }

    /// The window received close request `id`. `cancelled` means it will not
    /// close, for example because saving its edits failed.
    pub(crate) fn acknowledge_close(&self, id: u64, cancelled: bool) {
        lock(&self.close).acknowledge(id, cancelled, Instant::now());
        self.close_answered.notify_all();
    }

    /// Wait for the window to answer request `id`. Returns true when the
    /// request is still unanswered after `timeout` and now moves to native
    /// shutdown.
    pub(crate) fn close_unanswered(&self, id: u64, timeout: Duration) -> bool {
        let state = lock(&self.close);
        let (mut state, _) = self
            .close_answered
            .wait_timeout_while(state, timeout, |state| state.unanswered(id))
            .unwrap_or_else(|error| error.into_inner());
        if !state.unanswered(id) {
            return false;
        }
        if let Some(pending) = &mut state.pending {
            pending.stage = Stage::Native;
        }
        true
    }

    /// The close attempt ended without closing. The next request starts over.
    pub(crate) fn close_finished(&self) {
        lock(&self.close).pending = None;
        self.close_answered.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_startup_can_close_but_a_stopped_controller_cannot_skip_save_checks() {
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(receiver);
        let shared = Arc::new(Shared {
            published: Mutex::default(),
            overlay: Mutex::default(),
            health: Mutex::new(ControllerHealth::new(ControllerState::Running, None)),
            startup_failed: AtomicBool::new(false),
        });
        let bridge = Bridge::with_channel(sender, shared);
        assert!(!bridge.setting_bool("close_to_tray"));
        assert!(!bridge.has_frontend());
        assert!(!bridge.force_close_allowed());
        assert!(bridge.shutdown().is_err());
        assert!(
            bridge.force_close_allowed(),
            "a stopped controller offers closing anyway"
        );
        let snapshot = bridge.snapshot(None);
        assert_eq!(
            snapshot.sections["controller"]
                .data
                .as_ref()
                .unwrap()
                .value()["controller"]["state"],
            "unresponsive"
        );
        bridge.shared.startup_failed.store(true, Ordering::Release);
        assert!(bridge.shutdown().is_ok());
    }

    #[test]
    fn close_requests_wait_for_the_window_then_escalate() {
        let start = Instant::now();
        let mut state = CloseState::default();
        assert_eq!(state.request(false, start), CloseStep::Native);
        assert_eq!(state.request(false, start), CloseStep::InProgress);
        state.pending = None;
        let CloseStep::AskRenderer(id) = state.request(true, start) else {
            panic!("the window is asked first");
        };
        // A quick double click does not bypass the window's save.
        assert_eq!(
            state.request(true, start + Duration::from_millis(200)),
            CloseStep::InProgress
        );
        // An unanswered request is escalated by a later click.
        assert_eq!(
            state.request(true, start + Duration::from_secs(2)),
            CloseStep::Native
        );
        assert_eq!(
            state.request(true, start + Duration::from_secs(3)),
            CloseStep::InProgress
        );
        state.pending = None;
        let CloseStep::AskRenderer(id2) = state.request(true, start) else {
            panic!("a finished attempt starts over");
        };
        assert_ne!(id, id2);
        state.acknowledge(id, false, start);
        assert!(state.unanswered(id2), "stale acknowledgements are ignored");
        state.acknowledge(id2, false, start + Duration::from_millis(10));
        assert!(!state.unanswered(id2));
        // An acknowledged window gets time to save before a click escalates.
        assert_eq!(
            state.request(true, start + Duration::from_secs(1)),
            CloseStep::InProgress
        );
        assert_eq!(
            state.request(true, start + Duration::from_secs(3)),
            CloseStep::Native
        );
        state.pending = None;
        let CloseStep::AskRenderer(id3) = state.request(true, start) else {
            panic!("a new request is sent to the window");
        };
        state.acknowledge(id3, true, start);
        assert!(state.pending.is_none(), "a cancelled close is finished");
    }

    #[test]
    fn an_unanswered_close_times_out_to_native_shutdown() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let shared = Arc::new(Shared {
            published: Mutex::default(),
            overlay: Mutex::default(),
            health: Mutex::new(ControllerHealth::new(ControllerState::Running, None)),
            startup_failed: AtomicBool::new(false),
        });
        let bridge = Bridge::with_channel(sender, shared);
        bridge.frontend_seen.store(true, Ordering::Relaxed);
        let CloseStep::AskRenderer(id) = bridge.request_close() else {
            panic!("the window is asked first");
        };
        assert!(bridge.close_unanswered(id, Duration::from_millis(10)));
        assert_eq!(bridge.request_close(), CloseStep::InProgress);
        bridge.close_finished();
        let CloseStep::AskRenderer(id) = bridge.request_close() else {
            panic!("the window is asked again");
        };
        bridge.acknowledge_close(id, false);
        assert!(!bridge.close_unanswered(id, Duration::from_millis(10)));
    }

    #[test]
    fn abandoned_shutdown_reply_does_not_stop_controller() {
        let (reply, response) = mpsc::channel();
        drop(response);
        assert!(!reply_completed(reply, Ok(()), true));
        let (reply, response) = mpsc::channel();
        assert!(reply_completed(reply, Ok(()), true));
        assert_eq!(response.recv().unwrap(), Ok(()));
    }

    #[test]
    fn saving_progress_delays_close_escalation() {
        let start = Instant::now();
        let mut state = CloseState::default();
        let CloseStep::AskRenderer(id) = state.request(true, start) else {
            panic!("expected renderer");
        };
        state.acknowledge(id, false, start);
        state.acknowledge(id, false, start + Duration::from_secs(5));
        assert_eq!(
            state.request(true, start + Duration::from_secs(6)),
            CloseStep::InProgress
        );
        assert_eq!(
            state.request(true, start + Duration::from_secs(8)),
            CloseStep::Native
        );
    }

    #[test]
    fn panic_messages_are_readable() {
        assert_eq!(
            panic_message(Box::new("synthetic failure")),
            "synthetic failure"
        );
        assert_eq!(
            panic_message(Box::new(String::from("owned failure"))),
            "owned failure"
        );
        assert_eq!(
            panic_message(Box::new(7_u8)),
            "an unexpected internal error"
        );
    }
}
