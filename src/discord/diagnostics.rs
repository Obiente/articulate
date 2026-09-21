//! Bounded local transport counters. Never accept audio, identities or free text.
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const NATIVE: &[&str] = &[
    "produced",
    "polled",
    "queue_full",
    "contention",
    "configuration_discard",
    "stop_flush",
    "max_depth",
    "queue_depth",
    "queue_capacity",
];
const TRANSPORT: &[&str] = &[
    "control_failures",
    "audio_failures",
    "requests",
    "packets",
    "max_control_gap_ms",
    "max_drain_gap_ms",
    "max_request_ms",
];

pub(super) fn validate(bytes: &[u8]) -> Result<Value> {
    ensure!(bytes.len() <= 4096, "Diagnostic payload too large");
    let value: Value = serde_json::from_slice(bytes)?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Invalid diagnostics"))?;
    ensure!(
        object
            .keys()
            .all(|key| ["version", "native", "transport"].contains(&key.as_str()))
            && value["version"] == 1,
        "Invalid diagnostic fields"
    );
    for (name, keys) in [("native", NATIVE), ("transport", TRANSPORT)] {
        if name == "native" && value[name].is_null() {
            continue;
        }
        let counters = value[name]
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Invalid counters"))?;
        ensure!(
            counters.len() == keys.len()
                && counters
                    .iter()
                    .all(|(key, number)| keys.contains(&key.as_str())
                        && number.as_u64().is_some_and(|n| n <= 9_007_199_254_740_991)),
            "Invalid diagnostic counters"
        );
    }
    Ok(value)
}

pub(super) struct Recorder {
    path: PathBuf,
    companion: Value,
    receiver: Value,
    last_save: Option<Instant>,
    last_gap_save: Option<Instant>,
}
impl Recorder {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            companion: Value::Null,
            receiver: Value::Null,
            last_save: None,
            last_gap_save: None,
        }
    }
    pub fn companion(&mut self, report: Value) {
        self.companion = report;
        if self
            .last_save
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(1))
        {
            self.save();
        }
    }
    pub fn failure(
        &mut self,
        reason: &'static str,
        previous: u64,
        incoming: u64,
        lost: u64,
        queued: usize,
        bytes: usize,
    ) {
        self.receiver = json!({"reason": reason, "previous_sequence": previous, "incoming_sequence": incoming, "lost_packets": lost, "queued_frames": queued, "queued_bytes": bytes, "at_ms": now()});
        // Recoverable gaps can arrive on every callback during a bad interval.
        // Keep the latest counters without doing disk I/O on every audio POST.
        // Fatal failures still persist immediately, even just after a gap.
        if reason == "sequence_loss" {
            if self
                .last_gap_save
                .is_some_and(|at| at.elapsed() < Duration::from_secs(1))
            {
                return;
            }
            self.last_gap_save = Some(Instant::now());
        }
        self.save();
    }
    fn save(&mut self) {
        self.last_save = Some(Instant::now());
        // One bounded snapshot, overwritten instead of growing an event log.
        let value = json!({"version":1, "updated_ms":now(), "companion":self.companion, "receiver":self.receiver});
        let Ok(bytes) = serde_json::to_vec(&value) else {
            return;
        };
        if bytes.len() > 8192 {
            return;
        }
        if let Some(parent) = self.path.parent()
            && std::fs::create_dir_all(parent).is_err()
        {
            return;
        }
        let _ = std::fs::write(&self.path, bytes);
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn writes_one_bounded_snapshot_and_preserves_failure_during_updates() {
        let path = std::env::temp_dir().join(format!(
            "articulate-audio-diagnostics-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut recorder = Recorder::new(path.clone());
        recorder.companion(json!({"version":1}));
        recorder.failure("sequence_loss", 10, 80, 69, 2, 2048);
        let bytes = std::fs::read(&path).unwrap();
        let saved: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved["receiver"]["lost_packets"], 69);
        assert_eq!(saved["receiver"]["reason"], "sequence_loss");
        assert!(bytes.len() < 8192);
        recorder.companion(json!({"version":1,"native":null}));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "Rapid reports do not cause repeated disk writes"
        );
        recorder.failure("sequence_loss", 80, 91, 79, 0, 0);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "Repeated recoverable gaps are rate limited too"
        );
        recorder.failure("sequence_order", 80, 1, 69, 0, 0);
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["receiver"]["reason"], "sequence_order");
        assert_eq!(saved["companion"]["native"], Value::Null);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn accepts_only_known_numeric_counters_and_never_free_text() {
        let mut report = json!({"version":1, "transport": TRANSPORT.iter().map(|key| ((*key).to_owned(), json!(0))).collect::<serde_json::Map<_,_>>()});
        assert!(validate(&serde_json::to_vec(&report).unwrap()).is_ok());
        report["transport"]["packets"] = json!("private text");
        assert!(validate(&serde_json::to_vec(&report).unwrap()).is_err());
        report["transport"]["packets"] = json!(0);
        report["transcript"] = json!("private text");
        assert!(validate(&serde_json::to_vec(&report).unwrap()).is_err());
        report.as_object_mut().unwrap().remove("transcript");
        report["transport"]["packets"] = json!(-1);
        assert!(validate(&serde_json::to_vec(&report).unwrap()).is_err());
        assert!(validate(&vec![b' '; 4097]).is_err());
    }
}
