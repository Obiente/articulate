use serde::{Deserialize, Serialize};

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
        self.app == other.app && self.cues == other.cues
    }
    pub fn same_key(&self, other: &Self) -> bool {
        self.heard == other.heard && self.same_scope(other)
    }
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
                && nearby(text, at, end, &entry.cues)
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
        .collect();
    entries.sort_by_key(|e| {
        std::cmp::Reverse((e.heard.chars().count(), e.app.is_some(), !e.cues.is_empty()))
    });
    // Prepare case folding once per rule, not once for every input character.
    let entries: Vec<_> = entries
        .into_iter()
        .map(|e| {
            (
                e,
                e.heard.chars().count(),
                e.ignore_case.then(|| e.heard.to_lowercase()),
            )
        })
        .collect();
    let mut output = String::with_capacity(text.len());
    let mut at = 0;
    let mut changed = 0;
    while at < text.len() {
        let found = entries.iter().find_map(|(e, chars, folded)| {
            let len = if e.ignore_case {
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
                && nearby(text, at, at + len, &e.cues))
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
