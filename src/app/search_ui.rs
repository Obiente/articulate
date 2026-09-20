use super::*;
use crate::brain::search::{self, Field, Hit, Job, Results};
use theme::{INK, MUTED};

#[derive(Default)]
pub(super) struct State {
    pub query: String,
    pub results: Option<Results>,
    job: Option<Job>,
    submitted: String,
    pub error: Option<String>,
    opening: Option<Hit>,
}

impl State {
    fn receive(&mut self, result: Result<Results, String>) {
        self.job = None;
        match result {
            Ok(results) if results.query == self.submitted => {
                self.results = Some(results);
                self.error = None;
            }
            Ok(_) => {}
            Err(error) => self.error = Some(error),
        }
    }
    fn cancel(&mut self) {
        self.job = None;
    }
}

impl App {
    pub(super) fn poll_saved_search(&mut self) {
        if self.page != 8 {
            self.saved_search.cancel();
        }
        if let Some(job) = &self.saved_search.job {
            match job.try_recv() {
                Ok(result) => self.saved_search.receive(result),
                Err(mpsc::TryRecvError::Disconnected) => self
                    .saved_search
                    .receive(Err("Search stopped unexpectedly. Try again.".into())),
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let Some(hit) = self.saved_search.opening.clone() else {
            return;
        };
        if !matches!(self.page, 3 | 5) {
            self.saved_search.opening = None;
            return;
        }
        if self
            .history
            .selected
            .as_ref()
            .is_some_and(|session| session.id == hit.session_id)
        {
            self.saved_search.opening = None;
            let session = self.history.selected.as_ref().unwrap();
            self.page = if session.kind == crate::history::Kind::Dictation {
                5
            } else {
                6
            };
            self.history.notetaker_tab = if hit.field == Field::CallTranscript {
                2
            } else {
                0
            };
            if hit.field == Field::CallTranscript {
                let current = hit.row.and_then(|index| session.rows.get(index));
                if current.is_some_and(|row| row.text.contains(&hit.excerpt)) {
                    self.history.detail_search = hit.excerpt;
                } else {
                    self.history.notice = "This transcript has changed since the search. Search again for its latest text.".into();
                }
            }
        } else if self.page == 3
            && self
                .history
                .call
                .as_ref()
                .is_some_and(|session| session.id == hit.session_id)
        {
            self.saved_search.opening = None;
            if hit.field == Field::CallTranscript {
                self.call_tab = 0;
                self.call_search = hit.excerpt;
            }
        } else if !self.history.loading && self.history.error.is_some() {
            self.saved_search.opening = None;
            self.saved_search.error = Some("Could not open that saved item. It may have been deleted. Refresh the search or retry in History.".into());
            self.page = 8;
        }
    }

    fn start_saved_search(&mut self) {
        self.saved_search.cancel();
        self.saved_search.error = None;
        self.saved_search.submitted = self.saved_search.query.trim().to_owned();
        match search::start(self.saved_search.submitted.clone()) {
            Ok(job) => self.saved_search.job = Some(job),
            Err(error) => self.saved_search.error = Some(error.to_string()),
        }
    }

    #[cfg(test)]
    pub(super) fn prepare_search_capture(&mut self) {
        self.page = 8;
        self.saved_search.query = "release".into();
        self.saved_search.submitted = "release".into();
        self.saved_search.results = Some(Results {
            query: "release".into(), scanned_sessions: 12, partial: false, unreadable_sessions: 0,
            hits: vec![Hit { session_id: "synthetic-session".into(), title: "Planning the next release".into(), created_ms: 1_789_891_200_000,
                field: Field::CallTranscript, excerpt: "Let's review the first release together on Thursday before we share it with the team.".into(), row: Some(0), speaker: Some("Casey".into()), start_ms: Some(95_000), end_ms: Some(108_000) },
                Hit { session_id: "synthetic-note".into(), title: "Ideas for the next release".into(), created_ms: 1_789_891_200_000,
                field: Field::PersonalNotes, excerpt: "Keep the release checklist short. Ask Jordan to review the first recording flow.".into(), row: None, speaker: None, start_ms: None, end_ms: None }],
        });
    }

    pub(super) fn saved_search_ui(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "Search");
        ui.label(
            RichText::new("Find words in saved dictations, conversations and personal notes.")
                .color(MUTED),
        );
        ui.add_space(14.0);
        let mut submit = false;
        ui.horizontal(|ui| {
            let field = ui.add_sized(
                [(ui.available_width() - 100.0).max(120.0), 40.0],
                egui::TextEdit::singleline(&mut self.saved_search.query)
                    .hint_text("Search for a word or phrase")
                    .char_limit(160)
                    .margin(egui::vec2(12.0, 10.0)),
            );
            submit |= field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            submit |= ui
                .add_enabled(
                    !self.saved_search.query.trim().is_empty(),
                    egui::Button::new(RichText::new("Search").color(theme::BASE))
                        .fill(ACCENT)
                        .min_size(egui::vec2(84.0, 40.0)),
                )
                .clicked();
        });
        if submit {
            self.start_saved_search();
        }
        if self.saved_search.job.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Searching for “{}”…", self.saved_search.submitted));
                if ui.small_button("Cancel").clicked() {
                    self.saved_search.cancel();
                }
            });
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        if let Some(error) = &self.saved_search.error {
            ui.colored_label(Color32::LIGHT_YELLOW, error);
        }
        ui.add_space(10.0);
        let Some(results) = &self.saved_search.results else {
            if self.saved_search.job.is_none() && self.saved_search.error.is_none() {
                ui.add_space(24.0);
                ui.label(RichText::new("Your saved words are searchable here.").size(22.0));
                ui.label(
                    RichText::new(
                        "Search stays on this device. Live, unsaved text is not included.",
                    )
                    .color(MUTED),
                );
            }
            return;
        };
        ui.label(
            RichText::new(format!(
                "{} matches for “{}” · {} saved items searched",
                results.hits.len(),
                results.query,
                results.scanned_sessions
            ))
            .color(MUTED),
        );
        if results.query != self.saved_search.query.trim() {
            ui.small(
                "These results are from your previous search. Choose Search to use the new phrase.",
            );
        }
        if results.partial {
            ui.colored_label(Color32::LIGHT_YELLOW,"Showing a limited set of recent saved content. Try a more specific phrase; older items may not have been searched.");
        }
        if results.unreadable_sessions > 0 {
            ui.colored_label(
                Color32::LIGHT_YELLOW,
                format!(
                    "{} saved items could not be searched. Open History to check them.",
                    results.unreadable_sessions
                ),
            );
        }
        ui.add_space(8.0);
        let mut open = None;
        egui::ScrollArea::vertical()
            .id_salt("saved_search_results")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if results.hits.is_empty() {
                    ui.add_space(20.0);
                    ui.label(
                        RichText::new(if results.partial {
                            "No matches in the content searched."
                        } else {
                            "No matching saved text."
                        })
                        .size(22.0),
                    );
                }
                for (index, hit) in results.hits.iter().enumerate() {
                    ui.push_id(index, |ui| {
                        egui::Frame::new()
                            .fill(theme::SURFACE)
                            .corner_radius(12)
                            .inner_margin(16)
                            .show(ui, |ui| {
                                ui.set_min_width((ui.available_width() - 1.0).max(0.0));
                                ui.horizontal(|ui| {
                                    ui.allocate_ui_with_layout(
                                        egui::vec2((ui.available_width() - 120.0).max(80.0), 36.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(if hit.title.is_empty() {
                                                        "Untitled"
                                                    } else {
                                                        &hit.title
                                                    })
                                                    .size(19.0)
                                                    .strong()
                                                    .color(INK),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(&hit.title);
                                        },
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.button("Open source").clicked() {
                                                open = Some(hit.clone());
                                            }
                                        },
                                    );
                                });
                                ui.label(RichText::new(metadata(hit)).small().color(ACCENT));
                                ui.add_space(8.0);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&hit.excerpt).size(17.0).color(INK),
                                    )
                                    .wrap()
                                    .selectable(true),
                                );
                            });
                        ui.add_space(10.0);
                    });
                }
            });
        if let Some(hit) = open {
            self.page = 5;
            self.notetaker_open(hit.session_id.clone());
            self.saved_search.opening = Some(hit);
            self.poll_saved_search();
        }
    }
}

fn metadata(hit: &Hit) -> String {
    let field = match hit.field {
        Field::Title => "Title",
        Field::Dictation => "Dictation",
        Field::PersonalNotes => "My notes",
        Field::CallTranscript => "Transcript",
    };
    let when =
        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(hit.created_ms) * 1_000_000)
            .ok()
            .map(|timestamp| {
                let local = timestamp.to_offset(
                    time::UtcOffset::local_offset_at(timestamp).unwrap_or(time::UtcOffset::UTC),
                );
                format!(
                    "{} · {:02}:{:02}",
                    local.date(),
                    local.hour(),
                    local.minute()
                )
            })
            .unwrap_or_else(|| "Date unavailable".into());
    let mut details = format!("{field} · {when}");
    if let Some(speaker) = &hit.speaker {
        details.push_str(&format!(" · {speaker}"));
    }
    if let (Some(start), Some(end)) = (hit.start_ms, hit.end_ms) {
        details.push_str(&format!(" · {} to {}", stamp(start), stamp(end)));
    }
    details
}
fn stamp(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn superseded_search_results_never_replace_current_query() {
        let mut state = State {
            submitted: "new phrase".into(),
            ..Default::default()
        };
        state.receive(Ok(Results {
            query: "old phrase".into(),
            ..Default::default()
        }));
        assert!(state.results.is_none());
        state.receive(Ok(Results {
            query: "new phrase".into(),
            ..Default::default()
        }));
        assert_eq!(state.results.unwrap().query, "new phrase");
    }
    #[test]
    fn result_metadata_preserves_source_turn_times() {
        let hit = Hit {
            session_id: "synthetic".into(),
            title: "Plan".into(),
            created_ms: 0,
            field: Field::CallTranscript,
            excerpt: "Exact source words.".into(),
            row: Some(0),
            speaker: Some("Casey".into()),
            start_ms: Some(90_000),
            end_ms: Some(125_000),
        };
        let label = metadata(&hit);
        assert!(label.contains("Casey"));
        assert!(label.contains("01:30 to 02:05"));
    }
    #[test]
    fn open_source_actions_fit_narrow_and_wide_search_results() {
        for (width, height) in [(850.0, 620.0), (1200.0, 840.0)] {
            let (mut app, _) = super::super::tests::app();
            app.prepare_search_capture();
            let ctx = egui::Context::default();
            theme::configure(&ctx);
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, height),
                )),
                ..Default::default()
            };
            let mut output = ctx.run(input(), |ctx| app.surface(ctx));
            for _ in 0..2 {
                output = ctx.run(input(), |ctx| app.surface(ctx));
            }
            let actions: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) if text.galley.job.text == "Open source" => {
                        Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(actions.len(), 2);
            assert!(
                actions
                    .iter()
                    .all(|rect| rect.right() <= width - 32.0 && rect.bottom() <= height - 32.0),
                "Action overflow at {width}: {actions:?}"
            );
        }
    }

    #[test]
    fn source_open_waits_for_exact_loaded_id_and_selects_transcript() {
        let (mut app, _) = super::super::tests::app();
        app.prepare_search_capture();
        let mut hit = app.saved_search.results.as_ref().unwrap().hits[0].clone();
        let mut session = crate::history::Session::new(crate::history::Kind::Call);
        hit.session_id = session.id.clone();
        session.rows.push(calls::Row {
            start_ms: 95_000,
            end_ms: 108_000,
            microphone: true,
            speakers: vec![],
            discord: None,
            text: hit.excerpt.clone(),
        });
        app.page = 5;
        app.saved_search.opening = Some(hit.clone());
        app.history.selected = Some(crate::history::Session::new(crate::history::Kind::Call));
        app.poll_saved_search();
        assert_eq!(app.page, 5);
        assert!(app.saved_search.opening.is_some());
        app.history.selected = Some(session);
        app.poll_saved_search();
        assert_eq!(app.page, 6);
        assert_eq!(app.history.notetaker_tab, 2);
        assert_eq!(app.history.detail_search, hit.excerpt);
        assert!(app.saved_search.opening.is_none());
    }
}
