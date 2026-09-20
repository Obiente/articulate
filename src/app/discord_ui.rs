use super::*;
use crate::discord::install::{self, Detection, InstallReport, Plan, PluginStatus};
use crate::discord::{Snapshot, Status};
use theme::{LINE, MUTED, SURFACE};

const SETUP_COMMAND: &str = r#"$discordApp = Get-ChildItem -LiteralPath (Join-Path $env:LOCALAPPDATA 'Discord') -Directory -Filter 'app-*' | Where-Object { $_.Name -match '^app-\d+(\.\d+)+$' -and (Test-Path -LiteralPath (Join-Path $_.FullName 'Discord.exe')) } | Sort-Object { [version]$_.Name.Substring(4) } -Descending | Select-Object -First 1
if ($null -eq $discordApp) { throw 'Discord installation not found.' }
& (Join-Path $discordApp.FullName 'Discord.exe') --remote-debugging-port=9222 --remote-debugging-address=127.0.0.1"#;

#[derive(Default)]
pub(super) struct LaunchState {
    pub(super) confirm: bool,
    pending: Option<Receiver<Result<(), String>>>,
    error: Option<String>,
    companion: CompanionState,
    retry: Retry,
    pub(super) automation: AutoCall,
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
    owned: Option<String>,
    suppressed: Option<String>,
}
impl AutoCall {
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
            return None;
        }
        if !active && let Some(channel) = self.owned.take() {
            self.suppressed = Some(channel);
        }
        match presence {
            Presence::Unknown => {
                self.joined = None;
                self.left = None;
            }
            Presence::Out => {
                self.joined = None;
                let since = *self.left.get_or_insert(now);
                if now.duration_since(since) >= Duration::from_secs(2) {
                    self.suppressed = None;
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
                        && self.suppressed.as_ref() != Some(&channel)
                        && self.joined.as_ref().is_some_and(|(_, at)| {
                            now.duration_since(*at) >= Duration::from_millis(750)
                        })
                    {
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

enum CompanionAction {
    Detect(Option<PathBuf>),
    Install(PathBuf),
    Prepare,
    OpenInstaller(PathBuf),
}

impl App {
    #[cfg(test)]
    pub(super) fn prepare_companion_capture(&mut self) {
        self.settings.discord_companion = true;
        self.settings.discord_pairing_key.clear();
        self.settings.vencord_source.clear();
        self.settings.vencord_auto_update = false;
        self.settings.discord_auto_connect = false;
        self.settings.discord_auto_transcribe = false;
        self.discord_launch.companion = CompanionState {
            checked: true,
            detection: Some(Detection::default()),
            ..Default::default()
        };
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
            crate::discord::Connection::start()
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
        self.poll_discord_connection();
        self.poll_automatic_call();
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
            Ok(()) => {
                self.settings.discord_companion = false;
                self.save();
                self.connect_discord();
                self.call_status = "Discord opened. Connecting to speaker names…".into();
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
        let action = self.discord_launch.automation.tick(
            Instant::now(),
            presence,
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

    fn relaunch_discord(&mut self, ctx: &egui::Context) {
        if self.discord_launch.pending.is_some() {
            return;
        }
        self.discord_launch.confirm = false;
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
        let ctx = ctx.clone();
        match std::thread::Builder::new()
            .name("discord-relaunch".into())
            .spawn(move || {
                let result =
                    crate::discord::launch::relaunch().map_err(|error| format!("{error:#}"));
                let _ = tx.send(result);
                ctx.request_repaint();
            }) {
            Ok(_) => self.discord_launch.pending = Some(rx),
            Err(error) => {
                self.discord_launch.error =
                    Some(format!("Could not start Discord relaunch: {error}"))
            }
        }
    }

    pub(super) fn discord_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let snapshot = self
            .discord
            .as_ref()
            .map(|connection| connection.snapshot());
        if snapshot.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.discord_card(ui, ctx, snapshot.as_ref());
    }

    pub(super) fn discord_card(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        snapshot: Option<&Snapshot>,
    ) {
        egui::Frame::new()
            .fill(SURFACE)
            .stroke(egui::Stroke::new(1.0_f32, LINE))
            .corner_radius(18)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_min_width((ui.available_width() - 1.0).max(0.0));
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Discord speakers").strong().size(18.0));
                    ui.add_space(8.0);
                    let (label, active) = connection_label(snapshot);
                    ui.label(RichText::new(label).small().color(if active { ACCENT } else { MUTED }));
                    ui.add_space(8.0);
                    let connected = snapshot.is_some();
                    if ui.add_enabled(self.discord_launch.pending.is_none(),
                        egui::Button::new(if connected { "Disconnect" } else { "Connect" })
                            .corner_radius(14),
                    ).clicked()
                    {
                        if let Some(connection) = self.discord.take() {
                            self.settings.discord_auto_connect = false;
                            if self.discord_launch.automation.owned.is_some() {
                                if let Some(control) = &self.call { control.stop(); }
                                self.discord_launch.automation.manual_finish();
                                self.call_status = "Saving your automatic call transcript…".into();
                            }
                            self.save();
                            if let Some(control) = &self.call {
                                control.set_discord(None);
                            }
                            connection.disconnect();
                        } else {
                            self.settings.discord_auto_connect = true;
                            self.discord_launch.retry = Retry::default();
                            self.save();
                            self.connect_discord();
                            ctx.request_repaint();
                        }
                    }
                });
                if let Some(observation) = snapshot.and_then(current_observation)
                    && observation.channel_id.is_some()
                    && !observation.participants.is_empty()
                {
                        ui.add_space(10.0);
                        ui.horizontal_wrapped(|ui| {
                            for participant in &observation.participants {
                                let speaking = participant.speaking;
                                let color = if speaking { ACCENT } else { MUTED };
                                let name = if participant.is_self { "You" } else { &participant.name };
                                egui::Frame::new()
                                    .fill(if speaking { Color32::from_rgb(35, 63, 56) } else { Color32::from_rgb(28, 38, 40) })
                                    .corner_radius(12)
                                    .inner_margin(egui::Margin::symmetric(10, 5))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            let (rect, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
                                            ui.painter().circle_filled(rect.center(), 3.0, color);
                                            ui.add(egui::Label::new(RichText::new(name).small().color(color)).truncate())
                                                .on_hover_text(if speaking { format!("{name} is speaking") } else { name.to_owned() });
                                        });
                                    });
                            }
                        });
                }
                ui.add_space(6.0);
                ui.add_enabled_ui(snapshot.is_none() && self.discord_launch.pending.is_none(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let standard = ui.selectable_value(&mut self.settings.discord_companion, false, "Standard Discord").changed();
                        let companion = ui.selectable_value(&mut self.settings.discord_companion, true, "Vencord companion").changed();
                        if standard || companion {
                            self.discord_launch.confirm = false;
                            self.discord_launch.error = None;
                            self.save();
                            if companion {
                                self.settings.discord_auto_connect = true;
                                self.connect_discord();
                            }
                        }
                    });
                });
                ui.label(RichText::new("Use names from your voice channel. Choose the audio output Discord uses.").small().color(MUTED));
                if ui.checkbox(&mut self.settings.discord_auto_connect, "Connect automatically").changed() {
                    self.discord_launch.retry = Retry::default();
                    self.save();
                }
                if ui.checkbox(&mut self.settings.discord_auto_transcribe, "Transcribe Discord calls automatically").changed() {
                    self.save();
                }
                ui.small("Starts when you join a voice channel and saves when you leave. Calls you start yourself stay under your control.");
                if self.settings.discord_companion {
                    self.discord_companion_ui(ui, ctx);
                    return;
                }
                let needs_setup = snapshot.is_none_or(|state| !matches!(state.status, Status::Ready));
                if needs_setup {
                    ui.add_space(4.0);
                    self.discord_launch_ui(ui, ctx);
                    ui.collapsing("Manual connection", |ui| {
                        ui.label("1. Quit Discord completely, including its tray icon.");
                        ui.label("2. Copy this command and run it in PowerShell to open Discord.");
                        if ui.button("Copy launch command").clicked() {
                            ctx.copy_text(SETUP_COMMAND.to_owned());
                            self.call_status = "Discord launch command copied".into();
                        }
                        ui.label("3. Choose Connect here, then join a voice channel.");
                        ui.add_space(6.0);
                        ui.label(RichText::new("This allows programs on this computer to inspect Discord through its local debugger. Restart Discord normally to turn this access off.").small().color(MUTED));
                        if let Some(Snapshot { status: Status::Unavailable(detail), .. }) = snapshot {
                            ui.collapsing("Connection details", |ui| {
                                ui.label(RichText::new(detail).small().color(MUTED));
                            });
                        }
                    });
                }
            });
    }

    fn discord_companion_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.companion_install_ui(ui, ctx);
        ui.add_space(6.0);
        ui.label("Paste the pairing key into the Articulate plugin in Vencord once. Articulate reconnects automatically.");
        ui.small("The companion shares speaker names and activity. The companion captures each participant separately when its audio adapter is connected.");
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    crate::discord::plugin::valid_token(&self.settings.discord_pairing_key),
                    egui::Button::new("Copy pairing key"),
                )
                .clicked()
            {
                ctx.copy_text(self.settings.discord_pairing_key.clone());
                self.call_status =
                    "Pairing key copied. Paste it into the Vencord plugin settings.".into();
            }
            ui.hyperlink_to(
                "Set up the companion",
                "https://github.com/obiente/articulate/blob/main/docs/vencord.md",
            );
        });
        ui.small("Keep the pairing key private. Disconnect to stop sharing speaker activity.");
        if let Some(error) = &self.discord_launch.error {
            ui.label(RichText::new(error).color(Color32::from_rgb(244, 180, 160)));
        }
        if let Some(snapshot) = self
            .discord
            .as_ref()
            .map(|connection| connection.snapshot())
            && let Status::Unavailable(detail) = snapshot.status
        {
            ui.small(detail);
        }
    }

    fn companion_work(&mut self, ctx: Option<&egui::Context>, action: CompanionAction) {
        if self.discord_launch.companion.pending.is_some() {
            return;
        }
        self.discord_launch.companion.error = None;
        self.discord_launch.companion.progress = "Checking Vencord…".into();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.cloned();
        let result = std::thread::Builder::new()
            .name("vencord-companion-install".into())
            .spawn(move || {
                let progress = |message: &str| {
                    let _ = tx.send(CompanionEvent::Progress(message.into()));
                    if let Some(ctx) = &ctx {
                        ctx.request_repaint();
                    }
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
                if let Some(ctx) = &ctx {
                    ctx.request_repaint();
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
        if !self.settings.vencord_auto_update
            || self.settings.vencord_source.trim().is_empty()
            || self.discord_launch.companion.pending.is_some()
            || self.discord_launch.companion.error.is_some()
        {
            return;
        }
        if !self.discord_launch.companion.checked {
            self.discord_launch.companion.checked = true;
            self.companion_work(
                None,
                CompanionAction::Detect(Some(PathBuf::from(self.settings.vencord_source.trim()))),
            );
        } else if let Some(plan) = &self.discord_launch.companion.plan
            && plan.status == PluginStatus::UpdateAvailable
            && plan.can_build
        {
            self.companion_work(None, CompanionAction::Install(plan.source.clone()));
        }
    }

    fn companion_install_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if !self.discord_launch.companion.checked && self.discord_launch.companion.pending.is_none()
        {
            self.discord_launch.companion.checked = true;
            let hint = (!self.settings.vencord_source.trim().is_empty())
                .then(|| PathBuf::from(self.settings.vencord_source.trim()));
            self.companion_work(Some(ctx), CompanionAction::Detect(hint));
        }
        let busy = self.discord_launch.companion.pending.is_some();
        ui.add_space(8.0);
        ui.label(RichText::new("Companion installation").strong());
        if let Some(detection) = &self.discord_launch.companion.detection {
            ui.label(if detection.installed {
                "Vencord detected"
            } else {
                "No installed Vencord detected"
            });
            if detection.selected_active {
                ui.small("Discord is configured to use the selected custom Vencord build. Restart Discord after a companion update.");
            } else if detection.source.is_some() {
                ui.small("This source build is not active in Discord yet. Build the companion, then install the custom build.");
            }
            if detection.source.is_none() {
                ui.label("The companion needs a custom Vencord build. A regular Vencord installation cannot load the plugin files on their own.");
            }
        }
        if busy {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label(&self.discord_launch.companion.progress);
            });
            ctx.request_repaint_after(Duration::from_millis(100));
        } else {
            if let Some(plan) = &self.discord_launch.companion.plan {
                let status = plan.status;
                let source = plan.source.clone();
                let can_build = plan.can_build;
                ui.label(match status {
                    PluginStatus::Missing => "Ready to add the companion",
                    PluginStatus::Current => "Companion files are up to date",
                    PluginStatus::UpdateAvailable => "A companion update is available",
                });
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            can_build,
                            egui::Button::new(if status == PluginStatus::UpdateAvailable {
                                "Update companion"
                            } else if status == PluginStatus::Current {
                                "Rebuild companion"
                            } else {
                                "Install companion"
                            }),
                        )
                        .clicked()
                    {
                        self.companion_work(Some(ctx), CompanionAction::Install(source.clone()));
                    }
                    if ui
                        .add_enabled(
                            status == PluginStatus::Current,
                            egui::Button::new("Install custom build in Discord"),
                        )
                        .clicked()
                    {
                        self.companion_work(
                            Some(ctx),
                            CompanionAction::OpenInstaller(source.clone()),
                        );
                    }
                });
                if !can_build {
                    ui.small("Building this custom Vencord source requires Node.js 22 or newer. Reopen Articulate after installing Node.js.");
                    ui.hyperlink_to("Download Node.js", "https://nodejs.org/en/download");
                }
                ui.small(if self.discord_launch.companion.native_enabled {
                    "Separate participant audio is included. Restart Discord after rebuilding, then rejoin your call."
                } else {
                    "Rebuild the companion to include separate participant audio."
                });
            }
            ui.horizontal_wrapped(|ui| {
                if ui.button("Set up automatically").clicked() {
                    let existing = self
                        .discord_launch
                        .companion
                        .detection
                        .as_ref()
                        .and_then(|detection| detection.source.clone());
                    self.companion_work(
                        Some(ctx),
                        existing
                            .map(CompanionAction::Install)
                            .unwrap_or(CompanionAction::Prepare),
                    );
                }
                if ui.button("Check installation").clicked() {
                    self.discord_launch.companion.checked = false;
                    self.discord_launch.companion.error = None;
                }
            });
            if !self.discord_launch.companion.progress.is_empty() {
                ui.label(&self.discord_launch.companion.progress);
            }
        }
        ui.small("Automatic setup prepares Vencord and builds the companion locally. The Vencord installer closes the Discord client you select, disconnecting any active call.");
        if let Some(report) = &self.discord_launch.companion.report
            && report.built
        {
            ui.small(if report.changed {
                "The companion was updated and compiled successfully."
            } else {
                "The companion was compiled successfully."
            });
        }
        if ui
            .checkbox(
                &mut self.settings.vencord_auto_update,
                "Keep the companion up to date",
            )
            .changed()
        {
            self.save();
        }
        ui.small("When enabled, Articulate rebuilds your selected custom source when this app includes a newer companion. Discord is never restarted automatically.");
        ui.collapsing("Use an existing Vencord source checkout", |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                let previous = self.settings.vencord_source.clone();
                ui.label("Vencord source folder");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings.vencord_source)
                            .desired_width((ui.available_width() - 100.0).max(120.0)),
                    );
                    if ui.button("Browse…").clicked() {
                        match super::file_picker::open(
                            "Choose package.json in your Vencord source checkout",
                            "json",
                        ) {
                            Ok(Some(path)) => {
                                self.settings.vencord_source = path
                                    .parent()
                                    .unwrap_or(&path)
                                    .to_string_lossy()
                                    .into_owned()
                            }
                            Ok(None) => {}
                            Err(error) => {
                                self.discord_launch.companion.error = Some(format!("{error:#}"))
                            }
                        }
                    }
                });
                if previous != self.settings.vencord_source {
                    self.discord_launch.companion.checked = true;
                    self.discord_launch.companion.plan = None;
                    self.save();
                }
                if ui.button("Use this folder").clicked() {
                    self.discord_launch.companion.checked = true;
                    self.companion_work(
                        Some(ctx),
                        CompanionAction::Detect(Some(PathBuf::from(
                            self.settings.vencord_source.trim(),
                        ))),
                    );
                }
            });
        });
        if let Some(error) = &self.discord_launch.companion.error {
            ui.label(RichText::new(error).color(Color32::from_rgb(244, 180, 160)));
        }
        ui.add_space(10.0);
    }

    fn discord_launch_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.discord_launch.pending.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Relaunching Discord…");
            });
        } else if self.discord_launch.confirm {
            ui.label(RichText::new("Relaunch Discord?").strong());
            ui.label("This disconnects any active Discord call. Rejoin your voice channel after Discord opens.");
            ui.label(RichText::new("Local debugging lets apps on this computer inspect Discord. Restart Discord normally to turn it off.").small().color(MUTED));
            ui.horizontal(|ui| {
                if ui.button("Relaunch and connect").clicked() {
                    self.relaunch_discord(ctx);
                }
                if ui.button("Cancel").clicked() {
                    self.discord_launch.confirm = false;
                }
            });
        } else if ui
            .button("Relaunch Discord")
            .on_hover_text(
                "Open Discord with local debugging and connect speaker names automatically",
            )
            .clicked()
        {
            self.discord_launch.confirm = true;
        }
        if let Some(error) = &self.discord_launch.error {
            ui.label(
                RichText::new(
                    "Could not relaunch Discord. Try again or use the manual connection.",
                )
                .color(Color32::from_rgb(244, 180, 160)),
            );
            ui.collapsing("Relaunch details", |ui| {
                ui.label(error);
            });
        }
    }
}

fn current_observation(snapshot: &Snapshot) -> Option<&crate::discord::Observation> {
    snapshot.observation.as_ref().filter(|observation| {
        matches!(snapshot.status, Status::Ready)
            && observation.valid
            && observation.at.elapsed() < Duration::from_secs(1)
    })
}

pub(super) fn connection_label(snapshot: Option<&Snapshot>) -> (&'static str, bool) {
    match snapshot {
        None => ("Not connected", false),
        Some(Snapshot {
            status: Status::Connecting,
            ..
        }) => ("Connecting…", false),
        Some(Snapshot {
            status: Status::Unavailable(_),
            ..
        }) => ("Reconnecting…", false),
        Some(snapshot) => match current_observation(snapshot) {
            Some(observation) if observation.channel_id.is_some() => ("Ready", true),
            Some(_) => ("Join a voice channel", false),
            None => ("Reconnecting…", false),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn companion_setup_wraps_errors_and_scrolls_to_pairing() {
        for (width, height) in [(850.0, 620.0), (1200.0, 840.0)] {
            let (mut app, _) = super::super::tests::app();
            app.page = 3;
            app.call_tab = 2;
            app.settings.discord_companion = true;
            app.settings.discord_pairing_key = "b".repeat(64);
            app.discord_launch.companion.checked = true;
            app.discord_launch.companion.detection = Some(Detection::default());
            let error = format!(
                "Synthetic setup error at C:\\Example\\{}plugin. Choose another source folder and try again.",
                "long-folder-name\\".repeat(30)
            );
            app.discord_launch.companion.error = Some(error.clone());
            let ctx = egui::Context::default();
            theme::configure(&ctx);
            let mut error_seen = false;
            let mut pairing_visible = false;
            for frame in 0..10 {
                let mut events = vec![egui::Event::PointerMoved(egui::pos2(
                    width / 2.0,
                    height - 90.0,
                ))];
                if frame > 0 {
                    events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -500.0),
                        modifiers: Default::default(),
                    });
                }
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, height),
                        )),
                        time: Some(frame as f64 / 30.0),
                        events,
                        ..Default::default()
                    },
                    |ctx| app.surface(ctx),
                );
                for shape in &output.shapes {
                    if let egui::epaint::Shape::Text(text) = &shape.shape {
                        if text.galley.job.text == error {
                            error_seen = true;
                            assert!(
                                text.galley.size().x <= width - 48.0,
                                "Long paths must wrap inside the setup panel"
                            );
                        }
                        if text.galley.job.text == "Copy pairing key"
                            && shape
                                .clip_rect
                                .contains(text.pos + text.galley.size() / 2.0)
                        {
                            pairing_visible = true;
                        }
                    }
                }
            }
            assert!(error_seen, "The diagnostic text should be rendered");
            assert!(
                pairing_visible,
                "Pairing controls must remain reachable by scrolling at {width}x{height}"
            );
            assert!(app.discord_launch.companion.pending.is_none());
            assert!(app.discord.is_none());
        }
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
    fn companion_setup_keeps_pairing_key_out_of_rendered_text_and_stays_disconnected() {
        let (mut app, _) = super::super::tests::app();
        app.settings.discord_companion = true;
        app.settings.discord_pairing_key = "a".repeat(64);
        let ctx = egui::Context::default();
        theme::configure(&ctx);
        ctx.enable_accesskit();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(850.0, 620.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.discord_card(ui, ctx, None));
            },
        );
        let tree = output.platform_output.accesskit_update.unwrap();
        assert!(
            tree.nodes
                .iter()
                .any(|(_, node)| node.label() == Some("Copy pairing key"))
        );
        assert!(!tree.nodes.iter().any(|(_, node)| {
            node.label()
                .is_some_and(|label| label.contains(&app.settings.discord_pairing_key))
        }));
        assert!(
            !tree
                .nodes
                .iter()
                .any(|(_, node)| node.label() == Some("Relaunch Discord"))
        );
        assert!(app.discord.is_none());
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
    fn speaker_connection_buttons_remain_enabled_during_capture() {
        for (snapshot, label) in [
            (None, "Connect"),
            (
                Some(Snapshot {
                    status: Status::Ready,
                    observation: None,
                }),
                "Disconnect",
            ),
        ] {
            let (mut app, _) = super::super::tests::app();
            let control = call_capture::Control::new();
            app.call = Some(control.clone());
            app.call_rows.push(calls::Row {
                start_ms: 0,
                end_ms: 1000,
                microphone: true,
                speakers: Vec::new(),
                discord: None,
                text: "Keep this transcript.".into(),
            });
            let ctx = egui::Context::default();
            theme::configure(&ctx);
            ctx.enable_accesskit();
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(850.0, 620.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .show(ctx, |ui| app.discord_card(ui, ctx, snapshot.as_ref()));
                },
            );
            let tree = output
                .platform_output
                .accesskit_update
                .expect("Accessible button state is available");
            let button = tree
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some(label))
                .expect("Speaker connection button is rendered");
            assert!(
                !button.1.is_disabled(),
                "{label} must be available while a call is recording"
            );
            assert!(std::sync::Arc::ptr_eq(app.call.as_ref().unwrap(), &control));
            assert_eq!(control.stop_ns.load(Ordering::SeqCst), 0);
            assert_eq!(app.call_rows[0].text, "Keep this transcript.");
            assert!(
                app.discord.is_none(),
                "Rendering never initiates a connection"
            );
        }
    }
}
