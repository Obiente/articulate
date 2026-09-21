//! Statistics derived from retained local dictation history, never audio or telemetry.
use crate::history::{Kind, Session};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use time::{Date, Duration, OffsetDateTime, UtcOffset};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DictationMetrics {
    /// Captured audio sample duration; not recognition or UI processing time.
    pub audio_duration_ms: Option<u64>,
    /// Final raw recognition count, before macros, cleanup, or manual edits.
    pub recognized_words: Option<u64>,
    /// Target process basename only; never a title, document, or path.
    pub app: Option<String>,
    pub dictionary_replacements: Option<u64>,
    pub cleanup_edits: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub words: u64,
    pub sessions: u64,
    pub measured_sessions: u64,
    pub audio_duration_ms: u64,
    pub words_per_minute: Option<f64>,
    pub active_days: u64,
    pub current_streak: u64,
    pub longest_streak: u64,
    pub apps: Vec<AppUsage>,
    /// Last 28 calendar days, oldest first, including zero-activity dates.
    pub days: Vec<DayActivity>,
    #[allow(
        dead_code,
        reason = "Preserve the date-basis flag for React insights messaging"
    )]
    pub dates_use_utc: bool,
    pub dictionary_replacements: u64,
    pub cleanup_edits: u64,
    pub correction_sessions: u64,
}

#[derive(Clone, Debug)]
pub struct AppUsage {
    pub app: Option<String>,
    pub words: u64,
    pub sessions: u64,
}

#[derive(Clone, Debug)]
pub struct DayActivity {
    pub date: String,
    pub words: u64,
    pub sessions: u64,
}

struct Record {
    updated_ms: u64,
    day: Date,
    words: u64,
    measured: Option<(u64, u64)>,
    dictionary_replacements: Option<u64>,
    cleanup_edits: Option<u64>,
    app: Option<String>,
}

pub(crate) struct Aggregate {
    now_ms: u64,
    today: Date,
    records: BTreeMap<String, Record>,
    dates_use_utc: bool,
}

fn timestamp(ms: u64) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok()
}

impl Aggregate {
    pub(crate) fn new(now_ms: u64) -> Self {
        let now = timestamp(now_ms).unwrap_or(OffsetDateTime::UNIX_EPOCH);
        let offset = UtcOffset::local_offset_at(now).ok();
        Self {
            now_ms,
            today: now.to_offset(offset.unwrap_or(UtcOffset::UTC)).date(),
            records: BTreeMap::new(),
            dates_use_utc: offset.is_none(),
        }
    }

    pub(crate) fn push(&mut self, session: &Session) {
        let Some(time) = timestamp(session.created_ms) else {
            return;
        };
        let offset = UtcOffset::local_offset_at(time).ok();
        self.dates_use_utc |= offset.is_none();
        self.push_on(
            session,
            time.to_offset(offset.unwrap_or(UtcOffset::UTC)).date(),
        );
    }

    fn push_on(&mut self, session: &Session, day: Date) {
        if session.kind != Kind::Dictation || session.created_ms > self.now_ms {
            return;
        }
        let words = session.metrics.recognized_words.unwrap_or_else(|| {
            session
                .text
                .split_whitespace()
                .filter(|word| word.chars().any(char::is_alphanumeric))
                .count() as u64
        });
        if self
            .records
            .get(&session.id)
            .is_some_and(|old| old.updated_ms > session.updated_ms)
        {
            return;
        }
        let measured = match (
            session.metrics.recognized_words,
            session.metrics.audio_duration_ms,
        ) {
            (Some(words), Some(ms)) if words > 0 && ms > 0 => Some((words, ms)),
            _ => None,
        };
        let app = session
            .metrics
            .app
            .as_deref()
            .and_then(|app| crate::dictionary::app_scope(app).ok().flatten());
        self.records.insert(
            session.id.clone(),
            Record {
                updated_ms: session.updated_ms,
                day,
                words,
                measured,
                app,
                dictionary_replacements: session.metrics.dictionary_replacements,
                cleanup_edits: session.metrics.cleanup_edits,
            },
        );
    }

    pub(crate) fn finish(self) -> Report {
        let mut report = Report {
            dates_use_utc: self.dates_use_utc,
            ..Default::default()
        };
        let mut days: BTreeMap<Date, (u64, u64)> = BTreeMap::new();
        let mut apps: BTreeMap<Option<String>, (u64, u64)> = BTreeMap::new();
        let mut measured_words = 0u64;
        for record in self.records.into_values() {
            if record.words == 0 {
                continue;
            }
            report.words = report.words.saturating_add(record.words);
            report.sessions += 1;
            if record.dictionary_replacements.is_some() || record.cleanup_edits.is_some() {
                report.correction_sessions += 1;
                report.dictionary_replacements = report
                    .dictionary_replacements
                    .saturating_add(record.dictionary_replacements.unwrap_or(0));
                report.cleanup_edits = report
                    .cleanup_edits
                    .saturating_add(record.cleanup_edits.unwrap_or(0));
            }
            let day = days.entry(record.day).or_default();
            day.0 = day.0.saturating_add(record.words);
            day.1 += 1;
            let app = apps.entry(record.app).or_default();
            app.0 = app.0.saturating_add(record.words);
            app.1 += 1;
            if let Some((words, ms)) = record.measured {
                report.measured_sessions += 1;
                measured_words = measured_words.saturating_add(words);
                report.audio_duration_ms = report.audio_duration_ms.saturating_add(ms);
            }
        }
        if report.audio_duration_ms > 0 {
            report.words_per_minute =
                Some(measured_words as f64 * 60_000.0 / report.audio_duration_ms as f64);
        }
        report.active_days = days.len() as u64;
        let mut previous: Option<Date> = None;
        let mut run = 0;
        for day in days.keys().copied() {
            run = if previous.and_then(Date::next_day) == Some(day) {
                run + 1
            } else {
                1
            };
            report.longest_streak = report.longest_streak.max(run);
            previous = Some(day);
        }
        let mut cursor = if days.contains_key(&self.today) {
            Some(self.today)
        } else {
            self.today.previous_day()
        };
        while let Some(day) = cursor {
            if !days.contains_key(&day) {
                break;
            }
            report.current_streak += 1;
            cursor = day.previous_day();
        }
        report.apps = apps
            .into_iter()
            .map(|(app, (words, sessions))| AppUsage {
                app,
                words,
                sessions,
            })
            .collect();
        report
            .apps
            .sort_by(|a, b| b.words.cmp(&a.words).then_with(|| a.app.cmp(&b.app)));
        for ago in (0..28).rev() {
            let Some(day) = self.today.checked_sub(Duration::days(ago)) else {
                continue;
            };
            let (words, sessions) = days.get(&day).copied().unwrap_or_default();
            report.days.push(DayActivity {
                date: day.to_string(),
                words,
                sessions,
            });
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn date(y: i32, m: u8, d: u8) -> Date {
        Date::from_calendar_date(y, m.try_into().unwrap(), d).unwrap()
    }
    fn aggregate(today: Date) -> Aggregate {
        Aggregate {
            now_ms: u64::MAX,
            today,
            records: BTreeMap::new(),
            dates_use_utc: false,
        }
    }
    fn dictation(words: u64, ms: Option<u64>) -> Session {
        let mut s = Session::new(Kind::Dictation);
        s.metrics.recognized_words = Some(words);
        s.metrics.audio_duration_ms = ms;
        s
    }
    #[test]
    fn weighted_pace_uses_raw_words_and_audio_only() {
        let mut a = aggregate(date(2026, 9, 20));
        let mut first = dictation(60, Some(60_000));
        first.text = "macro expansion ".repeat(500);
        a.push_on(&first, date(2026, 9, 20));
        a.push_on(&dictation(60, Some(30_000)), date(2026, 9, 20));
        let mut legacy = Session::new(Kind::Dictation);
        legacy.text = "three retained words".into();
        a.push_on(&legacy, date(2026, 9, 20));
        let r = a.finish();
        assert_eq!(r.words, 123);
        assert_eq!(r.sessions, 3);
        assert_eq!(r.measured_sessions, 2);
        assert_eq!(r.words_per_minute, Some(80.0));
    }
    #[test]
    fn calls_notes_and_missing_timing_do_not_invent_pace() {
        let mut a = aggregate(date(2026, 9, 20));
        for kind in [Kind::Call, Kind::Note] {
            let mut s = dictation(200, Some(1000));
            s.kind = kind;
            a.push_on(&s, date(2026, 9, 20));
        }
        a.push_on(&dictation(10, None), date(2026, 9, 20));
        let r = a.finish();
        assert_eq!(r.words, 10);
        assert_eq!(r.sessions, 1);
        assert_eq!(r.words_per_minute, None);
    }
    #[test]
    fn autosave_revisions_count_once_and_older_revision_cannot_replace() {
        let mut a = aggregate(date(2026, 9, 20));
        let mut s = dictation(10, None);
        s.updated_ms = 1;
        a.push_on(&s, date(2026, 9, 20));
        s.metrics.recognized_words = Some(12);
        s.updated_ms = 2;
        a.push_on(&s, date(2026, 9, 20));
        s.updated_ms = 0;
        s.metrics.recognized_words = Some(99);
        a.push_on(&s, date(2026, 9, 20));
        let r = a.finish();
        assert_eq!((r.sessions, r.words), (1, 12));
    }
    #[test]
    fn streaks_cross_month_year_and_allow_today_not_started() {
        let mut a = aggregate(date(2027, 1, 2));
        for day in [date(2026, 12, 30), date(2026, 12, 31), date(2027, 1, 1)] {
            a.push_on(&dictation(2, None), day);
        }
        let r = a.finish();
        assert_eq!((r.current_streak, r.longest_streak), (3, 3));
        assert_eq!(r.days.len(), 28);
        assert_eq!(r.days.last().unwrap().date, "2027-01-02");
        assert_eq!(r.days.last().unwrap().words, 0);
        let mut a = aggregate(date(2027, 1, 3));
        a.push_on(&dictation(2, None), date(2027, 1, 1));
        assert_eq!(a.finish().current_streak, 0);
    }
    #[test]
    fn local_midnight_assigns_day_before_utc_and_keeps_unknown_app() {
        let t = OffsetDateTime::from_unix_timestamp(1_767_225_600).unwrap(); // 2026-01-01 UTC
        let day = t.to_offset(UtcOffset::from_hms(-5, 0, 0).unwrap()).date();
        assert_eq!(day, date(2025, 12, 31));
        let mut a = aggregate(date(2026, 1, 1));
        let mut s = dictation(1, None);
        s.metrics.app = Some("C:/private/editor.exe".into());
        a.push_on(&s, day);
        let r = a.finish();
        assert!(r.apps[0].app.is_none());
        assert_eq!(r.current_streak, 1);
    }
    #[test]
    fn empty_latest_revision_and_future_timestamp_do_not_count() {
        let mut a = aggregate(date(2026, 9, 20));
        let mut s = dictation(10, None);
        s.updated_ms = 1;
        a.push_on(&s, date(2026, 9, 20));
        s.metrics.recognized_words = Some(0);
        s.updated_ms = 2;
        a.push_on(&s, date(2026, 9, 20));
        assert_eq!(a.finish().sessions, 0);
        let mut a = aggregate(date(2026, 9, 20));
        a.now_ms = 1;
        a.push_on(&dictation(2, None), date(2026, 9, 20));
        assert_eq!(a.finish().words, 0);
    }
}
