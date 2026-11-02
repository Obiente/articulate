//! Optional automatic deletion of old sessions.
//!
//! A session qualifies when it was last changed more than `days` days ago and
//! its kind is in scope. Pinned sessions, topic documents and recordings filed
//! into a topic are always exempt, as are sessions the caller marks as active.
use super::Kind;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const MAX_DAYS: u32 = 36_500;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub enabled: bool,
    pub days: u32,
    pub dictations: bool,
    pub calls: bool,
    pub notes: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            enabled: false,
            days: 90,
            dictations: true,
            calls: true,
            notes: true,
        }
    }
}

impl Policy {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=MAX_DAYS).contains(&self.days),
            "Choose a retention period between 1 and {MAX_DAYS} days."
        );
        Ok(())
    }

    pub fn includes(&self, kind: Kind) -> bool {
        match kind {
            Kind::Dictation => self.dictations,
            Kind::Call => self.calls,
            Kind::Note => self.notes,
        }
    }

    /// Sessions last changed before this instant are eligible.
    pub fn cutoff_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(u64::from(self.days).saturating_mul(DAY_MS))
    }
}

/// The facts the policy needs about one session.
pub(crate) struct Facts<'a> {
    pub id: &'a str,
    pub kind: Kind,
    pub updated_ms: u64,
    pub pinned: bool,
    pub is_collection: bool,
    pub topic_id: Option<&'a str>,
}

pub(crate) fn eligible(policy: &Policy, facts: &Facts<'_>, now_ms: u64, active: &[String]) -> bool {
    policy.includes(facts.kind)
        && facts.updated_ms < policy.cutoff_ms(now_ms)
        && !facts.pinned
        && !facts.is_collection
        && facts.topic_id.is_none()
        && !active.iter().any(|id| id == facts.id)
}

/// Outcome of applying a policy.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Applied {
    pub deleted: Vec<String>,
    /// Sessions from the confirmed preview that no longer qualified.
    pub skipped: Vec<String>,
    pub failed: Vec<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exemptions_and_scope_are_respected() {
        let policy = Policy {
            enabled: true,
            days: 30,
            calls: false,
            ..Default::default()
        };
        let now = 100 * DAY_MS;
        let old = |kind| Facts {
            id: "a",
            kind,
            updated_ms: 10 * DAY_MS,
            pinned: false,
            is_collection: false,
            topic_id: None,
        };
        assert!(eligible(&policy, &old(Kind::Dictation), now, &[]));
        assert!(eligible(&policy, &old(Kind::Note), now, &[]));
        assert!(!eligible(&policy, &old(Kind::Call), now, &[]));
        assert!(!eligible(
            &policy,
            &old(Kind::Dictation),
            now,
            &["a".into()]
        ));
        for facts in [
            Facts {
                pinned: true,
                ..old(Kind::Dictation)
            },
            Facts {
                is_collection: true,
                ..old(Kind::Note)
            },
            Facts {
                topic_id: Some("t"),
                ..old(Kind::Dictation)
            },
            Facts {
                updated_ms: 80 * DAY_MS,
                ..old(Kind::Dictation)
            },
        ] {
            assert!(!eligible(&policy, &facts, now, &[]));
        }
        assert!(
            Policy {
                days: 0,
                ..policy.clone()
            }
            .validate()
            .is_err()
        );
        assert!(policy.validate().is_ok());
    }
}
