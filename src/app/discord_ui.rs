use super::*;
use crate::discord::install::{self, Detection, InstallReport, Plan, PluginStatus};
use crate::discord::launch::{self, Flavor};
use crate::discord::{Snapshot, Status};
use std::{
    collections::VecDeque,
    sync::{Arc, atomic::AtomicBool},
};

#[path = "../meetings.rs"]
pub(crate) mod meetings;

#[derive(Default)]
pub(super) struct LaunchState {
    pending: Option<Receiver<Result<LaunchOutcome, String>>>,
    error: Option<String>,
    companion: CompanionState,
    retry: Retry,
    pub(super) automation: AutoCall,
    automatic_status: String,
    automatic_model_attempt: Option<String>,
    installed_flavors: Option<(Instant, Vec<Flavor>)>,
    avatars: Option<crate::discord::avatar::Cache>,
    pub(super) meetings: MeetingGlue,
}

/// What a background relaunch did when it succeeded.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum LaunchOutcome {
    /// Opened with a loopback debugger on a random port.
    Debugger(launch::Launched),
    /// Opened normally, closing any previous debugger access.
    Normal,
}

#[derive(Default)]
struct Retry {
    next: Option<Instant>,
    failures: u32,
}
impl Retry {
    fn due(&self, now: Instant) -> bool {
        self.next.is_none_or(|next| now >= next)
    }
    fn failed(&mut self, now: Instant) {
        self.failures = self.failures.saturating_add(1);
        self.next = Some(now + Duration::from_secs((2_u64 << self.failures.min(4)).min(30)));
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Presence {
    Unknown,
    Out,
    In(String),
}
#[derive(Debug, PartialEq, Eq)]
enum AutoAction {
    Start,
    Stop,
}
#[derive(Default)]
pub(super) struct AutoCall {
    joined: Option<(String, Instant)>,
    left: Option<Instant>,
    unavailable_since: Option<Instant>,
    owned: Option<String>,
    suppressed: Option<String>,
    retry_at: Option<Instant>,
    failed_attempts: u32,
    started: bool,
}
impl AutoCall {
    fn start_failed(&mut self, now: Instant) {
        self.started = false;
        let channel = self.owned.take();
        self.failed_attempts = self.failed_attempts.saturating_add(1);
        self.suppressed = if self.failed_attempts >= 3 {
            channel
        } else {
            None
        };
        self.retry_at = Some(now + Duration::from_secs(2));
    }
    pub(super) fn capture_started(&mut self) {
        if self.owned.is_some() {
            self.started = true;
            self.failed_attempts = 0;
        }
    }
    pub(super) fn capture_failed(&mut self, now: Instant, has_text: bool, stopping: bool) {
        if self.owned.is_some() && !self.started && !has_text && !stopping {
            self.start_failed(now);
        }
    }
    fn retry(&mut self) {
        self.failed_attempts = 0;
        self.suppressed = None;
        self.retry_at = None;
    }
    pub(super) fn manual_finish(&mut self) {
        if let Some(channel) = self.owned.take() {
            self.suppressed = Some(channel);
        }
    }
    fn tick(
        &mut self,
        now: Instant,
        presence: Presence,
        enabled: bool,
        active: bool,
        stopping: bool,
        can_start: bool,
    ) -> Option<AutoAction> {
        if !enabled {
            self.manual_finish();
            self.joined = None;
            self.left = None;
            self.unavailable_since = None;
            return None;
        }
        if presence != Presence::Unknown {
            self.unavailable_since = None;
        }
        if !active && let Some(channel) = self.owned.take() {
            self.suppressed = Some(channel);
        }
        match presence {
            Presence::Unknown => {
                self.joined = None;
                self.left = None;
                // Discord can exit without publishing a leave event. Brief
                // reconnects are tolerated, but an automatic system-audio
                // capture must not continue indefinitely without call presence.
                let since = *self.unavailable_since.get_or_insert(now);
                if active
                    && !stopping
                    && self.owned.is_some()
                    && now.duration_since(since) >= Duration::from_secs(5)
                {
                    return Some(AutoAction::Stop);
                }
            }
            Presence::Out => {
                self.joined = None;
                let since = *self.left.get_or_insert(now);
                if now.duration_since(since) >= Duration::from_secs(2) {
                    self.suppressed = None;
                    self.failed_attempts = 0;
                    if active && !stopping && self.owned.is_some() {
                        return Some(AutoAction::Stop);
                    }
                }
            }
            Presence::In(channel) => {
                if self.joined.as_ref().is_none_or(|(id, _)| id != &channel) {
                    self.joined = Some((channel.clone(), now));
                }
                if active {
                    if self.owned.as_ref().is_some_and(|id| id != &channel) {
                        let since = *self.left.get_or_insert(now);
                        if !stopping && now.duration_since(since) >= Duration::from_secs(2) {
                            return Some(AutoAction::Stop);
                        }
                    } else {
                        self.left = None;
                        // Finishing a manual capture must not immediately start another.
                        if self.owned.is_none() {
                            self.suppressed = Some(channel);
                        }
                    }
                } else {
                    self.left = None;
                    if can_start
                        && self.retry_at.is_none_or(|at| now >= at)
                        && self.suppressed.as_ref() != Some(&channel)
                        && self.joined.as_ref().is_some_and(|(_, at)| {
                            now.duration_since(*at) >= Duration::from_millis(750)
                        })
                    {
                        self.started = false;
                        self.owned = Some(channel.clone());
                        self.suppressed = Some(channel);
                        return Some(AutoAction::Start);
                    }
                }
            }
        }
        None
    }
}

#[derive(Default)]
struct CompanionState {
    checked: bool,
    detection: Option<Detection>,
    plan: Option<Plan>,
    pending: Option<Receiver<CompanionEvent>>,
    progress: String,
    error: Option<String>,
    report: Option<InstallReport>,
    native_enabled: bool,
}

enum CompanionEvent {
    Progress(String),
    Detected(Detection, Result<Option<Plan>, String>),
    Installed(PathBuf, Result<InstallReport, String>),
    InstallerOpened(Result<(), String>),
    NativeState(bool),
}

#[allow(
    dead_code,
    reason = "Companion installation operations retained for Tauri setup"
)]
enum CompanionAction {
    Detect(Option<PathBuf>),
    Install(PathBuf),
    Prepare,
    OpenInstaller(PathBuf),
}

impl App {
    pub(super) fn desktop_discord_status(&self) -> serde_json::Value {
        let snapshot = self
            .discord
            .as_ref()
            .map(|connection| connection.snapshot());
        let (status, connection_error) = match snapshot.as_ref().map(|s| &s.status) {
            Some(Status::Ready) => ("ready", None),
            Some(Status::Connecting) => ("connecting", None),
            Some(Status::Unavailable(error)) => ("unavailable", Some(error.as_str())),
            None => ("disconnected", None),
        };
        let companion = &self.discord_launch.companion;
        let plan = companion.plan.as_ref();
        let observation = snapshot.as_ref().and_then(current_observation);
        let connected = observation.is_some();
        let in_voice = observation.is_some_and(|o| o.channel_id.is_some());
        let audio_ready = self
            .discord
            .as_ref()
            .is_some_and(|c| c.native_audio_ready());
        let runtime_status = companion_runtime_status(
            plan.map(|p| p.status),
            companion.native_enabled,
            connected,
            snapshot
                .as_ref()
                .and_then(|s| s.companion_revision.as_deref()),
            &install::bundled_revision(),
        );
        let audio_status = snapshot
            .as_ref()
            .filter(|_| connected)
            .and_then(|s| s.audio_status.as_deref());
        let participants = observation
            .map(|observation| {
                observation
                    .participants
                    .iter()
                    .map(|participant| {
                        serde_json::json!({"id":participant.id,"name":participant.name,
                            "speaking":participant.speaking,"is_self":participant.is_self,
                            "avatar":participant.avatar.as_ref().filter(|_| self.settings.discord_avatars).map(|avatar| avatar.uri())})
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let protocol = snapshot
            .as_ref()
            .and_then(|s| s.companion_protocol.as_deref())
            .unwrap_or("unknown");
        serde_json::json!({"connected":connected,"listener_started":self.discord.is_some(),"status":status,
            "in_voice":in_voice,"audio_ready":audio_ready,
            "audio_status":audio_status,"audio_message":if audio_ready {None} else {companion_audio_message(audio_status)},
            "participants":participants,
            "pairing_state":if connected {"paired"} else if self.discord.is_some() {"waiting"} else {"automatic"},
            "pairing_key_present":crate::discord::plugin::valid_token(&self.settings.discord_pairing_key),
            "companion_protocol":protocol,
            "companion_protocol_message":companion_protocol_message(protocol),
            "flavor":self.settings.discord_flavor,
            "installed_flavors":self.installed_flavors().iter().map(|flavor| flavor.id()).collect::<Vec<_>>(),
            "debugger_port":self.settings.discord_debugger_port,
            "debugger_open":self.settings.discord_debugger_port != 0,
            "debugger_notice":if self.settings.discord_debugger_port != 0 {Some(DEBUGGER_NOTICE)} else {None},
            "automatic_can_retry":self.settings.discord_auto_transcribe && self.call.is_none() && (self.discord_launch.automation.suppressed.is_some() || self.discord_launch.automation.failed_attempts > 0 || self.discord_launch.error.is_some() || (!self.ready && !self.loading && self.discord_launch.automatic_model_attempt.is_some())),
            "error":self.discord_launch.error.as_deref().or(connection_error),
            "relaunching":self.discord_launch.pending.is_some(),"companion_selected":self.settings.discord_companion,
            "automatic_status":self.discord_launch.automatic_status,
            "meetings":self.desktop_meetings_status(),
            "companion":{"checked":companion.checked,"installed":companion.detection.as_ref().is_some_and(|d|d.installed),
                "runtime_status":runtime_status,
                "source_found":plan.is_some(),"active":companion.detection.as_ref().is_some_and(|d|d.selected_active),
                "busy":companion.pending.is_some(),"status":companion.progress,"error":companion.error,
                "plugin_status":plan.map(|p|match p.status {PluginStatus::Missing=>"missing",PluginStatus::Current=>"current",PluginStatus::UpdateAvailable=>"update_available"}).unwrap_or("unknown"),
                "can_build":plan.is_some_and(|p|p.can_build),"native_audio":companion.native_enabled,
                "installer_ready":plan.is_some_and(|p|p.status==PluginStatus::Current && p.source.join("dist/patcher.js").is_file())}
        })
    }

    pub(super) fn desktop_discord_retry(&mut self) -> Result<(), String> {
        if self.call.is_some() || self.discord_launch.pending.is_some() {
            return Err("Finish the current recording or Discord restart before retrying.".into());
        }
        self.discord_launch.automation.retry();
        self.discord_launch.automatic_model_attempt = None;
        self.discord_launch.retry = Retry::default();
        self.discord_launch.error = None;
        if self.discord.as_ref().is_some_and(|c| !c.is_running())
            && let Some(connection) = self.discord.take()
        {
            connection.disconnect();
        }
        self.connect_discord();
        self.discord_launch.error.clone().map_or(Ok(()), Err)
    }

    pub(super) fn desktop_discord_connect(&mut self, companion: bool) -> Result<(), String> {
        if self.call.is_some() || self.discord_launch.pending.is_some() {
            return Err(
                "Finish the recording or Discord restart before changing the connection.".into(),
            );
        }
        if let Some(connection) = self.discord.take() {
            connection.disconnect();
        }
        self.settings.discord_companion = companion;
        self.settings.discord_auto_connect = true;
        self.connect_discord();
        self.discord_launch.error.clone().map_or(Ok(()), Err)
    }

    pub(super) fn desktop_discord_disconnect(&mut self) -> Result<(), String> {
        if self.call.is_some() {
            return Err("Finish the recording before disconnecting Discord.".into());
        }
        self.settings.discord_auto_connect = false;
        if let Some(connection) = self.discord.take() {
            connection.disconnect();
        }
        self.save_preferences().map_err(|e| e.to_string())
    }

    pub(super) fn desktop_discord_relaunch(&mut self) -> Result<(), String> {
        if self.call.is_some() || self.recording.is_some() || self.busy {
            return Err("Finish recording before restarting Discord.".into());
        }
        if self.settings.discord_companion {
            return Err("The companion does not need debug mode. Restart Discord normally after installing it.".into());
        }
        self.relaunch_discord();
        Ok(())
    }

    /// Closes the debugger that an earlier relaunch opened by restarting the
    /// same Discord edition normally. Offered after capture ends and when the
    /// user switches to the companion.
    #[allow(
        dead_code,
        reason = "Backend operation retained for Tauri command wiring"
    )]
    pub(super) fn desktop_discord_relaunch_normally(&mut self) -> Result<(), String> {
        if self.call.is_some() || self.recording.is_some() || self.busy {
            return Err("Finish recording before restarting Discord.".into());
        }
        if self.discord_launch.pending.is_some() {
            return Err("A Discord restart is already running.".into());
        }
        let flavor = Flavor::parse(&self.settings.discord_flavor).unwrap_or(Flavor::Stable);
        self.discord_launch.error = None;
        if !self.settings.discord_companion {
            if let Some(control) = &self.call {
                control.set_discord(None);
            }
            if let Some(connection) = self.discord.take() {
                connection.disconnect();
            }
        }
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("discord-relaunch".into())
            .spawn(move || {
                let result = launch::relaunch_normally(flavor)
                    .map(|()| LaunchOutcome::Normal)
                    .map_err(|error| format!("{error:#}"));
                let _ = tx.send(result);
            })
            .map_err(|error| format!("Could not start Discord relaunch: {error}"))?;
        self.discord_launch.pending = Some(rx);
        Ok(())
    }

    /// Generates a fresh pairing key, invalidating every companion that holds
    /// the old one. The current companion re-reads the key from local settings.
    #[allow(
        dead_code,
        reason = "Backend operation retained for Tauri command wiring"
    )]
    pub(super) fn desktop_discord_rotate_key(&mut self) -> Result<(), String> {
        if self.call.is_some() {
            return Err("Finish the recording before changing the pairing key.".into());
        }
        let key = crate::discord::plugin::new_token().map_err(|e| e.to_string())?;
        self.replace_pairing(key, self.settings.discord_auto_connect)
    }

    /// Removes the pairing key and stops listening. Connecting again later
    /// pairs with a new key; the old companion key no longer works anywhere.
    #[allow(
        dead_code,
        reason = "Backend operation retained for Tauri command wiring"
    )]
    pub(super) fn desktop_discord_forget_pairing(&mut self) -> Result<(), String> {
        if self.call.is_some() {
            return Err("Finish the recording before forgetting the pairing.".into());
        }
        self.replace_pairing(String::new(), false)
    }

    fn replace_pairing(&mut self, key: String, reconnect: bool) -> Result<(), String> {
        if let Some(connection) = self.discord.take() {
            connection.disconnect();
        }
        if let Some(control) = &self.call {
            control.set_discord(None);
        }
        self.settings.discord_pairing_key = key;
        self.settings.discord_auto_connect = reconnect;
        self.discord_launch.retry = Retry::default();
        self.discord_launch.error = None;
        self.save_preferences().map_err(|e| e.to_string())?;
        if reconnect && self.settings.discord_companion {
            self.connect_discord();
        }
        self.discord_launch.error.clone().map_or(Ok(()), Err)
    }

    fn debugger_port(&self) -> u16 {
        match self.settings.discord_debugger_port {
            0 => crate::discord::DEFAULT_DEBUGGER_PORT,
            port => port,
        }
    }

    fn installed_flavors(&self) -> Vec<Flavor> {
        // Cached by poll; status snapshots must not scan the disk every frame.
        self.discord_launch
            .installed_flavors
            .as_ref()
            .map(|(_, flavors)| flavors.clone())
            .unwrap_or_default()
    }

    fn refresh_installed_flavors(&mut self) {
        let stale = self
            .discord_launch
            .installed_flavors
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(30));
        if stale && (self.settings.discord_standard_configured || !self.settings.discord_companion)
        {
            self.discord_launch.installed_flavors = Some((Instant::now(), launch::installed()));
        }
    }

    pub(super) fn desktop_companion_action(&mut self, action: &str) -> Result<(), String> {
        if self.discord_launch.companion.pending.is_some() {
            return Err("Companion setup is already running.".into());
        }
        if action != "detect" && (self.call.is_some() || self.recording.is_some() || self.busy) {
            return Err("Finish recording before changing the Discord companion.".into());
        }
        let action = match action {
            "detect" => CompanionAction::Detect(
                (!self.settings.vencord_source.trim().is_empty())
                    .then(|| PathBuf::from(&self.settings.vencord_source)),
            ),
            "install" => {
                self.settings.discord_companion = true;
                self.save_preferences().map_err(|e| e.to_string())?;
                if let Some(plan) = &self.discord_launch.companion.plan {
                    CompanionAction::Install(plan.source.clone())
                } else {
                    CompanionAction::Prepare
                }
            }
            "open_installer" => CompanionAction::OpenInstaller(
                self.discord_launch
                    .companion
                    .plan
                    .as_ref()
                    .ok_or("Build the companion before opening the installer.")?
                    .source
                    .clone(),
            ),
            _ => return Err("Unknown companion action.".into()),
        };
        self.companion_work(action);
        Ok(())
    }

    fn connect_discord(&mut self) {
        if self.discord.is_some() {
            return;
        }
        self.discord_launch.error = None;
        let connection = if self.settings.discord_companion {
            let result = (|| -> anyhow::Result<_> {
                if !crate::discord::plugin::valid_token(&self.settings.discord_pairing_key) {
                    self.settings.discord_pairing_key = crate::discord::plugin::new_token()?;
                }
                self.save_preferences()?;
                crate::discord::plugin::start(self.settings.discord_pairing_key.clone())
            })();
            match result {
                Ok(connection) => connection,
                Err(error) => {
                    self.discord_launch.retry.failed(Instant::now());
                    self.discord_launch.error =
                        Some(format!("Could not connect the companion: {error:#}"));
                    return;
                }
            }
        } else {
            self.settings.discord_standard_configured = true;
            self.save();
            crate::discord::Connection::start(self.debugger_port())
        };
        self.discord_launch.retry = Retry::default();
        self.discord = Some(connection);
        if let Some(control) = &self.call {
            control.set_discord(self.discord.clone());
        }
    }

    pub(super) fn poll_discord_launch(&mut self) {
        self.poll_companion_install();
        self.companion_auto_update();
        self.refresh_installed_flavors();
        self.poll_discord_connection();
        self.poll_avatars();
        self.poll_automatic_call();
        self.poll_meetings();
        let Some(receiver) = &self.discord_launch.pending else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Discord relaunch stopped unexpectedly. Try again.".into())
            }
        };
        self.discord_launch.pending = None;
        match result {
            Ok(LaunchOutcome::Debugger(launched)) => {
                self.settings.discord_companion = false;
                self.settings.discord_flavor = launched.flavor.id().into();
                self.settings.discord_debugger_port = launched.port;
                self.save();
                self.connect_discord();
                self.call_status = format!(
                    "{} opened with a temporary debugger. Connecting to speaker names…",
                    launched.flavor.label()
                );
            }
            Ok(LaunchOutcome::Normal) => {
                self.settings.discord_debugger_port = 0;
                self.save();
                self.call_status = "Discord reopened without debugger access.".into();
            }
            Err(error) => self.discord_launch.error = Some(error),
        }
    }

    fn poll_discord_connection(&mut self) {
        if self
            .discord
            .as_ref()
            .is_some_and(|connection| !connection.is_running())
        {
            if let Some(connection) = self.discord.take() {
                connection.disconnect();
            }
            if let Some(control) = &self.call {
                control.set_discord(None);
            }
            self.discord_launch.retry.failed(Instant::now());
        }
        // A healthy listener keeps waiting through Discord restarts and brief stale
        // metadata. Rebinding it would interrupt native capture and pairing.
        let configured = if self.settings.discord_companion {
            crate::discord::plugin::valid_token(&self.settings.discord_pairing_key)
                || !self.settings.vencord_source.is_empty()
        } else {
            self.settings.discord_standard_configured
        };
        if self.settings.discord_auto_connect
            && configured
            && self.discord.is_none()
            && self.discord_launch.pending.is_none()
            && self.discord_launch.retry.due(Instant::now())
        {
            self.connect_discord();
        }
    }

    fn poll_automatic_call(&mut self) {
        if self.call.is_some() && self.is_note_capture() {
            return;
        }
        let snapshot = self
            .discord
            .as_ref()
            .map(|connection| connection.snapshot());
        let presence = snapshot
            .as_ref()
            .and_then(current_observation)
            .map(|observation| {
                observation
                    .channel_id
                    .clone()
                    .map(Presence::In)
                    .unwrap_or(Presence::Out)
            })
            .unwrap_or(Presence::Unknown);
        let native_ready = self
            .discord
            .as_ref()
            .is_some_and(|connection| connection.native_audio_ready());
        let idle = self.call.is_none()
            && self.recording.is_none()
            && !self.busy
            && !self.preview_inflight
            && !self.loading
            && self.downloading.is_none()
            && !self.integration_inflight
            && self.integration_pending.is_none()
            && self.pending_final.is_none()
            && self.pending_install.is_none();
        if self.ready {
            self.discord_launch.automatic_model_attempt = None;
        }
        if self.settings.discord_auto_transcribe
            && matches!(presence, Presence::In(_))
            && idle
            && !self.ready
            && PathBuf::from(&self.settings.model_path).is_file()
            && self.discord_launch.automatic_model_attempt.as_ref()
                != Some(&self.settings.model_path)
        {
            self.discord_launch.automatic_model_attempt = Some(self.settings.model_path.clone());
            self.load();
        }
        self.discord_launch.automatic_status = if !self.settings.discord_auto_transcribe {
            String::new()
        } else if self.call.is_some() {
            if self.discord_launch.automation.owned.is_some() {
                "Automatic capture is running."
            } else {
                "This capture is controlled manually."
            }
            .into()
        } else if matches!(presence, Presence::Unknown) {
            if let Some(error) = &self.discord_launch.error {
                format!("Automatic capture is waiting for Discord: {error}")
            } else if self.discord.is_none() {
                "Connect Discord to start calls automatically.".into()
            } else if self.settings.discord_companion {
                "Waiting for the companion to pair and report your voice channel. Update the companion and restart Discord if this continues.".into()
            } else {
                "Waiting for Discord to report your voice channel.".into()
            }
        } else if matches!(presence, Presence::Out) {
            "Waiting for you to join a Discord voice channel.".into()
        } else if self.loading {
            "Loading the speech model for automatic capture…".into()
        } else if !self.ready {
            if PathBuf::from(&self.settings.model_path).is_file() {
                format!("Speech model is not ready. {}", self.status)
            } else {
                "Choose and download a speech model in Settings to start automatically.".into()
            }
        } else if self.settings.discord_companion && !native_ready {
            companion_audio_message(snapshot.as_ref().and_then(|s| s.audio_status.as_deref())).unwrap_or("Speaker names are connected. Waiting for the participant audio adapter. Update the companion and restart Discord if this continues.").into()
        } else if self
            .discord_launch
            .automation
            .suppressed
            .as_ref()
            .is_some_and(|id| matches!(&presence, Presence::In(channel) if channel == id))
        {
            "Automatic capture is paused for this voice channel. Retry to start again.".into()
        } else if !idle {
            "Waiting for the current dictation or task to finish.".into()
        } else {
            "Voice channel detected. Starting automatic capture…".into()
        };
        let action = self.discord_launch.automation.tick(
            Instant::now(),
            presence.clone(),
            self.settings.discord_auto_transcribe,
            self.call.is_some(),
            self.call
                .as_ref()
                .is_some_and(|control| control.stop_ns.load(Ordering::SeqCst) != 0),
            idle && self.ready && (!self.settings.discord_companion || native_ready),
        );
        match action {
            Some(AutoAction::Start) => {
                if self.start_call_capture() {
                    self.call_status = "Joined Discord. Starting your call transcript…".into();
                } else {
                    self.discord_launch.automation.start_failed(Instant::now());
                    self.discord_launch.automatic_status = self.call_status.clone();
                }
            }
            Some(AutoAction::Stop) => {
                if let Some(control) = &self.call {
                    control.stop();
                }
                self.call_status = "Left the voice channel. Saving your call transcript…".into();
            }
            None => {}
        }
    }

    #[allow(
        dead_code,
        reason = "Discord setup and recovery operations retained for the Tauri integration"
    )]
    fn relaunch_discord(&mut self) {
        if self.discord_launch.pending.is_some() {
            return;
        }
        self.discord_launch.error = None;
        self.settings.discord_companion = false;
        self.save();
        if let Some(control) = &self.call {
            control.set_discord(None);
        }
        if let Some(connection) = self.discord.take() {
            connection.disconnect();
        }
        let (tx, rx) = mpsc::channel();
        let requested = Flavor::parse(&self.settings.discord_flavor);
        match std::thread::Builder::new()
            .name("discord-relaunch".into())
            .spawn(move || {
                let result = launch::relaunch(requested)
                    .map(LaunchOutcome::Debugger)
                    .map_err(|error| format!("{error:#}"));
                let _ = tx.send(result);
            }) {
            Ok(_) => self.discord_launch.pending = Some(rx),
            Err(error) => {
                self.discord_launch.error =
                    Some(format!("Could not start Discord relaunch: {error}"))
            }
        }
    }

    fn companion_work(&mut self, action: CompanionAction) {
        if self.discord_launch.companion.pending.is_some() {
            return;
        }
        self.discord_launch.companion.error = None;
        self.discord_launch.companion.progress = "Checking Vencord…".into();
        let (tx, rx) = mpsc::channel();

        let result = std::thread::Builder::new()
            .name("vencord-companion-install".into())
            .spawn(move || {
                let progress = |message: &str| {
                    let _ = tx.send(CompanionEvent::Progress(message.into()));
                };
                match action {
                    CompanionAction::Detect(hint) => {
                        let detection = install::detect(hint.as_deref());
                        let _ = tx.send(CompanionEvent::NativeState(
                            detection
                                .source
                                .as_deref()
                                .is_some_and(install::native_audio_enabled),
                        ));
                        let plan = detection
                            .source
                            .as_deref()
                            .map(install::plan)
                            .transpose()
                            .map_err(|error| format!("{error:#}"));
                        let _ = tx.send(CompanionEvent::Detected(detection, plan));
                    }
                    CompanionAction::Install(source) => {
                        let result = (|| -> anyhow::Result<_> {
                            let plan = install::plan(&source)?;
                            install::install(&plan, true, progress)
                        })()
                        .map_err(|error| format!("{error:#}"));
                        let _ = tx.send(CompanionEvent::NativeState(
                            install::native_audio_enabled(&source),
                        ));
                        let _ = tx.send(CompanionEvent::Installed(source, result));
                    }
                    CompanionAction::Prepare => {
                        let result = (|| -> anyhow::Result<_> {
                            let plan = install::prepare_managed(progress)?;
                            let source = plan.source.clone();
                            let report = install::install(&plan, true, progress)?;
                            Ok((source, report))
                        })();
                        match result {
                            Ok((source, report)) => {
                                let _ = tx.send(CompanionEvent::NativeState(
                                    install::native_audio_enabled(&source),
                                ));
                                let _ = tx.send(CompanionEvent::Installed(source, Ok(report)));
                            }
                            Err(error) => {
                                let _ = tx.send(CompanionEvent::Installed(
                                    PathBuf::new(),
                                    Err(format!("{error:#}")),
                                ));
                            }
                        }
                    }
                    CompanionAction::OpenInstaller(source) => {
                        let result = install::open_installer(&source, progress)
                            .map_err(|error| format!("{error:#}"));
                        let _ = tx.send(CompanionEvent::InstallerOpened(result));
                    }
                }
            });
        match result {
            Ok(_) => self.discord_launch.companion.pending = Some(rx),
            Err(error) => {
                self.discord_launch.companion.error =
                    Some(format!("Could not start companion setup: {error}"))
            }
        }
    }

    fn poll_companion_install(&mut self) {
        let Some(receiver) = &self.discord_launch.companion.pending else {
            return;
        };
        let mut events = Vec::new();
        loop {
            match receiver.try_recv() {
                Ok(event) => events.push(event),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if !events.iter().any(|event| {
                        !matches!(
                            event,
                            CompanionEvent::Progress(_) | CompanionEvent::NativeState(_)
                        )
                    }) {
                        self.discord_launch.companion.pending = None;
                        self.discord_launch.companion.error =
                            Some("Companion setup stopped unexpectedly. Try again.".into());
                    }
                    break;
                }
            }
        }
        for event in events {
            match event {
                CompanionEvent::NativeState(enabled) => {
                    self.discord_launch.companion.native_enabled = enabled
                }
                CompanionEvent::Progress(message) => {
                    self.discord_launch.companion.progress = message
                }
                CompanionEvent::Detected(detection, plan) => {
                    self.discord_launch.companion.pending = None;
                    self.discord_launch.companion.checked = true;
                    self.discord_launch.companion.detection = Some(detection);
                    self.discord_launch.companion.progress.clear();
                    match plan {
                        Ok(plan) => self.discord_launch.companion.plan = plan,
                        Err(error) => {
                            self.discord_launch.companion.plan = None;
                            self.discord_launch.companion.error = Some(error);
                        }
                    }
                }
                CompanionEvent::Installed(source, result) => {
                    self.discord_launch.companion.pending = None;
                    match result {
                        Ok(report) => {
                            self.settings.vencord_source = source.to_string_lossy().into_owned();
                            self.discord_launch.companion.plan = install::plan(&source).ok();
                            self.discord_launch.companion.report = Some(report);
                            self.discord_launch.companion.progress="Companion built. If you already use this custom build, restart Discord. Otherwise, install it below first.".into();
                            self.save();
                            if self.settings.discord_auto_connect && self.discord.is_none() {
                                self.connect_discord();
                            }
                        }
                        Err(error) => self.discord_launch.companion.error = Some(error),
                    }
                }
                CompanionEvent::InstallerOpened(result) => {
                    self.discord_launch.companion.pending = None;
                    match result {
                    Ok(())=>self.discord_launch.companion.progress="Finish setup in the Vencord installer, then restart Discord and connect here.".into(),
                    Err(error)=>self.discord_launch.companion.error=Some(error),
                }
                }
            }
        }
    }

    fn companion_auto_update(&mut self) {
        if self.discord_launch.companion.pending.is_some()
            || self.discord_launch.companion.error.is_some()
        {
            return;
        }
        if !self.discord_launch.companion.checked {
            let source = self.settings.vencord_source.trim();
            if source.is_empty() && !self.settings.discord_companion {
                return;
            }
            self.discord_launch.companion.checked = true;
            self.companion_work(CompanionAction::Detect(
                (!source.is_empty()).then(|| PathBuf::from(source)),
            ));
        } else if self.settings.vencord_auto_update
            && !self.settings.vencord_source.trim().is_empty()
            && let Some(plan) = &self.discord_launch.companion.plan
            && plan.status == PluginStatus::UpdateAvailable
            && plan.can_build
        {
            self.companion_work(CompanionAction::Install(plan.source.clone()));
        }
    }
}

impl App {
    /// Serves a cached participant picture for a `bytes://discord-avatar/...`
    /// identifier. Returns `None` until the picture has been fetched locally.
    #[allow(
        dead_code,
        reason = "Backend operation retained for Tauri command wiring"
    )]
    pub(super) fn desktop_discord_avatar(&mut self, uri: &str) -> Option<Vec<u8>> {
        if !self.settings.discord_avatars {
            return None;
        }
        self.discord_launch
            .avatars
            .get_or_insert_with(Default::default)
            .bytes_for_uri(uri)
            .map(|bytes| bytes.to_vec())
    }

    fn poll_avatars(&mut self) {
        if !self.settings.discord_avatars {
            return;
        }
        let Some(snapshot) = self
            .discord
            .as_ref()
            .map(|connection| connection.snapshot())
        else {
            return;
        };
        let Some(observation) = current_observation(&snapshot) else {
            return;
        };
        let avatars: Vec<_> = observation
            .participants
            .iter()
            .filter_map(|participant| participant.avatar.clone())
            .collect();
        if avatars.is_empty() {
            return;
        }
        let cache = self
            .discord_launch
            .avatars
            .get_or_insert_with(Default::default);
        for avatar in &avatars {
            cache.request(avatar);
        }
    }

    /// Mirrors Discord's automatic transcripts for detected Teams, Zoom and
    /// Meet meetings. Capture always starts through `start_stream_capture`.
    fn poll_meetings(&mut self) {
        let settings = self.settings.meetings.clone();
        let now = Instant::now();
        let events = self.discord_launch.meetings.advance(now, &settings);
        for event in events {
            match event {
                meetings::Event::Started { app, .. } => match settings.start {
                    meetings::StartMode::Automatic => {
                        self.discord_launch.meetings.pending_start = Some(app)
                    }
                    meetings::StartMode::Ask => self.discord_launch.meetings.prompt = Some(app),
                },
                meetings::Event::Ended { app } => {
                    let glue = &mut self.discord_launch.meetings;
                    if glue.prompt == Some(app) {
                        glue.prompt = None;
                    }
                    if glue.pending_start == Some(app) {
                        glue.pending_start = None;
                    }
                    if glue.suppressed == Some(app) {
                        glue.suppressed = None;
                    }
                    if glue.owned == Some(app)
                        && let Some(control) = &self.call
                    {
                        control.stop();
                        self.call_status =
                            format!("{} meeting ended. Saving your transcript…", app.label());
                    }
                }
            }
        }
        // A capture the user finished stays finished until the meeting ends.
        if self.call.is_none()
            && let Some(app) = self.discord_launch.meetings.owned.take()
        {
            self.discord_launch.meetings.suppressed = Some(app);
        }
        let Some(app) = self.discord_launch.meetings.pending_start else {
            self.discord_launch.meetings.status = self
                .discord_launch
                .meetings
                .describe(&settings, self.call.is_some());
            return;
        };
        if self.discord_launch.meetings.suppressed == Some(app)
            || !self
                .discord_launch
                .meetings
                .detector
                .active()
                .contains(&app)
        {
            self.discord_launch.meetings.pending_start = None;
            return;
        }
        let idle = self.call.is_none()
            && self.recording.is_none()
            && !self.busy
            && !self.preview_inflight
            && !self.loading
            && self.downloading.is_none()
            && !self.integration_inflight
            && self.integration_pending.is_none()
            && self.pending_final.is_none()
            && self.pending_install.is_none();
        if !idle {
            self.discord_launch.meetings.status =
                "Waiting for the current dictation or task to finish.".into();
            return;
        }
        if !self.ready {
            if PathBuf::from(&self.settings.model_path).is_file() {
                if self.discord_launch.meetings.model_attempt.as_ref()
                    != Some(&self.settings.model_path)
                {
                    self.discord_launch.meetings.model_attempt =
                        Some(self.settings.model_path.clone());
                    self.load();
                }
                self.discord_launch.meetings.status =
                    "Loading the speech model for the meeting transcript…".into();
            } else {
                self.discord_launch.meetings.status =
                    "Choose and download a speech model in Settings to transcribe meetings.".into();
            }
            return;
        }
        self.discord_launch.meetings.pending_start = None;
        let preference = (!self.settings.transcription_language.is_empty())
            .then(|| self.settings.transcription_language.clone());
        // Mixed microphone and output capture without Discord participant audio.
        if self.start_stream_capture(false, false, false, preference, None) {
            self.discord_launch.meetings.owned = Some(app);
            self.call_status = format!(
                "{} meeting detected. Starting your transcript… {}",
                app.label(),
                meetings::CONSENT_REMINDER
            );
        } else {
            self.discord_launch.meetings.suppressed = Some(app);
            self.discord_launch.meetings.status = self.call_status.clone();
        }
    }

    /// Answers the "start a transcript?" prompt for a detected meeting.
    #[allow(
        dead_code,
        reason = "Backend operation retained for Tauri command wiring"
    )]
    pub(super) fn desktop_meeting_respond(&mut self, accept: bool) -> Result<(), String> {
        let glue = &mut self.discord_launch.meetings;
        let app = glue
            .prompt
            .take()
            .ok_or("No meeting is waiting for an answer.")?;
        if accept {
            glue.pending_start = Some(app);
        } else {
            glue.suppressed = Some(app);
        }
        Ok(())
    }

    fn desktop_meetings_status(&self) -> serde_json::Value {
        let glue = &self.discord_launch.meetings;
        let app_json = |app: meetings::App| serde_json::json!({"id": app.id(), "label": app.label(), "best_effort": app.best_effort()});
        serde_json::json!({
            "enabled": self.settings.meetings.any(),
            "settings": self.settings.meetings,
            "consent_reminder": meetings::CONSENT_REMINDER,
            "active": glue.detector.active().into_iter().map(app_json).collect::<Vec<_>>(),
            "prompt": glue.prompt.map(app_json),
            "owned": glue.owned.map(app_json),
            "status": glue.status,
            "probe_error": glue.probe_error,
            "events": glue.events.iter().map(|(event, at)| serde_json::json!({
                "app": event.app().id(), "label": event.app().label(), "kind": event.kind(),
                "at_unix_ms": at.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
            })).collect::<Vec<_>>(),
        })
    }
}

/// Application-side state for meeting detection: the background probe, the
/// debounced detector and which capture, if any, this feature owns.
#[derive(Default)]
pub(super) struct MeetingGlue {
    detector: meetings::Detector,
    probe: Option<(
        Receiver<Result<Vec<meetings::Evidence>, String>>,
        Arc<AtomicBool>,
    )>,
    probe_error: Option<String>,
    latest: Option<Vec<meetings::Evidence>>,
    pending_start: Option<meetings::App>,
    prompt: Option<meetings::App>,
    owned: Option<meetings::App>,
    suppressed: Option<meetings::App>,
    model_attempt: Option<String>,
    status: String,
    events: VecDeque<(meetings::Event, std::time::SystemTime)>,
}

impl MeetingGlue {
    /// Starts or stops the probe thread to match the settings, drains its
    /// results and runs the detector. Returns confirmed events.
    fn advance(&mut self, now: Instant, settings: &meetings::Settings) -> Vec<meetings::Event> {
        if settings.any() {
            if self
                .probe
                .as_ref()
                .is_none_or(|(_, stop)| stop.load(Ordering::Acquire))
            {
                self.start_probe();
            }
        } else if let Some((_, stop)) = self.probe.take() {
            stop.store(true, Ordering::Release);
            self.latest = None;
            self.pending_start = None;
            self.prompt = None;
        }
        if let Some((receiver, stop)) = &self.probe {
            loop {
                match receiver.try_recv() {
                    Ok(Ok(evidence)) => {
                        self.probe_error = None;
                        self.latest = Some(evidence);
                    }
                    Ok(Err(error)) => self.probe_error = Some(error),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        stop.store(true, Ordering::Release);
                        break;
                    }
                }
            }
        }
        let evidence = self.latest.clone().unwrap_or_default();
        let events = self.detector.tick(now, &evidence, settings);
        for event in &events {
            self.events
                .push_back((event.clone(), std::time::SystemTime::now()));
            while self.events.len() > 16 {
                self.events.pop_front();
            }
        }
        events
    }

    fn start_probe(&mut self) {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let worker_stop = stop.clone();
        let spawned = std::thread::Builder::new()
            .name("meeting-probe".into())
            .spawn(move || {
                while !worker_stop.load(Ordering::Acquire) {
                    let result = meetings::probe().map_err(|error| format!("{error:#}"));
                    if tx.send(result).is_err() {
                        break;
                    }
                    for _ in 0..10 {
                        if worker_stop.load(Ordering::Acquire) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            });
        match spawned {
            Ok(_) => self.probe = Some((rx, stop)),
            Err(error) => {
                self.probe_error = Some(format!("Could not start meeting detection: {error}"))
            }
        }
    }

    fn describe(&self, settings: &meetings::Settings, capturing: bool) -> String {
        if !settings.any() {
            String::new()
        } else if let Some(app) = self.owned.filter(|_| capturing) {
            format!("Transcribing the {} meeting.", app.label())
        } else if capturing {
            "This capture is controlled manually.".into()
        } else if let Some(app) = self.prompt {
            format!(
                "{} meeting detected. Choose whether to start a transcript.",
                app.label()
            )
        } else if let Some(app) = self.suppressed {
            format!("Transcription is paused for this {} meeting.", app.label())
        } else if let Some(error) = &self.probe_error {
            format!("Meeting detection is unavailable: {error}")
        } else {
            "Watching for a meeting.".into()
        }
    }
}

impl Drop for MeetingGlue {
    fn drop(&mut self) {
        if let Some((_, stop)) = &self.probe {
            stop.store(true, Ordering::Release);
        }
    }
}

/// Shown while a relaunched Discord still exposes its debugger.
pub(super) const DEBUGGER_NOTICE: &str = "Discord is running with its local debugger open. Any program on this computer can read or control that Discord window until you restart Discord without it.";

fn companion_protocol_message(protocol: &str) -> Option<&'static str> {
    match protocol {
        "outdated" => Some(
            "The Discord companion is older than this Articulate version. Update the companion from Settings, then restart Discord.",
        ),
        "newer" => Some(
            "The Discord companion is newer than this Articulate version. Update Articulate to connect.",
        ),
        _ => None,
    }
}

fn companion_audio_message(status: Option<&str>) -> Option<&'static str> {
    match status? {
        "preload-unavailable" | "addon-unavailable" => Some(
            "Discord has loaded speaker names without the audio adapter. Repair the companion, then quit Discord completely and reopen it.",
        ),
        "unsupported-native-build" => Some(
            "This Discord voice version is not supported by the audio adapter. Check for an Articulate update.",
        ),
        "native-hook-unavailable" => Some(
            "The audio adapter could not attach to Discord. Quit Discord completely and reopen it. Repair the companion if this continues.",
        ),
        "waiting-for-voice-engine" => {
            Some("Waiting for Discord to initialize voice audio. Join a voice channel to continue.")
        }
        "waiting-for-articulate" | "control-unavailable" => Some(
            "The audio adapter is loaded but cannot reach Articulate. Reconnect Discord in these settings.",
        ),
        "audio-transport-unavailable" => Some(
            "The separate audio connection was interrupted. Reconnect Discord before starting another capture.",
        ),
        "disabled" => Some(
            "The audio adapter has not been enabled by the companion. Repair the companion and restart Discord if this continues.",
        ),
        _ => None,
    }
}

fn companion_runtime_status(
    build: Option<PluginStatus>,
    native: bool,
    connected: bool,
    revision: Option<&str>,
    expected: &str,
) -> &'static str {
    if build.is_some_and(|status| status != PluginStatus::Current)
        || (build == Some(PluginStatus::Current) && !native)
    {
        "update_required"
    } else if connected && revision == Some(expected) {
        "current"
    } else if build == Some(PluginStatus::Current) && native {
        if connected {
            "restart_required"
        } else {
            "offline"
        }
    } else {
        "unverified"
    }
}

fn current_observation(snapshot: &Snapshot) -> Option<&crate::discord::Observation> {
    snapshot.observation.as_ref().filter(|observation| {
        matches!(snapshot.status, Status::Ready)
            && observation.valid
            && observation.at.elapsed() < Duration::from_secs(1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_files_never_prove_the_running_companion_is_current() {
        let expected = "a".repeat(64);
        let old = "b".repeat(64);
        let current = Some(PluginStatus::Current);
        assert_eq!(
            companion_runtime_status(current, true, true, Some(&expected), &expected),
            "current"
        );
        assert_eq!(
            companion_runtime_status(current, true, true, Some(&old), &expected),
            "restart_required"
        );
        assert_eq!(
            companion_runtime_status(current, true, true, None, &expected),
            "restart_required"
        );
        assert_eq!(
            companion_runtime_status(current, true, false, Some(&expected), &expected),
            "offline"
        );
        assert_eq!(
            companion_runtime_status(current, false, true, Some(&expected), &expected),
            "update_required"
        );
        assert_eq!(
            companion_runtime_status(
                Some(PluginStatus::UpdateAvailable),
                true,
                true,
                Some(&expected),
                &expected
            ),
            "update_required"
        );
        assert_eq!(
            companion_runtime_status(None, false, false, None, &expected),
            "unverified"
        );
    }

    #[test]
    fn connection_readiness_requires_fresh_confirmed_membership() {
        let mut snapshot = Snapshot {
            audio_status: None,
            companion_revision: None,
            companion_protocol: None,
            status: Status::Ready,
            observation: Some(crate::discord::Observation {
                at: Instant::now(),
                generation: 1,
                channel_id: Some("123".into()),
                participants: Vec::new(),
                valid: true,
            }),
        };
        assert!(current_observation(&snapshot).is_some());
        snapshot.observation.as_mut().unwrap().at = Instant::now() - Duration::from_secs(2);
        assert!(current_observation(&snapshot).is_none());
        snapshot.observation.as_mut().unwrap().at = Instant::now();
        snapshot.observation.as_mut().unwrap().valid = false;
        assert!(current_observation(&snapshot).is_none());
        snapshot.observation.as_mut().unwrap().valid = true;
        snapshot.status = Status::Connecting;
        assert!(current_observation(&snapshot).is_none());
    }

    #[test]
    fn automatic_status_explains_missing_connection_and_exposes_paused_retry() {
        let (mut app, _) = super::super::tests::app();
        app.settings.discord_auto_transcribe = true;
        app.poll_automatic_call();
        let state = app.desktop_discord_status();
        assert_eq!(state["connected"], false);
        assert_eq!(state["listener_started"], false);
        assert_eq!(state["pairing_state"], "automatic");
        assert_eq!(state["automatic_can_retry"], false);
        assert!(
            state["automatic_status"]
                .as_str()
                .unwrap()
                .starts_with("Connect Discord")
        );
        app.discord_launch.error = Some("Synthetic listener failure".into());
        app.poll_automatic_call();
        assert!(
            app.discord_launch
                .automatic_status
                .contains("Synthetic listener failure")
        );
        assert_eq!(app.desktop_discord_status()["automatic_can_retry"], true);
        app.discord_launch.error = None;
        app.discord_launch.automation.suppressed = Some("123".into());
        assert_eq!(app.desktop_discord_status()["automatic_can_retry"], true);
        app.discord_launch.automation.retry();
        assert_eq!(app.desktop_discord_status()["automatic_can_retry"], false);
        app.call = Some(call_capture::Control::new());
        assert!(app.desktop_discord_retry().is_err());
    }

    #[test]
    fn automatic_calls_debounce_join_leave_and_wait_for_audio_readiness() {
        let mut state = AutoCall::default();
        let at = Instant::now();
        let inside = || Presence::In("123".into());
        assert_eq!(state.tick(at, inside(), true, false, false, true), None);
        assert_eq!(
            state.tick(
                at + Duration::from_secs(1),
                inside(),
                true,
                false,
                false,
                false
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(2),
                inside(),
                true,
                false,
                false,
                true
            ),
            Some(AutoAction::Start)
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(3),
                Presence::Out,
                true,
                true,
                false,
                false
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(4),
                Presence::Unknown,
                true,
                true,
                false,
                false
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(5),
                inside(),
                true,
                true,
                false,
                false
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(6),
                Presence::Out,
                true,
                true,
                false,
                false
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(8),
                Presence::Out,
                true,
                true,
                false,
                false
            ),
            Some(AutoAction::Stop)
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(9),
                Presence::Out,
                true,
                true,
                true,
                false
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(10),
                Presence::Out,
                true,
                false,
                false,
                true
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(11),
                inside(),
                true,
                false,
                false,
                true
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(12),
                inside(),
                true,
                false,
                false,
                true
            ),
            Some(AutoAction::Start)
        );
    }

    #[test]
    fn manual_calls_and_manual_finish_remain_in_user_control() {
        let mut state = AutoCall::default();
        let at = Instant::now();
        let inside = || Presence::In("123".into());
        assert_eq!(state.tick(at, inside(), true, true, false, false), None);
        assert_eq!(
            state.tick(
                at + Duration::from_secs(1),
                Presence::Out,
                true,
                true,
                false,
                false
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(4),
                Presence::Out,
                true,
                true,
                false,
                false
            ),
            None
        );
        state.tick(
            at + Duration::from_secs(5),
            inside(),
            true,
            true,
            false,
            false,
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(7),
                inside(),
                true,
                false,
                false,
                true
            ),
            None
        );
        let mut auto = AutoCall::default();
        auto.tick(at, inside(), true, false, false, true);
        assert_eq!(
            auto.tick(
                at + Duration::from_secs(1),
                inside(),
                true,
                false,
                false,
                true
            ),
            Some(AutoAction::Start)
        );
        auto.manual_finish();
        assert_eq!(
            auto.tick(
                at + Duration::from_secs(2),
                inside(),
                true,
                false,
                false,
                true
            ),
            None
        );
        assert_eq!(
            auto.tick(
                at + Duration::from_secs(60),
                inside(),
                true,
                false,
                false,
                true
            ),
            None
        );
    }

    #[test]
    fn channel_switch_finishes_owned_capture_before_starting_the_next() {
        let mut state = AutoCall::default();
        let at = Instant::now();
        state.tick(at, Presence::In("123".into()), true, false, false, true);
        state.tick(
            at + Duration::from_secs(1),
            Presence::In("123".into()),
            true,
            false,
            false,
            true,
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(2),
                Presence::In("456".into()),
                true,
                true,
                false,
                false
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(4),
                Presence::In("456".into()),
                true,
                true,
                false,
                false
            ),
            Some(AutoAction::Stop)
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(5),
                Presence::In("456".into()),
                true,
                true,
                true,
                false
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(6),
                Presence::In("456".into()),
                true,
                false,
                false,
                true
            ),
            Some(AutoAction::Start)
        );
    }

    #[test]
    fn unavailable_disabled_and_failed_capture_do_not_create_retry_loops() {
        let mut state = AutoCall::default();
        let at = Instant::now();
        assert_eq!(
            state.tick(at, Presence::In("123".into()), false, false, false, true),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(1),
                Presence::Unknown,
                true,
                false,
                false,
                true
            ),
            None
        );
        state.tick(
            at + Duration::from_secs(2),
            Presence::In("123".into()),
            true,
            false,
            false,
            true,
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(3),
                Presence::In("123".into()),
                true,
                false,
                false,
                true
            ),
            Some(AutoAction::Start)
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(4),
                Presence::In("123".into()),
                true,
                false,
                false,
                true
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(60),
                Presence::In("123".into()),
                true,
                false,
                false,
                true
            ),
            None
        );
        state.owned = Some("123".into());
        assert_eq!(
            state.tick(
                at + Duration::from_secs(61),
                Presence::Out,
                false,
                true,
                false,
                false
            ),
            None
        );
        assert!(
            state.owned.is_none(),
            "Turning automation off leaves the existing call manual"
        );
    }

    #[test]
    fn listener_retries_are_bounded_and_old_settings_never_arm_recording() {
        let mut retry = Retry::default();
        let at = Instant::now();
        assert!(retry.due(at));
        for attempt in 0..20 {
            let now = at + Duration::from_secs(attempt * 31);
            retry.failed(now);
            assert!(!retry.due(now + Duration::from_secs(1)));
            assert!(retry.due(now + Duration::from_secs(30)));
        }
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(settings.discord_auto_connect);
        assert!(!settings.discord_auto_transcribe);
        assert!(!settings.discord_standard_configured);
    }

    #[test]
    fn automatic_companion_updates_require_opt_in_known_source_and_build_readiness() {
        for (enabled, known, status, can_build, error) in [
            (false, true, PluginStatus::UpdateAvailable, true, false),
            (true, false, PluginStatus::UpdateAvailable, true, false),
            (true, true, PluginStatus::Current, true, false),
            (true, true, PluginStatus::Missing, true, false),
            (true, true, PluginStatus::UpdateAvailable, false, false),
            (true, true, PluginStatus::UpdateAvailable, true, true),
        ] {
            let (mut app, _) = super::super::tests::app();
            app.settings.vencord_auto_update = enabled;
            if known {
                app.settings.vencord_source = "synthetic-source".into();
            }
            app.discord_launch.companion.checked = true;
            app.discord_launch.companion.plan = Some(Plan {
                source: "synthetic-source".into(),
                status,
                can_build,
            });
            if error {
                app.discord_launch.companion.error = Some("Previous update failed".into());
            }
            app.companion_auto_update();
            assert!(
                app.discord_launch.companion.pending.is_none(),
                "Unapproved or unready update must not start"
            );
        }
    }

    #[test]
    fn companion_update_is_detected_without_enabling_automatic_installation() {
        let (mut app, _) = super::super::tests::app();
        app.settings.discord_companion = true;
        app.settings.vencord_source = "synthetic-source".into();
        app.settings.vencord_auto_update = false;
        app.companion_auto_update();
        assert!(app.discord_launch.companion.checked);
        assert!(app.discord_launch.companion.pending.is_some());
    }

    #[test]
    fn companion_background_errors_are_reported_without_changing_capture() {
        let (mut app, _) = super::super::tests::app();
        let control = call_capture::Control::new();
        app.call = Some(control.clone());
        let (tx, rx) = mpsc::channel();
        app.discord_launch.companion.pending = Some(rx);
        tx.send(CompanionEvent::Installed(
            PathBuf::new(),
            Err("Synthetic build failure".into()),
        ))
        .unwrap();
        drop(tx);
        app.poll_companion_install();
        assert!(app.discord_launch.companion.pending.is_none());
        assert_eq!(
            app.discord_launch.companion.error.as_deref(),
            Some("Synthetic build failure")
        );
        assert_eq!(control.stop_ns.load(Ordering::SeqCst), 0);
        assert!(app.settings.vencord_source.is_empty());
    }

    #[test]
    fn missing_discord_presence_stops_only_owned_capture_after_a_grace_period() {
        let at = Instant::now();
        let mut state = AutoCall::default();
        let inside = || Presence::In("123".into());
        state.tick(at, inside(), true, false, false, true);
        assert_eq!(
            state.tick(
                at + Duration::from_secs(1),
                inside(),
                true,
                false,
                false,
                true
            ),
            Some(AutoAction::Start)
        );
        state.capture_started();
        assert_eq!(
            state.tick(
                at + Duration::from_secs(2),
                Presence::Unknown,
                true,
                true,
                false,
                false
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(6),
                Presence::Unknown,
                true,
                true,
                false,
                false
            ),
            None
        );
        // A restored connection resets the grace period.
        assert_eq!(
            state.tick(
                at + Duration::from_secs(7),
                inside(),
                true,
                true,
                false,
                false
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(8),
                Presence::Unknown,
                true,
                true,
                false,
                false
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(13),
                Presence::Unknown,
                true,
                true,
                false,
                false
            ),
            Some(AutoAction::Stop)
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(14),
                Presence::Unknown,
                true,
                true,
                true,
                false
            ),
            None
        );
        let mut manual = AutoCall::default();
        manual.tick(at, Presence::Unknown, true, true, false, false);
        assert_eq!(
            manual.tick(
                at + Duration::from_secs(60),
                Presence::Unknown,
                true,
                true,
                false,
                false
            ),
            None
        );
    }

    #[test]
    fn failed_relaunch_preserves_capture_and_allows_retry() {
        let (mut app, _) = super::super::tests::app();
        let control = call_capture::Control::new();
        app.call = Some(control.clone());
        app.text = "Keep the current transcript".into();
        let (tx, rx) = mpsc::channel();
        app.discord_launch.pending = Some(rx);
        app.poll_discord_launch();
        assert!(app.discord_launch.pending.is_some());
        tx.send(Err("Installation not found".into())).unwrap();
        app.poll_discord_launch();
        assert!(app.discord_launch.pending.is_none());
        assert_eq!(
            app.discord_launch.error.as_deref(),
            Some("Installation not found")
        );
        assert!(app.discord.is_none());
        assert_eq!(app.text, "Keep the current transcript");
        assert_eq!(control.stop_ns.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn abandoned_relaunch_worker_is_reported() {
        let (mut app, _) = super::super::tests::app();
        let (tx, rx) = mpsc::channel();
        app.discord_launch.pending = Some(rx);
        drop(tx);
        app.poll_discord_launch();
        assert!(app.discord_launch.pending.is_none());
        assert!(app.discord_launch.error.is_some());
    }

    #[test]
    fn failed_automatic_start_retries_without_rejoining_and_preserves_native_gate() {
        let mut state = AutoCall::default();
        let at = Instant::now();
        let inside = || Presence::In("123".into());
        state.tick(at, inside(), true, false, false, true);
        assert_eq!(
            state.tick(
                at + Duration::from_secs(1),
                inside(),
                true,
                false,
                false,
                true
            ),
            Some(AutoAction::Start)
        );
        state.start_failed(at + Duration::from_secs(1));
        assert_eq!(
            state.tick(
                at + Duration::from_secs(2),
                inside(),
                true,
                false,
                false,
                true
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(3),
                inside(),
                true,
                false,
                false,
                false
            ),
            None
        );
        assert_eq!(
            state.tick(
                at + Duration::from_secs(4),
                inside(),
                true,
                false,
                false,
                true
            ),
            Some(AutoAction::Start)
        );
    }
    #[test]
    fn worker_startup_failure_retries_three_times_without_erasing_manual_or_started_calls() {
        let (mut app, _commands) = super::super::tests::app();
        let at = Instant::now();
        app.discord_launch.automation.tick(
            at,
            Presence::In("123".into()),
            true,
            false,
            false,
            true,
        );
        for attempt in 0..3 {
            let time = at + Duration::from_secs(1 + attempt * 4);
            assert_eq!(
                app.discord_launch.automation.tick(
                    time,
                    Presence::In("123".into()),
                    true,
                    false,
                    false,
                    true
                ),
                Some(AutoAction::Start)
            );
            app.call = Some(call_capture::Control::new());
            app.event_tx
                .send(Event::CallFinished(Err(
                    "Synthetic microphone unavailable".into()
                )))
                .unwrap();
            app.receive();
            assert!(app.call.is_none());
            assert_eq!(
                app.discord_launch.automation.failed_attempts,
                attempt as u32 + 1
            );
        }
        assert_eq!(
            app.discord_launch.automation.tick(
                at + Duration::from_secs(30),
                Presence::In("123".into()),
                true,
                false,
                false,
                true
            ),
            None
        );
        app.discord_launch.automation.retry();
        assert_eq!(
            app.discord_launch.automation.tick(
                at + Duration::from_secs(31),
                Presence::In("123".into()),
                true,
                false,
                false,
                true
            ),
            Some(AutoAction::Start)
        );
        app.call = Some(call_capture::Control::new());
        app.event_tx
            .send(Event::Call(calls::Update::Started))
            .unwrap();
        app.receive();
        assert_eq!(app.discord_launch.automation.failed_attempts, 0);
        app.event_tx
            .send(Event::CallFinished(Err("Synthetic runtime failure".into())))
            .unwrap();
        app.receive();
        assert_eq!(
            app.discord_launch.automation.tick(
                at + Duration::from_secs(35),
                Presence::In("123".into()),
                true,
                false,
                false,
                true
            ),
            None
        );
        let mut manual = AutoCall::default();
        manual.capture_failed(at, false, false);
        assert_eq!(manual.failed_attempts, 0);
        let mut stopping = AutoCall {
            owned: Some("123".into()),
            ..Default::default()
        };
        stopping.capture_failed(at, false, true);
        assert_eq!(stopping.failed_attempts, 0);
        let mut with_text = AutoCall {
            owned: Some("123".into()),
            ..Default::default()
        };
        with_text.capture_failed(at, true, false);
        assert_eq!(with_text.failed_attempts, 0);
    }
    #[test]
    fn explicit_retry_releases_manual_stop_suppression() {
        let mut state = AutoCall::default();
        let at = Instant::now();
        let inside = || Presence::In("123".into());
        state.tick(at, inside(), true, false, false, true);
        state.tick(
            at + Duration::from_secs(1),
            inside(),
            true,
            false,
            false,
            true,
        );
        state.manual_finish();
        assert_eq!(
            state.tick(
                at + Duration::from_secs(2),
                inside(),
                true,
                false,
                false,
                true
            ),
            None
        );
        state.retry();
        assert_eq!(
            state.tick(
                at + Duration::from_secs(3),
                inside(),
                true,
                false,
                false,
                true
            ),
            Some(AutoAction::Start)
        );
    }
    #[test]
    fn repeated_start_failures_pause_until_explicit_retry() {
        let mut state = AutoCall::default();
        let at = Instant::now();
        let inside = || Presence::In("123".into());
        state.tick(at, inside(), true, false, false, true);
        for seconds in [1, 3, 5] {
            let now = at + Duration::from_secs(seconds);
            assert_eq!(
                state.tick(now, inside(), true, false, false, true),
                Some(AutoAction::Start)
            );
            state.start_failed(now);
        }
        assert_eq!(
            state.tick(
                at + Duration::from_secs(20),
                inside(),
                true,
                false,
                false,
                true
            ),
            None
        );
        state.retry();
        assert_eq!(
            state.tick(
                at + Duration::from_secs(21),
                inside(),
                true,
                false,
                false,
                true
            ),
            Some(AutoAction::Start)
        );
    }
}
