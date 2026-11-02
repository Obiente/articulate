//! Timestamped audio interruptions recorded with a call.
//!
//! Capture reports when audio could not be recorded: a device that was
//! disconnected or invalidated until it reopened, audio dropped because
//! processing could not drain it in time, Discord packets that never arrived,
//! and audio skipped because transcription fell behind. Times are milliseconds
//! from the start of the recording, like transcript rows.
use serde::{Deserialize, Serialize};

/// Stored interruptions per session. Later reports extend the last one.
pub const MAX_STORED: usize = 2000;
/// Reports of the same kind closer than this are one interruption.
const JOIN_MS: u64 = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Microphone,
    SystemAudio,
    Participants,
    Processing,
    /// Written by a newer version.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    /// The device stopped delivering audio or was invalidated.
    DeviceLost,
    /// Audio was dropped because the capture buffer was full.
    Overflow,
    /// Discord packets were missing.
    PacketLoss,
    /// Audio arrived too late or processing stopped keeping up.
    FellBehind,
    /// Written by a newer version.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interruption {
    pub start_ms: u64,
    pub end_ms: u64,
    pub source: Source,
    pub cause: Cause,
    /// Missing packets for packet loss and late audio, otherwise zero.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub count: u64,
    /// The device has not come back yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ongoing: bool,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

impl Interruption {
    pub fn new(start_ms: u64, end_ms: u64, source: Source, cause: Cause) -> Self {
        Self {
            start_ms,
            end_ms: end_ms.max(start_ms),
            source,
            cause,
            count: 0,
            ongoing: false,
        }
    }

    /// What happened, without times: "microphone disconnected".
    pub fn reason(&self) -> String {
        let packets = |noun: &str| {
            if self.count == 1 {
                format!("1 {noun} packet")
            } else {
                format!("{} {noun} packets", self.count)
            }
        };
        match (self.source, self.cause) {
            (Source::Microphone, Cause::DeviceLost) => "microphone disconnected".into(),
            (Source::SystemAudio, Cause::DeviceLost) => {
                "system audio device disconnected or changed".into()
            }
            (Source::Microphone, Cause::Overflow) => {
                "microphone audio dropped while processing caught up".into()
            }
            (Source::SystemAudio, Cause::Overflow) => {
                "system audio dropped while processing caught up".into()
            }
            (Source::Participants, Cause::PacketLoss) if self.count > 0 => {
                format!("{} lost", packets("Discord audio"))
            }
            (Source::Participants, Cause::PacketLoss) => "Discord audio packets lost".into(),
            (Source::Participants, Cause::FellBehind) if self.count > 0 => {
                format!("{} arrived too late", packets("Discord audio"))
            }
            (Source::Participants, Cause::FellBehind) => {
                "participant audio could not be processed in time".into()
            }
            (_, Cause::FellBehind) => "transcription fell behind and capture stopped".into(),
            _ => "audio was not recorded".into(),
        }
    }

    /// "Audio interrupted 12:03–12:07: microphone disconnected".
    pub fn message(&self) -> String {
        let reason = self.reason();
        if self.ongoing {
            format!(
                "Audio interrupted from {}: {reason} (reconnecting)",
                clock(self.start_ms)
            )
        } else if self.end_ms / 1000 <= self.start_ms / 1000 {
            format!("Audio interrupted at {}: {reason}", clock(self.start_ms))
        } else {
            format!(
                "Audio interrupted {}–{}: {reason}",
                clock(self.start_ms),
                clock(self.end_ms)
            )
        }
    }
}

/// Add or update one interruption. An ongoing report is replaced by its
/// later updates; nearby reports of the same kind are joined.
pub fn record(list: &mut Vec<Interruption>, next: Interruption) {
    if let Some(existing) = list.iter_mut().rev().find(|existing| {
        existing.source == next.source
            && existing.cause == next.cause
            && (existing.start_ms == next.start_ms
                || (next.start_ms >= existing.start_ms
                    && next.start_ms <= existing.end_ms.saturating_add(JOIN_MS)))
    }) {
        if existing.start_ms == next.start_ms && (existing.ongoing || next.ongoing) {
            // The same open interruption, reported again as it progresses.
            existing.end_ms = existing.end_ms.max(next.end_ms);
            existing.count = existing.count.max(next.count);
            existing.ongoing = next.ongoing;
        } else {
            existing.end_ms = existing.end_ms.max(next.end_ms);
            existing.count = existing.count.saturating_add(next.count);
            existing.ongoing = next.ongoing;
        }
        return;
    }
    if list.len() >= MAX_STORED {
        if let Some(last) = list.last_mut() {
            last.end_ms = last.end_ms.max(next.end_ms);
            last.count = last.count.saturating_add(next.count);
        }
        return;
    }
    let at = list.partition_point(|existing| existing.start_ms <= next.start_ms);
    list.insert(at, next);
}

/// Close interruptions still open when a recording ends.
pub fn close(list: &mut [Interruption], end_ms: u64) {
    for interruption in list.iter_mut().filter(|i| i.ongoing) {
        interruption.ongoing = false;
        interruption.end_ms = interruption.end_ms.max(end_ms).max(interruption.start_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_name_the_cause_and_time_range() {
        let lost = Interruption::new(723_000, 727_000, Source::Microphone, Cause::DeviceLost);
        assert_eq!(
            lost.message(),
            "Audio interrupted 12:03–12:07: microphone disconnected"
        );
        let mut packets = Interruption::new(5_000, 5_200, Source::Participants, Cause::PacketLoss);
        packets.count = 3;
        assert_eq!(
            packets.message(),
            "Audio interrupted at 00:05: 3 Discord audio packets lost"
        );
        let mut open =
            Interruption::new(3_723_000, 3_723_000, Source::SystemAudio, Cause::DeviceLost);
        open.ongoing = true;
        assert!(open.message().starts_with("Audio interrupted from 1:02:03"));
    }

    #[test]
    fn ongoing_reports_update_in_place_and_nearby_reports_join() {
        let mut list = Vec::new();
        let mut open = Interruption::new(10_000, 11_000, Source::Microphone, Cause::DeviceLost);
        open.ongoing = true;
        record(&mut list, open.clone());
        open.end_ms = 15_000;
        open.ongoing = false;
        record(&mut list, open);
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].end_ms, list[0].ongoing), (15_000, false));

        let mut loss = Interruption::new(20_000, 20_100, Source::Participants, Cause::PacketLoss);
        loss.count = 2;
        record(&mut list, loss.clone());
        loss.start_ms = 21_000;
        loss.end_ms = 21_500;
        record(&mut list, loss.clone());
        assert_eq!(list.len(), 2);
        assert_eq!((list[1].end_ms, list[1].count), (21_500, 4));
        loss.start_ms = 40_000;
        loss.end_ms = 40_000;
        record(&mut list, loss);
        // A different kind at an earlier time stays separate and ordered.
        record(
            &mut list,
            Interruption::new(1_000, 2_000, Source::SystemAudio, Cause::Overflow),
        );
        assert_eq!(
            list.iter().map(|i| i.start_ms).collect::<Vec<_>>(),
            [1_000, 10_000, 20_000, 40_000]
        );
        let mut open = Interruption::new(50_000, 50_000, Source::Microphone, Cause::DeviceLost);
        open.ongoing = true;
        record(&mut list, open);
        close(&mut list, 55_000);
        assert_eq!((list[4].end_ms, list[4].ongoing), (55_000, false));
    }

    #[test]
    fn stored_form_tolerates_newer_values_and_stays_small() {
        let parsed: Interruption = serde_json::from_str(
            r#"{"start_ms":1,"end_ms":2,"source":"bluetooth","cause":"thermal","extra":true}"#,
        )
        .unwrap();
        assert_eq!(
            (parsed.source, parsed.cause),
            (Source::Unknown, Cause::Unknown)
        );
        assert_eq!(
            parsed.message(),
            "Audio interrupted at 00:00: audio was not recorded"
        );
        let json = serde_json::to_string(&Interruption::new(
            0,
            1,
            Source::Microphone,
            Cause::Overflow,
        ))
        .unwrap();
        assert!(!json.contains("count") && !json.contains("ongoing"));
    }
}
