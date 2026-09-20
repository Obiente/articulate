//! Opt-in native visual check. All content is synthetic; no audio workers run.
use super::*;

fn populated_app() -> App {
    let (mut app, _) = tests::app();
    app.page = 3;
    app.preview_inflight = false;
    app.call_status = "Call transcript ready".into();
    app.speaker_names = [
        "Casey".into(),
        "Jordan".into(),
        String::new(),
        String::new(),
    ];
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

fn page_app(page: &str) -> App {
    let mut app = match page {
        "history" => populated_history_app(false),
        "reader" => populated_history_app(true),
        _ => populated_app(),
    };
    match page {
        "notetaker"
        | "notetaker-editor"
        | "notetaker-active"
        | "notetaker-live-editor"
        | "notetaker-transcript"
        | "notetaker-highlights" => {
            app = populated_history_app(false);
            app.page = 6;
            let mut note = crate::history::Session::new(crate::history::Kind::Note);
            note.title = "Ideas for the next release".into();
            note.personal_notes = "Keep the first recording simple.

Questions for the team
• Which microphone should we recommend?
• Can we make the first saved transcript easier to find?

Next steps
Ask Casey to review the onboarding copy before Thursday."
                .into();
            app.history.items.insert(0, (&note).into());
            if page == "notetaker-editor" {
                app.history.selected = Some(note);
            } else if page == "notetaker-active" || page == "notetaker-live-editor" {
                app.call = Some(call_capture::Control::new());
                let mut session = crate::history::Session::new(crate::history::Kind::Call);
                session.title = "Weekly product check-in".into();
                session.personal_notes = note.personal_notes;
                app.history.call = Some(session);
                if page == "notetaker-live-editor" {
                    app.page = 3;
                    app.call_tab = 3;
                }
            } else if page == "notetaker-transcript" || page == "notetaker-highlights" {
                let mut session = crate::history::Session::new(crate::history::Kind::Call);
                session.title = "Planning the next release".into();
                session.rows = app.call_rows.clone();
                session.speaker_names = app.speaker_names.clone();
                session.text = calls::text(&session.rows, &session.speaker_names);
                session.notes = Some(crate::notes::Notes::build(&session.rows));
                session.personal_notes = note.personal_notes;
                app.history.selected = Some(session);
                app.history.notetaker_tab = if page == "notetaker-transcript" { 2 } else { 1 };
            }
        }
        "notes" | "notes-review" => {
            app.call_tab = 1;
            app.call_rows.truncate(3);
            app.call_committed = app.call_rows.clone();
            if page == "notes-review" {
                app.prepare_assort_review_capture(true);
            }
        }
        "discord-setup" => {
            app.call_tab = 2;
            let defaults = (
                app.settings.discord_auto_connect,
                app.settings.discord_auto_transcribe,
            );
            app.prepare_companion_capture();
            // This harness calls surface only, never connection polling. Preserve
            // actual preference defaults without opening any Discord connection.
            app.settings.discord_auto_connect = defaults.0;
            app.settings.discord_auto_transcribe = defaults.1;
        }
        "calls" => {
            app.call = Some(call_capture::Control::new());
            app.call_status = "Listening to your microphone and call audio".into();
            app.call_rows.truncate(4);
            for (index,text) in ["The new onboarding feels much clearer. I think we should keep the first step focused on getting a good microphone level.", "Agreed. Once someone finishes their first recording, we can show them where their transcript is saved.", "I will update the checklist and share it before tomorrow. Let us keep the download step short and explain what happens next.", "And we should also make sure..."].iter().enumerate() {
                app.call_rows[index].text=(*text).into();
                app.call_rows[index].microphone=index>=2;
                app.call_rows[index].speakers=vec![if index==0 {1}else{2}];
            }
            let rows = std::mem::take(&mut app.call_rows);
            calls::append_rows(&mut app.call_committed, rows[..3].to_vec());
            app.call_rows = app.call_committed.clone();
            calls::append_rows(&mut app.call_rows, rows[3..].to_vec());
            let mut session = crate::history::Session::new(crate::history::Kind::Call);
            session.title = "Weekly check-in".into();
            app.history.call = Some(session);
        }

        "dictate" | "correction-review" | "polish" => {
            app.page = 0;
            app.text = "Hi Casey, could you review the updated plan before tomorrow? I moved the launch check-in to Thursday at three.".into();
            app.raw = app.text.clone();
            app.status = "Your transcript is ready".into();
            if page == "polish" {
                app.prepare_polish_capture();
            }
            if page == "correction-review" {
                app.prepare_assort_review_capture(false);
            }
        }
        "vocabulary" => {
            app.page = 1;
            app.populate_editor_capture(false);
        }
        "shortcuts" => {
            app.page = 4;
            app.populate_editor_capture(true);
        }
        "settings" => {
            app.page = 2;
        }
        _ => {}
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
            transcript_clip.bottom() <= height - 32.0,
            "Transcript extends beyond the content canvas"
        );
    }
}

#[test]
fn review_actions_and_first_note_are_visible_without_scrolling() {
    for (width, height) in [(850.0, 620.0), (1200.0, 840.0)] {
        for page in ["notes-review", "dictate"] {
            let mut app = page_app(page);
            let first_quote = app.call_rows[0].text.clone();
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
            let labels = if page == "notes-review" {
                vec!["Save 2 selected passages", first_quote.as_str()]
            } else {
                vec!["Review vocabulary"]
            };
            for label in labels {
                let visible = output.as_ref().unwrap().shapes.iter().any(|shape| {
                    if let egui::epaint::Shape::Text(text) = &shape.shape {
                        let bounds = text.galley.rect.translate(text.pos.to_vec2());
                        text.galley.job.text == label && shape.clip_rect.contains_rect(bounds)
                    } else {
                        false
                    }
                });
                assert!(visible, "{label} is clipped at {width}x{height} on {page}");
            }
        }
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
            if std::env::var_os("ARTICULATE_UI_ASSORT_MODELS").is_some() {
                egui::CentralPanel::default().show(ctx, |ui| self.app.assort_settings_ui(ui));
            } else if self.card {
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
                                avatar: None,
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
            // Let window and disclosure animations settle before visual QA.
            if self.frames == 12 {
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
            // Capture final visual states, not a timing-dependent window fade.
            cc.egui_ctx.style_mut(|style| style.animation_time = 0.0);
            let mut app = if let Ok(page) = std::env::var("ARTICULATE_UI_PAGE") {
                page_app(&page)
            } else {
                match std::env::var("ARTICULATE_UI_HISTORY").as_deref() {
                    Ok("list") => populated_history_app(false),
                    Ok("detail") => populated_history_app(true),
                    _ => populated_app(),
                }
            };
            if let Some(session) = app.history.selected.as_ref().or(app.history.call.as_ref())
                && std::env::var("ARTICULATE_UI_PAGE")
                    .is_ok_and(|page| page.starts_with("notetaker"))
            {
                let folder = output
                    .parent()
                    .unwrap()
                    .join(format!("fixture-{}", session.id));
                let worker = crate::history::Worker::test_directory(folder);
                worker.save(session.clone());
                app.history.worker = Some(worker);
            }
            Ok(Box::new(Capture {
                app,
                output,
                frames: 0,
                card: std::env::var_os("ARTICULATE_UI_CARD").is_some(),
            }))
        }),
    )
    .unwrap();
}

#[test]
fn library_save_stays_visible_at_supported_sizes() {
    for (page, label) in [
        ("shortcuts", "Save shortcut"),
        ("vocabulary", "Save correction"),
    ] {
        for (width, height) in [(850.0, 620.0), (1200.0, 840.0), (1920.0, 1080.0)] {
            let mut app = page_app(page);
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
            let shape = output
                .shapes
                .iter()
                .find_map(|shape| {
                    if let egui::epaint::Shape::Text(text) = &shape.shape
                        && text.galley.job.text == label
                    {
                        Some((
                            shape.clip_rect,
                            text.galley.rect.translate(text.pos.to_vec2()),
                        ))
                    } else {
                        None
                    }
                })
                .expect("Save shortcut must be rendered");
            assert!(
                shape.0.contains_rect(shape.1),
                "Save clipped at {width}x{height}: {shape:?}"
            );
            assert!(shape.1.bottom() < height - 44.0, "Save overlaps footer");
        }
    }
}

#[test]
fn live_calls_keep_reading_space_with_follow_controls() {
    for (width, height, minimum) in [
        (850.0, 620.0, 300.0),
        (1200.0, 840.0, 450.0),
        (1920.0, 1080.0, 680.0),
    ] {
        let mut app = populated_app();
        app.call = Some(call_capture::Control::new());
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
        let clip = output
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
            .expect("Live transcript renders");
        assert!(
            clip.height() >= minimum,
            "Live transcript too short at {width}x{height}: {clip:?}"
        );
        assert!(
            clip.bottom() <= height - 32.0,
            "Live transcript extends beyond the content canvas"
        );
    }
}

#[test]
fn sidebar_navigation_supports_keyboard_activation() {
    let mut app = page_app("dictate");
    let ctx = egui::Context::default();
    theme::configure(&ctx);
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(850.0, 620.0),
        )),
        ..Default::default()
    };
    let _ = ctx.run(input(), |ctx| app.surface(ctx));
    for page in [6usize, 1, 4, 5, 2, 0] {
        let id = ctx
            .data_mut(|data| {
                data.get_temp::<egui::Id>(egui::Id::new(("navigation_response", page)))
            })
            .unwrap();
        ctx.memory_mut(|memory| memory.request_focus(id));
        let mut raw = input();
        raw.events.push(egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        });
        let _ = ctx.run(raw, |ctx| app.surface(ctx));
        assert_eq!(app.page, page);
        let mut raw = input();
        raw.events.push(egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: Default::default(),
        });
        let _ = ctx.run(raw, |ctx| app.surface(ctx));
    }
}
