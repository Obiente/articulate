use super::{ACCENT, App, theme};
use crate::insights::Report;
use eframe::egui::{self, RichText};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct State {
    pub report: Option<Report>,
    pub loading: bool,
    pub error: Option<String>,
    pub stale: bool,
    refreshed: Option<Instant>,
}

impl App {
    pub(super) fn insights_ui(&mut self, ui: &mut egui::Ui) {
        let due = self
            .insights
            .refreshed
            .is_none_or(|at| at.elapsed() > Duration::from_secs(5));
        if !self.insights.loading && due && (self.insights.report.is_none() || self.insights.stale)
        {
            self.insights_refresh();
        }
        ui.horizontal(|ui| {
            theme::page_title(ui, "Insights");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!self.insights.loading, egui::Button::new("Refresh"))
                    .clicked()
                {
                    self.insights_refresh();
                }
                if self.insights.loading {
                    ui.spinner();
                }
            });
        });
        ui.label(RichText::new("Your dictation habits, on this device.").color(theme::MUTED));
        ui.add_space(14.0);
        if let Some(error) = &self.insights.error {
            ui.label(error);
        }
        let Some(report) = &self.insights.report else {
            ui.label(if self.insights.loading {
                "Reading your saved dictations…"
            } else {
                "Insights appear after you save a dictation."
            });
            return;
        };
        egui::ScrollArea::vertical().id_salt("insights_body").auto_shrink([false, false]).show(ui, |ui| {
            let metrics = [
                (number(report.words), "Dictation words", "Recognized words, with saved text used for older recordings."),
                (report.words_per_minute.map_or_else(|| "Not measured".into(), |pace| format!("{pace:.0} wpm")), "Dictation pace", "Words divided by microphone recording time, including pauses. Imported audio and older untimed sessions are excluded."),
                (number(report.dictionary_replacements.saturating_add(report.cleanup_edits)), "Cleanup edits", "Recorded vocabulary replacements and cleanup rule applications. This does not measure transcription accuracy."),
                (format!("{} days", report.current_streak), "Current streak", "Consecutive local days with saved dictations, ending today or yesterday."),
            ];
            let columns = if ui.available_width() >= 850.0 { 4 } else { 2 };
            for row in metrics.chunks(columns) {
                ui.columns(columns, |columns| {
                    for (column, (value, label, explanation)) in columns.iter_mut().zip(row) {
                        egui::Frame::new().fill(theme::SURFACE).corner_radius(16).inner_margin(18).show(column, |ui| {
                            ui.set_min_width((ui.available_width() - 1.0).max(0.0));
                            ui.label(RichText::new(value).size(29.0).strong().color(theme::INK));
                            ui.label(RichText::new(*label).color(theme::MUTED)).on_hover_text(*explanation);
                        });
                    }
                });
                ui.add_space(10.0);
            }
            ui.label(RichText::new(format!("{} saved dictations · {} active days · longest streak: {} days", number(report.sessions), report.active_days, report.longest_streak)).color(theme::MUTED));
            ui.add_space(20.0);
            panel(ui, "Your activity", |ui| {
                ui.label(RichText::new("Last 28 days").color(theme::MUTED));
                let max = report.days.iter().map(|day| day.words).max().unwrap_or(1).max(1) as f32;
                let width = ((ui.available_width() - 6.0 * 8.0) / 7.0).clamp(28.0, 92.0);
                for week in report.days.chunks(7) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for day in week {
                            let intensity = (day.words as f32 / max).sqrt();
                            let color = if day.words == 0 { theme::BASE } else { egui::Color32::from_rgb((36.0 + 60.0*intensity) as u8,(76.0+121.0*intensity) as u8,(68.0+104.0*intensity) as u8) };
                            let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 39.0), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 7, color);
                            let date = day.date.get(5..).unwrap_or(&day.date);
                            ui.painter().text(rect.center(),egui::Align2::CENTER_CENTER,date,egui::FontId::proportional(13.0),if intensity > 0.6 { theme::BASE } else { theme::INK });
                            let detail = format!("{}: {} words in {} dictations", day.date, number(day.words), day.sessions);
                            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, &detail));
                            response.on_hover_text(detail);
                        }
                    });
                }
                ui.add_space(6.0);
                ui.label(RichText::new("Darker days are quieter. Brighter days have more dictated words.").small().color(theme::MUTED));
            });
            ui.add_space(18.0);
            panel(ui, "Where you dictate", |ui| {
                if report.apps.is_empty() { ui.label("Dictate into an app to see your usage here."); }
                for app in &report.apps {
                    let share = app.words as f32 / report.words.max(1) as f32;
                    ui.horizontal(|ui| {
                        ui.label(app.app.as_deref().unwrap_or("Articulate or unrecorded app")).on_hover_text(format!("{} saved dictations", app.sessions));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { ui.label(format!("{} words · {:.0}%", number(app.words), share * 100.0)); });
                    });
                    ui.add(egui::ProgressBar::new(share).fill(ACCENT).desired_height(8.0));
                    ui.add_space(8.0);
                }
            });
            ui.add_space(18.0);
            ui.collapsing("How these numbers work", |ui| {
                ui.label("Insights use the dictations currently saved in History. Deleting a dictation removes it from these totals. Calls and personal notes are excluded.");
                ui.label(format!("Pace is available for {} measured recordings. Cleanup counts are available for {} recordings. Older recordings may not include these measurements.", report.measured_sessions, report.correction_sessions));
                ui.label(format!("Vocabulary replacements: {}. Cleanup rule applications: {}.",number(report.dictionary_replacements),number(report.cleanup_edits)));
                ui.label(if report.dates_use_utc { "Dates use UTC because local time was unavailable." } else { "Activity dates use your computer's local time zone." });
                ui.label("No usage data is sent anywhere. There is no comparison with other users.");
            });
        });
    }

    fn insights_refresh(&mut self) {
        if let Some(worker) = &self.history.worker {
            worker.insights();
            self.insights.loading = true;
            self.insights.stale = false;
            self.insights.refreshed = Some(Instant::now());
        } else {
            self.insights.error =
                Some("Saved history is unavailable. Open History to retry saving.".into());
        }
    }
}

fn panel(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .corner_radius(16)
        .inner_margin(18)
        .show(ui, |ui| {
            ui.set_min_width((ui.available_width() - 1.0).max(0.0));
            ui.label(RichText::new(title).size(21.0).strong());
            ui.add_space(8.0);
            contents(ui);
        });
}

fn number(value: u64) -> String {
    let digits = value.to_string();
    let mut result = String::new();
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            result.push(',');
        }
        result.push(character);
    }
    result
}
