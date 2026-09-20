use super::*;
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
}

impl App {
    fn connect_discord(&mut self) {
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
                    self.discord_launch.error =
                        Some(format!("Could not connect the companion: {error:#}"));
                    return;
                }
            }
        } else {
            crate::discord::Connection::start()
        };
        self.discord = Some(connection);
        if let Some(control) = &self.call {
            control.set_discord(self.discord.clone());
        }
    }

    pub(super) fn poll_discord_launch(&mut self) {
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
                            if let Some(control) = &self.call {
                                control.set_discord(None);
                            }
                            connection.disconnect();
                        } else {
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
                        }
                    });
                });
                ui.label(RichText::new("Use names from your voice channel. Choose the audio output Discord uses.").small().color(MUTED));
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
        ui.add_space(6.0);
        ui.label("Connect here, then paste the pairing key into the Articulate plugin in Vencord.");
        ui.small("The companion shares speaker names and activity. Call audio still comes from the selected output.");
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
