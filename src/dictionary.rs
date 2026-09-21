use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Context {
    pub cues: Vec<String>,
    pub ignore_case: bool,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(from = "StoredEntry")]
pub struct Entry {
    pub heard: String,
    pub wanted: String,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub cues: Vec<String>,
    #[serde(default)]
    pub ignore_case: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contexts: Vec<Context>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

#[derive(Deserialize)]
struct StoredEntry {
    heard: String,
    wanted: String,
    #[serde(default)]
    app: Option<String>,
    #[serde(default)]
    cues: Vec<String>,
    #[serde(default)]
    ignore_case: Option<bool>,
    #[serde(default)]
    contexts: Vec<Context>,
    #[serde(default = "enabled")]
    enabled: bool,
}
impl From<StoredEntry> for Entry {
    fn from(value: StoredEntry) -> Self {
        // Legacy mention rules predate the capitalization control. ASR often
        // capitalizes "At" at sentence starts; preserve explicit newer choices.
        let ignore_case = value
            .ignore_case
            .unwrap_or_else(|| spoken_mention(&value.heard, &value.wanted));
        Self {
            heard: value.heard,
            wanted: value.wanted,
            app: value.app,
            cues: value.cues,
            ignore_case,
            contexts: value.contexts,
            enabled: value.enabled,
        }
    }
}

fn spoken_mention(heard: &str, wanted: &str) -> bool {
    let Some(handle) = wanted.strip_prefix('@') else {
        return false;
    };
    !handle.is_empty()
        && handle.chars().all(|c| c.is_alphanumeric() || c == '_')
        && heard.eq_ignore_ascii_case(&format!("at {handle}"))
}
fn enabled() -> bool {
    true
}
impl Entry {
    pub fn same_scope(&self, other: &Self) -> bool {
        self.same_app(other) && self.cues == other.cues && self.contexts == other.contexts
    }
    pub fn same_key(&self, other: &Self) -> bool {
        self.heard == other.heard && self.same_scope(other)
    }
    pub fn same_app(&self, other: &Self) -> bool {
        match (&self.app, &other.app) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            (None, None) => true,
            _ => false,
        }
    }
    pub fn same_correction(&self, other: &Self) -> bool {
        self.heard == other.heard && self.wanted == other.wanted && self.same_app(other)
    }
    pub fn policies(&self) -> impl Iterator<Item = (&[String], bool)> {
        std::iter::once((self.cues.as_slice(), self.ignore_case)).chain(
            self.contexts
                .iter()
                .map(|c| (c.cues.as_slice(), c.ignore_case)),
        )
    }
    #[allow(
        dead_code,
        reason = "Preserve merged vocabulary context inspection for React vocabulary controls"
    )]
    pub fn all_cues(&self) -> Vec<String> {
        let mut cues: Vec<_> = self
            .policies()
            .flat_map(|(cues, _)| cues.iter().cloned())
            .collect();
        cues.sort();
        cues.dedup();
        cues
    }
    #[allow(
        dead_code,
        reason = "Preserve merged vocabulary context inspection for React vocabulary controls"
    )]
    pub fn has_alternative_contexts(&self) -> bool {
        !self.contexts.is_empty()
    }
    /// Concrete policies are useful when a review must retain the precise rule
    /// that matched, rather than accidentally widening its capitalization.
    #[allow(
        dead_code,
        reason = "Preserve merged vocabulary context inspection for React vocabulary controls"
    )]
    pub fn variants(&self) -> impl Iterator<Item = Self> + '_ {
        self.policies().map(|(cues, ignore_case)| {
            let mut entry = self.clone();
            entry.cues = cues.to_vec();
            entry.ignore_case = ignore_case;
            entry.contexts.clear();
            entry
        })
    }
}

fn merge_contexts(existing: &mut Entry, incoming: &Entry) {
    let mut policies: Vec<Context> = Vec::new();
    for (cues, ignore_case) in existing.policies().chain(incoming.policies()) {
        let mut cues = cues.to_vec();
        cues.sort();
        cues.dedup();
        if let Some(policy) = policies.iter_mut().find(|p| {
            p.ignore_case == ignore_case
                && ((p.cues.is_empty() && cues.is_empty())
                    || (!p.cues.is_empty() && !cues.is_empty() && {
                        let mut joined = p.cues.clone();
                        joined.extend(cues.iter().cloned());
                        joined.sort();
                        joined.dedup();
                        joined.len() <= 8
                    }))
        }) {
            if policy.cues.is_empty() || cues.is_empty() {
                policy.cues.clear();
            } else {
                policy.cues.extend(cues);
                policy.cues.sort();
                policy.cues.dedup();
            }
        } else {
            policies.push(Context { cues, ignore_case });
        }
    }
    let first = policies.remove(0);
    existing.cues = first.cues;
    existing.ignore_case = first.ignore_case;
    existing.contexts = policies;
}

/// Merge existing duplicate rows without changing the union of their matching
/// policies. Disabled rows remain separate from enabled rows.
pub fn deduplicate(entries: &mut Vec<Entry>) -> bool {
    let before = entries.clone();
    let mut result: Vec<Entry> = Vec::with_capacity(entries.len());
    for entry in entries.drain(..) {
        if result.contains(&entry) {
            continue;
        }
        // Ambiguous replacement families retain their original row order.
        // Moving one of their clauses could change which spelling wins ties.
        let ambiguous = before.iter().any(|other| {
            (other.heard == entry.heard
                || ((other.policies().any(|(_, insensitive)| insensitive)
                    || entry.policies().any(|(_, insensitive)| insensitive))
                    && other.heard.to_lowercase() == entry.heard.to_lowercase()))
                && other.same_app(&entry)
                && other.wanted != entry.wanted
                && other.enabled
                && entry.enabled
        });
        if let Some(existing) = result
            .iter_mut()
            .find(|e| !ambiguous && e.same_correction(&entry) && e.enabled == entry.enabled)
        {
            merge_contexts(existing, &entry);
        } else {
            result.push(entry);
        }
    }
    *entries = result;
    *entries != before
}

/// Explicit edits replace only the exact row the editor opened. Adding an
/// existing correction adds its context policies instead of another row.
pub fn save(entries: &mut Vec<Entry>, entry: Entry, original: Option<&Entry>) -> Entry {
    if let Some(index) = original.and_then(|original| entries.iter().position(|e| e == original)) {
        // Equal-length, equally scoped rules resolve in saved order. Editing a
        // spelling must not move it behind a case-folded competing rule.
        entries[index] = entry.clone();
    } else {
        entries.push(entry.clone());
    }
    deduplicate(entries);
    entries
        .iter()
        .find(|e| e.same_correction(&entry) && e.enabled == entry.enabled)
        .unwrap()
        .clone()
}
pub fn app_scope(value: &str) -> anyhow::Result<Option<String>> {
    let value = value.trim().to_lowercase();
    if value.is_empty() {
        return Ok(None);
    }
    anyhow::ensure!(
        value.len() <= 100
            && value.ends_with(".exe")
            && !value.chars().any(|c| c.is_control() || "/\\:".contains(c)),
        "Use an app name such as code.exe, without a folder path"
    );
    Ok(Some(value))
}
pub fn cue_words(value: &str) -> anyhow::Result<Vec<String>> {
    let mut cues: Vec<_> = value
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    anyhow::ensure!(
        cues.len() <= 8
            && cues.iter().all(|s| s.len() <= 40
                && s.chars()
                    .all(|c| c.is_alphanumeric() || c == ' ' || c == '-')),
        "Use up to eight short cues, separated by commas"
    );
    cues.sort();
    cues.dedup();
    Ok(cues)
}

pub fn validate(heard: &str, wanted: &str) -> anyhow::Result<Entry> {
    let heard = heard.trim();
    let wanted = wanted.trim();
    anyhow::ensure!(
        !heard.is_empty() && !wanted.is_empty(),
        "Fill in both words or phrases"
    );
    anyhow::ensure!(heard != wanted, "The corrected spelling must be different");
    anyhow::ensure!(
        heard.len() <= 160 && wanted.len() <= 160,
        "Keep dictionary entries under 160 bytes"
    );
    anyhow::ensure!(
        !heard.chars().chain(wanted.chars()).any(char::is_control),
        "Dictionary entries must be a single line"
    );
    Ok(Entry {
        heard: heard.to_owned(),
        wanted: wanted.to_owned(),
        app: None,
        cues: Vec::new(),
        ignore_case: spoken_mention(heard, wanted),
        contexts: Vec::new(),
        enabled: true,
    })
}

fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\'' || c == '’'
}

// Match original input only, longest first, at Unicode word boundaries.
// Exact-case matching is the default; entries may opt into case-insensitive matching.
#[cfg(test)]
pub fn apply(text: &str, entries: &[Entry]) -> (String, usize) {
    apply_in(text, entries, None)
}

fn nearby(text: &str, at: usize, end: usize, cues: &[String]) -> bool {
    if cues.is_empty() {
        return true;
    }
    let mut left: String = text[..at]
        .chars()
        .rev()
        .take(120)
        .take_while(|c| !".!?\n\r".contains(*c))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let mut right: String = text[end..]
        .chars()
        .take(120)
        .take_while(|c| !".!?\n\r".contains(*c))
        .collect();
    // A clipped word must not acquire an artificial word boundary at the edge
    // of the context window. A sentinel keeps boundary matching honest.
    if text[..at - left.len()]
        .chars()
        .next_back()
        .is_some_and(word)
    {
        left.insert(0, '_');
    }
    if text[end + right.len()..].chars().next().is_some_and(word) {
        right.push('_');
    }
    // Exclude the matched phrase itself: a cue must supply additional context.
    [left.to_lowercase(), right.to_lowercase()]
        .iter()
        .any(|context| {
            cues.iter().any(|cue| {
                context.match_indices(cue).any(|(i, _)| {
                    (i == 0 || !context[..i].chars().next_back().is_some_and(word))
                        && !context[i + cue.len()..].chars().next().is_some_and(word)
                })
            })
        })
}

pub(crate) fn output_in_context(entry: &Entry, text: &str) -> bool {
    !entry.wanted.is_empty()
        && text.match_indices(&entry.wanted).any(|(at, _)| {
            let end = at + entry.wanted.len();
            (at == 0 || !text[..at].chars().next_back().is_some_and(word))
                && !text[end..].chars().next().is_some_and(word)
                && entry
                    .policies()
                    .any(|(cues, _)| nearby(text, at, end, cues))
        })
}

pub fn apply_in(text: &str, entries: &[Entry], app: Option<&str>) -> (String, usize) {
    let mut entries: Vec<_> = entries
        .iter()
        .filter(|e| {
            e.enabled
                && !e.heard.is_empty()
                && e.app
                    .as_deref()
                    .is_none_or(|scope| app.is_some_and(|app| app.eq_ignore_ascii_case(scope)))
        })
        .flat_map(|entry| {
            entry
                .policies()
                .map(move |(cues, ignore_case)| (entry, cues, ignore_case))
        })
        .collect();
    entries.sort_by_key(|(e, cues, _)| {
        std::cmp::Reverse((e.heard.chars().count(), e.app.is_some(), !cues.is_empty()))
    });
    // Prepare case folding once per rule, not once for every input character.
    let entries: Vec<_> = entries
        .into_iter()
        .map(|(e, cues, ignore_case)| {
            (
                e,
                e.heard.chars().count(),
                ignore_case.then(|| e.heard.to_lowercase()),
                cues,
            )
        })
        .collect();
    let mut output = String::with_capacity(text.len());
    let mut at = 0;
    let mut changed = 0;
    while at < text.len() {
        let found = entries.iter().find_map(|(e, chars, folded, cues)| {
            let len = if folded.is_some() {
                text[at..].chars().take(*chars).map(char::len_utf8).sum()
            } else {
                e.heard.len()
            };
            let candidate = text.get(at..at + len)?;
            let matches = if let Some(folded) = folded {
                candidate.to_lowercase() == *folded
            } else {
                candidate == e.heard
            };
            (matches
                && (at == 0 || !text[..at].chars().next_back().is_some_and(word))
                && !text[at + len..].chars().next().is_some_and(word)
                && nearby(text, at, at + len, cues))
            .then_some((*e, len))
        });
        if let Some((entry, len)) = found {
            output.push_str(&entry.wanted);
            at += len;
            changed += 1;
        } else {
            let c = text[at..].chars().next().unwrap();
            output.push(c);
            at += c.len_utf8();
        }
    }
    (output, changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn e(a: &str, b: &str) -> Entry {
        validate(a, b).unwrap()
    }
    #[test]
    fn duplicates_merge_exact_matching_union_without_crossing_case_contexts() {
        let mut a = e("mercury", "Mercury");
        a.cues = vec!["planet".into()];
        let mut b = a.clone();
        b.cues = vec!["function".into()];
        b.ignore_case = true;
        let before = vec![a, b];
        let mut merged = before.clone();
        assert!(deduplicate(&mut merged));
        assert_eq!(merged.len(), 1);
        assert!(!deduplicate(&mut merged));
        for text in [
            "mercury planet",
            "MERCURY planet",
            "mercury function",
            "MERCURY function",
            "mercury",
            "function. MERCURY planet",
        ] {
            assert_eq!(apply(text, &before), apply(text, &merged), "{text}");
        }
        let restored: Vec<Entry> =
            serde_json::from_slice(&serde_json::to_vec(&merged).unwrap()).unwrap();
        assert!(restored == merged);
    }
    #[test]
    fn merge_preserves_apps_replacements_disabled_choices_and_explicit_edits() {
        let a = e("Jon", "John");
        let mut other_app = a.clone();
        other_app.app = Some("editor.exe".into());
        let mut disabled = a.clone();
        disabled.enabled = false;
        let other_spelling = e("Jon", "Jonathan");
        let mut entries = vec![a.clone(), other_app, disabled, other_spelling];
        assert!(!deduplicate(&mut entries));
        let mut edited = a.clone();
        edited.cues = vec!["team".into()];
        save(&mut entries, edited, Some(&a));
        assert_eq!(entries.len(), 4);
        assert!(entries.iter().any(|e| !e.enabled && e.cues.is_empty()));
        assert!(
            entries
                .iter()
                .any(|e| e.enabled && e.wanted == "John" && e.app.is_none() && e.cues == ["team"])
        );
    }
    #[test]
    fn context_union_keeps_each_clause_editable_under_cue_limit() {
        let mut a = e("Jon", "John");
        a.cues = (0..8).map(|i| format!("team{i}")).collect();
        let mut b = a.clone();
        b.cues = (8..16).map(|i| format!("team{i}")).collect();
        let mut entries = vec![a, b];
        deduplicate(&mut entries);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].policies().all(|(cues, _)| cues.len() <= 8));
        assert_eq!(entries[0].all_cues().len(), 16);
    }
    #[test]
    fn competing_replacement_priority_survives_migration() {
        let mut a = e("Jon", "John");
        a.cues = vec!["planet".into()];
        let mut other = e("Jon", "Jonathan");
        other.cues = vec!["team".into()];
        let mut b = a.clone();
        b.cues = vec!["team".into()];
        let before = vec![a, other, b];
        let mut entries = before.clone();
        deduplicate(&mut entries);
        assert_eq!(apply("Jon team", &entries), apply("Jon team", &before));
        assert!(entries == before);
    }
    #[test]
    fn folded_competing_replacement_priority_survives_migration() {
        let mut first = e("Jon", "John");
        first.cues = vec!["planet".into()];
        first.ignore_case = true;
        let mut competitor = e("jon", "Jonathan");
        competitor.cues = vec!["team".into()];
        competitor.ignore_case = true;
        let mut last = first.clone();
        last.cues = vec!["team".into()];
        let original = vec![first, competitor, last];
        let mut entries = original.clone();
        assert!(!deduplicate(&mut entries));
        assert!(entries == original);
        assert_eq!(apply("Jon team", &entries).0, "Jonathan team");
    }
    #[test]
    fn legacy_mentions_handle_sentence_case_and_explicit_choices_survive() {
        let legacy: Entry =
            serde_json::from_str(r#"{"heard":"at ExampleHandle","wanted":"@ExampleHandle"}"#)
                .unwrap();
        assert_eq!(
            apply("At ExampleHandle. Hello at ExampleHandle.", &[legacy]).0,
            "@ExampleHandle. Hello @ExampleHandle."
        );
        let explicit: Entry = serde_json::from_str(
            r#"{"heard":"at ExampleHandle","wanted":"@ExampleHandle","ignore_case":false}"#,
        )
        .unwrap();
        assert_eq!(apply("At ExampleHandle.", &[explicit]).1, 0);
        assert!(
            validate("at ExampleHandle", "@ExampleHandle")
                .unwrap()
                .ignore_case
        );
        assert!(!validate("at home", "@office").unwrap().ignore_case);
    }
    #[test]
    fn context_rules_require_the_app_and_nearby_cue() {
        let mut scoped = e("rust", "Rust");
        scoped.app = Some("code.exe".into());
        scoped.cues = vec!["crate".into()];
        scoped.ignore_case = true;
        assert_eq!(
            apply_in("A rust crate", &[scoped.clone()], Some("CODE.EXE")).0,
            "A Rust crate"
        );
        assert_eq!(
            apply_in("A rust crate", &[scoped.clone()], Some("chat.exe")).1,
            0
        );
        assert_eq!(
            apply_in("The gate has rust.", &[scoped.clone()], Some("code.exe")).1,
            0
        );
        assert_eq!(
            apply_in(
                "A crate. The gate has rust.",
                &[scoped.clone()],
                Some("code.exe")
            )
            .1,
            0
        );
        assert_eq!(apply_in("rust crates", &[scoped], Some("code.exe")).1, 0);
    }
    #[test]
    fn scoped_rules_win_and_old_dictionaries_load() {
        let global = e("mercury", "Mercury");
        let mut scoped = e("mercury", "mercury() ");
        scoped.app = Some("code.exe".into());
        assert_eq!(
            apply_in(
                "mercury",
                &[global.clone(), scoped.clone()],
                Some("code.exe")
            )
            .0,
            "mercury()"
        );
        scoped.enabled = false;
        assert_eq!(
            apply_in("mercury", &[global, scoped], Some("code.exe")).0,
            "Mercury"
        );
        let old: Entry = serde_json::from_str(r#"{"heard":"Jon","wanted":"John"}"#).unwrap();
        assert!(old.enabled && old.app.is_none() && old.cues.is_empty());
        assert!(app_scope("C:\\private\\editor.exe").is_err());
    }
    #[test]
    fn replacement_is_not_substring_or_cascading() {
        let pairs = vec![e("flow", "Flow"), e("Flow", "wrong"), e("Ann", "Anne")];
        assert_eq!(
            apply("flow workflow Ann Anna Ann's", &pairs),
            ("Flow workflow Anne Anna Ann's".into(), 2)
        );
    }
    #[test]
    fn unicode_and_longest_phrase() {
        let pairs = vec![e("café", "Café"), e("New York", "NYC"), e("New", "old")];
        assert_eq!(
            apply("café, décafé New York!", &pairs).0,
            "Café, décafé NYC!"
        );
    }
    #[test]
    fn rejects_invalid_pairs() {
        assert!(validate("", "name").is_err());
        assert!(validate("a", "a").is_err());
        assert!(validate("a\nb", "x").is_err());
    }
    #[test]
    fn clipped_context_does_not_turn_substrings_into_cues() {
        let mut rule = e("rust", "Rust");
        rule.cues = vec!["crate".into()];
        let left = format!("xcrate{}rust", " ".repeat(115));
        let right = format!("rust{}cratex", " ".repeat(115));
        assert_eq!(apply(&left, &[rule.clone()]).1, 0);
        assert_eq!(apply(&right, &[rule]).1, 0);
    }
    #[test]
    fn case_insensitive_matching_handles_different_utf8_widths() {
        let mut rule = e("K", "kelvin");
        rule.ignore_case = true;
        assert_eq!(apply("k K sky", &[rule]).0, "kelvin kelvin sky");
    }
}
