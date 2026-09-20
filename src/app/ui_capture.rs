//! Opt-in native visual check. All content is synthetic; no audio workers run.
use super::*;

fn populated_app() -> App {
    let (mut app, _) = tests::app();
    app.page = 3;
    app.preview_inflight = false;
    app.call_status = "Call transcript ready".into();
    app.speaker_names = ["Alex".into(), "Sam".into(), String::new(), String::new()];
    for index in 0..16 {
        app.call_rows.push(calls::Row {
            start_ms: index * 14000,
            end_ms: index * 14000 + 12000,
            microphone: index % 3 == 0,
            speakers: vec![(index % 2 + 1) as i32],
            discord: None,
            text: match index % 3 {
                0 => "Let's keep the conversation easy to follow. The transcript should have room to breathe, with every speaker clearly identified.",
                1 => "Agreed. We can review the notes after the call. Right now, I want to see what everyone is saying without scrolling past the settings.",
                _ => "I'll check the final details tomorrow and send an update. This gives us a useful record to come back to.",
            }.into(),
        });
    }
    app
}

fn populated_history_app(detail: bool) -> App {
    let mut app = populated_app();
    app.page = 5;
    let mut call = crate::history::Session::new(crate::history::Kind::Call);
    call.title = "Planning the next release".into();
    call.rows = app.call_rows.clone();
    call.speaker_names = app.speaker_names.clone();
    call.text = calls::text(&call.rows, &call.speaker_names);
    call.notes = Some(crate::notes::Notes::build(&call.rows));
    let mut dictation = crate::history::Session::new(crate::history::Kind::Dictation);
    dictation.title = "An update for the team".into();
    dictation.text = "The changes are ready to review. Let's discuss the details tomorrow.".into();
    dictation.original =
        "The changes are, um, ready to review. Let's discuss the details tomorrow.".into();
    app.history.items = vec![(&call).into(), (&dictation).into()];
    if detail {
        app.history.selected = Some(call);
    }
    app
}

#[test]
fn calls_reserve_readable_transcript_height() {
    for (width, height, minimum) in [(850.0, 620.0, 300.0), (1200.0, 840.0, 450.0)] {
        let mut app = populated_app();
        let ctx = egui::Context::default();
        theme::configure(&ctx);
        let mut output = None;
        for _ in 0..3 {
            output = Some(ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, height),
                    )),
                    ..Default::default()
                },
                |ctx| app.surface(ctx),
            ));
        }
        let output = output.unwrap();
        let transcript_clip = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text)
                    if app
                        .call_rows
                        .iter()
                        .any(|row| text.galley.job.text == row.text) =>
                {
                    Some(shape.clip_rect)
                }
                _ => None,
            })
            .expect("Transcript is painted in the default Calls tab");
        assert!(
            transcript_clip.height() >= minimum,
            "Transcript clip too short at {width}x{height}: {transcript_clip:?}"
        );
        assert!(
            transcript_clip.bottom() <= height - 44.0,
            "Transcript overlaps footer"
        );
    }
}

#[cfg(windows)]
#[test]
#[ignore = "Opens a synthetic Calls window and saves to ARTICULATE_UI_CAPTURE"]
fn capture_calls_ui() {
    use winit::platform::windows::EventLoopBuilderExtWindows;
    struct Capture {
        app: App,
        output: PathBuf,
        frames: usize,
        card: bool,
    }
    impl eframe::App for Capture {
        fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
            if self.card {
                let snapshot = crate::discord::Snapshot {
                    status: crate::discord::Status::Ready,
                    observation: Some(crate::discord::Observation {
                        at: Instant::now(),
                        generation: 1,
                        channel_id: Some("synthetic-channel".into()),
                        valid: true,
                        participants: ["Casey", "Jordan", "Dana", "Morgan"]
                            .into_iter()
                            .enumerate()
                            .map(|(index, name)| crate::discord::Participant {
                                id: format!("synthetic-{index}"),
                                name: name.into(),
                                speaking: index == 1 || index == 3,
                                is_self: index == 0,
                            })
                            .collect(),
                    }),
                };
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(10.0, 4.0);
                    ui.spacing_mut().button_padding = egui::vec2(12.0, 6.0);
                    ui.spacing_mut().interact_size.y = 28.0;
                    let setup = std::env::var_os("ARTICULATE_UI_DISCORD_SETUP").is_some();
                    if setup {
                        self.app.discord_launch.confirm = true;
                    }
                    self.app
                        .discord_card(ui, ctx, if setup { None } else { Some(&snapshot) });
                });
            } else {
                self.app.surface(ctx);
            }
            self.frames += 1;
            if self.frames == 3 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
            let saved = ctx.input(|input| {
                for event in &input.events {
                    if let egui::Event::Screenshot { image, .. } = event {
                        use std::io::Write;
                        let mut file =
                            std::io::BufWriter::new(std::fs::File::create(&self.output).unwrap());
                        write!(file, "P6\n{} {}\n255\n", image.width(), image.height()).unwrap();
                        for pixel in &image.pixels {
                            file.write_all(&pixel.to_array()[..3]).unwrap();
                        }
                        file.flush().unwrap();
                        return true;
                    }
                }
                false
            });
            if saved {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            assert!(self.frames < 300, "Native screenshot did not arrive");
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }
    let output = PathBuf::from(
        std::env::var_os("ARTICULATE_UI_CAPTURE")
            .expect("Set ARTICULATE_UI_CAPTURE to an ignored .ppm output path"),
    );
    let width = std::env::var("ARTICULATE_UI_WIDTH")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(1200.0);
    let height = std::env::var("ARTICULATE_UI_HEIGHT")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(840.0);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([width, height])
            .with_title("Articulate visual check"),
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    eframe::run_native(
        "Articulate visual check",
        options,
        Box::new(move |cc| {
            theme::configure(&cc.egui_ctx);
            Ok(Box::new(Capture {
                app: match std::env::var("ARTICULATE_UI_HISTORY").as_deref() {
                    Ok("list") => populated_history_app(false),
                    Ok("detail") => populated_history_app(true),
                    _ => populated_app(),
                },
                output,
                frames: 0,
                card: std::env::var_os("ARTICULATE_UI_CARD").is_some(),
            }))
        }),
    )
    .unwrap();
}
