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
    pub sound_alike: bool,
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
    #[serde(default)]
    sound_alike: Option<bool>,
}
impl From<StoredEntry> for Entry {
    fn from(value: StoredEntry) -> Self {
        // Legacy mention rules predate the capitalization control. ASR often
        // capitalizes "At" at sentence starts; preserve explicit newer choices.
        let ignore_case = value
            .ignore_case
            .unwrap_or_else(|| spoken_mention(&value.heard, &value.wanted));
        let sound_alike = value
            .sound_alike
            .unwrap_or_else(|| crate::sound_alike::is_distinctive(&value.wanted));
        Self {
            sound_alike,
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
    existing.sound_alike &= incoming.sound_alike;
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
        sound_alike: crate::sound_alike::is_distinctive(wanted),
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

/// Exact matching remains available for callers that explicitly need saved pairs.
pub fn apply_in(text: &str, entries: &[Entry], app: Option<&str>) -> (String, usize) {
    let result = apply_with_sound(text, entries, app, false);
    (result.text, result.changed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    Exact,
    SoundAlike,
}

pub struct Match {
    pub entry: usize,
    pub range: std::ops::Range<usize>,
    pub kind: MatchKind,
}

#[derive(Clone)]
pub struct AppliedSound {
    pub entry: Entry,
    pub heard: String,
}

pub struct Applied {
    pub text: String,
    pub changed: usize,
    pub sounds: Vec<AppliedSound>,
    pub matches: Vec<Match>,
}

struct Rule {
    entry: usize,
    chars: usize,
    folded: Option<String>,
    cues: Vec<String>,
}

#[derive(Default)]
struct Compiled {
    entries: Vec<Entry>,
    exact: std::collections::HashMap<char, Vec<Rule>>,
    wanted: std::collections::HashMap<char, Vec<(usize, String, usize)>>,
    sounds: std::collections::HashMap<(u8, usize), Vec<(usize, crate::sound_alike::Key)>>,
    sound_initials: std::collections::HashSet<u8>,
}

fn initial(text: &str) -> char {
    text.chars().next().unwrap().to_lowercase().next().unwrap()
}
fn scoped(entry: &Entry, app: Option<&str>) -> bool {
    entry.enabled
        && entry
            .app
            .as_deref()
            .is_none_or(|scope| app.is_some_and(|app| app.eq_ignore_ascii_case(scope)))
}
fn phrase_len(
    text: &str,
    at: usize,
    phrase: &str,
    chars: usize,
    folded: Option<&str>,
) -> Option<usize> {
    let len = if folded.is_some() {
        text[at..].chars().take(chars).map(char::len_utf8).sum()
    } else {
        phrase.len()
    };
    let candidate = text.get(at..at + len)?;
    let matches = folded.map_or(candidate == phrase, |s| candidate.to_lowercase() == s);
    (matches && !text[at + len..].chars().next().is_some_and(word)).then_some(len)
}
impl Compiled {
    fn new(entries: &[Entry]) -> Self {
        let mut result = Self {
            entries: entries.to_vec(),
            ..Self::default()
        };
        for (index, entry) in entries.iter().enumerate() {
            if !entry.wanted.is_empty() {
                result
                    .wanted
                    .entry(initial(&entry.wanted))
                    .or_default()
                    .push((
                        index,
                        entry.wanted.to_lowercase(),
                        entry.wanted.chars().count(),
                    ));
            }
            if !entry.enabled || entry.heard.is_empty() {
                continue;
            }
            for (cues, ignore_case) in entry.policies() {
                result
                    .exact
                    .entry(initial(&entry.heard))
                    .or_default()
                    .push(Rule {
                        entry: index,
                        chars: entry.heard.chars().count(),
                        folded: ignore_case.then(|| entry.heard.to_lowercase()),
                        cues: cues.to_vec(),
                    });
            }
            if entry.sound_alike && crate::sound_alike::is_distinctive(&entry.wanted) {
                for phrase in [&entry.wanted, &entry.heard] {
                    if phrase.split_whitespace().any(crate::sound_alike::protected) {
                        continue;
                    }
                    if let Some(key) = crate::sound_alike::Key::new(phrase) {
                        result.sound_initials.insert(key.bucket().0);
                        result
                            .sounds
                            .entry(key.bucket())
                            .or_default()
                            .push((index, key));
                    }
                }
            }
        }
        for rules in result.exact.values_mut() {
            rules.sort_by_key(|r| {
                std::cmp::Reverse((r.chars, entries[r.entry].app.is_some(), !r.cues.is_empty()))
            });
        }
        result
    }

    fn apply(&self, text: &str, app: Option<&str>, enabled: bool) -> Applied {
        // Reserve exact matches and saved outputs before considering any wider
        // sound span. A fallback may never swallow a later exact match.
        let mut exact = std::collections::BTreeMap::new();
        let mut reserved = vec![false; text.len()];
        for (at, c) in text.char_indices() {
            if at > 0 && text[..at].chars().next_back().is_some_and(word) {
                continue;
            }
            let first = c.to_lowercase().next().unwrap();
            if let Some(rules) = self.exact.get(&first) {
                for rule in rules {
                    let entry = &self.entries[rule.entry];
                    if !scoped(entry, app) {
                        continue;
                    }
                    if let Some(len) =
                        phrase_len(text, at, &entry.heard, rule.chars, rule.folded.as_deref())
                        && nearby(text, at, at + len, &rule.cues)
                    {
                        exact.insert(at, (rule.entry, len));
                        reserved[at..at + len].fill(true);
                        break;
                    }
                }
            }
            if enabled && let Some(outputs) = self.wanted.get(&first) {
                for (index, folded, chars) in outputs {
                    if let Some(len) =
                        phrase_len(text, at, &self.entries[*index].wanted, *chars, Some(folded))
                    {
                        reserved[at..at + len].fill(true);
                    }
                }
            }
        }
        let mut result = Applied {
            text: String::with_capacity(text.len()),
            changed: 0,
            sounds: Vec::new(),
            matches: Vec::new(),
        };
        let mut at = 0;
        while at < text.len() {
            let found = exact
                .get(&at)
                .map(|(entry, len)| (*entry, *len, MatchKind::Exact))
                .or_else(|| {
                    (enabled
                        && !self.sounds.is_empty()
                        && !reserved[at]
                        && (at == 0 || !text[..at].chars().next_back().is_some_and(word)))
                    .then(|| self.sound_at(text, at, app, &reserved))
                    .flatten()
                    .map(|(entry, len)| (entry, len, MatchKind::SoundAlike))
                });
            if let Some((index, len, kind)) = found {
                let entry = &self.entries[index];
                result.text.push_str(&entry.wanted);
                if kind == MatchKind::SoundAlike {
                    result.sounds.push(AppliedSound {
                        entry: entry.clone(),
                        heard: text[at..at + len].into(),
                    });
                }
                result.matches.push(Match {
                    entry: index,
                    range: at..at + len,
                    kind,
                });
                at += len;
                result.changed += 1;
            } else {
                let c = text[at..].chars().next().unwrap();
                result.text.push(c);
                at += c.len_utf8();
            }
        }
        result
    }

    fn sound_at(
        &self,
        text: &str,
        at: usize,
        app: Option<&str>,
        reserved: &[bool],
    ) -> Option<(usize, usize)> {
        let mut end = at;
        let mut best: Option<(usize, usize, f32)> = None;
        let mut tied = false;
        for count in 1..=4 {
            let start = end;
            end += text[end..]
                .chars()
                .take_while(|c| word(*c))
                .map(char::len_utf8)
                .sum::<usize>();
            if end == start
                || end - at > 160
                || reserved[start..end].iter().any(|b| *b)
                || crate::sound_alike::protected(&text[start..end])
            {
                break;
            }
            let phrase = &text[at..end];
            if !(count == 1 && crate::sound_alike::common(phrase))
                && let Some(key) = crate::sound_alike::Key::new(phrase)
            {
                let (first, size) = key.bucket();
                if !self.sound_initials.contains(&first) {
                    break;
                }
                // A length difference alone must fit within the score budget.
                let allowance = (size as f32 * 0.12) as usize;
                for length in size.saturating_sub(allowance)..=size + allowance {
                    for (index, target) in self.sounds.get(&(first, length)).into_iter().flatten() {
                        let entry = &self.entries[*index];
                        // A case-only version of the literal heard phrase
                        // still belongs to its explicit capitalization policy.
                        if !scoped(entry, app) || phrase.eq_ignore_ascii_case(&entry.heard) {
                            continue;
                        }
                        let score = key.score(target);
                        if score < crate::sound_alike::THRESHOLD
                            || !entry
                                .policies()
                                .any(|(cues, _)| nearby(text, at, end, cues))
                        {
                            continue;
                        }
                        match best {
                            Some((old, old_len, old_score))
                                if (score - old_score).abs() < 0.00001 =>
                            {
                                if self.entries[old].wanted != entry.wanted || old_len != end - at {
                                    tied = true;
                                } else if (entry.app.is_some(), !entry.cues.is_empty())
                                    > (
                                        self.entries[old].app.is_some(),
                                        !self.entries[old].cues.is_empty(),
                                    )
                                {
                                    best = Some((*index, end - at, score));
                                }
                            }
                            Some((_, _, old_score)) if score < old_score => (),
                            _ => {
                                best = Some((*index, end - at, score));
                                tied = false;
                            }
                        }
                    }
                }
            }
            let spaces = text[end..].bytes().take_while(|b| *b == b' ').count();
            if spaces == 0 {
                break;
            }
            end += spaces;
        }
        if tied {
            None
        } else {
            best.map(|(entry, len, _)| (entry, len))
        }
    }
}

thread_local! {
    static MATCHER: std::cell::RefCell<Compiled> = std::cell::RefCell::new(Compiled::default());
}

/// Cached per calling thread. Equality checks detect edits, imports, learning,
/// undo and reorder without requiring every mutation site to invalidate keys.
/// The cache holds only the current vocabulary, never transcript text.
pub fn apply_with_sound(
    text: &str,
    entries: &[Entry],
    app: Option<&str>,
    enabled: bool,
) -> Applied {
    MATCHER.with_borrow_mut(|cache| {
        if cache.entries != entries {
            *cache = Compiled::new(entries);
        }
        cache.apply(text, app, enabled)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn e(a: &str, b: &str) -> Entry {
        validate(a, b).unwrap()
    }
    fn sound(text: &str, entries: &[Entry], app: Option<&str>) -> Applied {
        apply_with_sound(text, entries, app, true)
    }
    #[test]
    fn equal_sound_spelling_retains_the_more_specific_origin() {
        let global = e("obi ante", "Obiente");
        let mut scoped = global.clone();
        scoped.app = Some("editor.exe".into());
        let entries = [global, scoped.clone()];
        let applied = sound("obi yente", &entries, Some("editor.exe"));
        assert_eq!(applied.text, "Obiente");
        assert!(applied.sounds[0].entry == scoped);
    }
    #[test]
    fn sound_names_preserve_saved_case_and_phrase_boundaries() {
        let entries = [e("obi ante", "Obiente"), e("ven kord", "Vencord")];
        for (input, expected) in [
            ("Visit Obi Yente today.", "Visit Obiente today."),
            ("Use when cord.", "Use Vencord."),
            ("Use Ven cord and obi yente.", "Use Vencord and Obiente."),
            ("Use obi y ente.", "Use Obiente."),
            ("Use o bi y ente.", "Use Obiente."),
        ] {
            assert_eq!(sound(input, &entries, None).text, expected);
        }
        for text in [
            "obi. yente",
            "obi, yente",
            "obi\nyente",
            "preobi yente",
            "obi yente's",
            "o b i y ente",
        ] {
            assert_eq!(sound(text, &entries, None).text, text, "{text}");
        }
    }
    #[test]
    fn sound_respects_apps_all_context_policies_and_sentence_limits() {
        let mut entry = e("obi ante", "Obiente");
        entry.app = Some("editor.exe".into());
        entry.cues = vec!["website".into()];
        entry.contexts.push(Context {
            cues: vec!["domain".into()],
            ignore_case: true,
        });
        let entries = [entry];
        assert_eq!(
            sound("Obi Yente website", &entries, Some("EDITOR.EXE")).text,
            "Obiente website"
        );
        assert_eq!(
            sound("domain obi yente", &entries, Some("editor.exe")).text,
            "domain Obiente"
        );
        for (text, app) in [
            ("Obi Yente website", None),
            ("Obi Yente website", Some("chat.exe")),
            ("website. Obi Yente", Some("editor.exe")),
            ("Obi Yente", Some("editor.exe")),
        ] {
            assert_eq!(sound(text, &entries, app).changed, 0);
        }
        assert_eq!(
            sound(
                &format!("website {} obi yente", "x".repeat(121)),
                &entries,
                Some("editor.exe")
            )
            .changed,
            0
        );
    }
    #[test]
    fn sound_does_not_consume_protected_words_or_common_destinations() {
        for (heard, wanted, input) in [
            ("nah tredy", "Notready", "not ready"),
            ("tha corder", "Thecorder", "the corder"),
            ("too lace", "Toolace", "two lace"),
            ("wee cord", "Wecord", "we cord"),
            ("won day", "Monday", "mon day"),
            ("there", "Their", "there"),
            ("pro ject", "Project", "pro jekt"),
            ("fif teen", "Fifteen", "fif teen"),
        ] {
            let mut entry = e(heard, wanted);
            entry.sound_alike = true;
            // Only fallback is under test; a saved exact pair remains explicit.
            if heard == input {
                entry.heard = "unrelated spelling".into();
            }
            assert_eq!(sound(input, &[entry], None).changed, 0, "{input}");
        }
        for text in [
            "there their then",
            "10 100 one two",
            "I you we they",
            "Mon day",
            "when record",
        ] {
            assert_eq!(
                sound(
                    text,
                    &[e("obi ante", "Obiente"), e("ven cord", "Vencord")],
                    None
                )
                .changed,
                0
            );
        }
    }
    #[test]
    fn sound_uses_heard_alias_but_exact_priority_and_outputs_are_reserved() {
        let entries = [e("cinderkit", "Obiente"), e("kit", "package")];
        assert_eq!(sound("sinderkit", &entries, None).text, "Obiente");
        assert_eq!(sound("cinder kit", &entries, None).text, "cinder package");
        let entries = [e("obi ante", "Obiente"), e("other", "Obi Yente")];
        assert_eq!(sound("Obi Yente", &entries, None).changed, 0);
        let entries = [e("obi ante", "Obiente"), e("Obiente", "changed")];
        assert_eq!(sound("obi yente", &entries, None).text, "Obiente");
        let entries = [e("obi yente", "exact wins"), e("obi ante", "Obiente")];
        assert_eq!(sound("obi yente", &entries, None).text, "exact wins");
    }
    #[test]
    fn sound_picks_closest_and_abstains_on_equal_alternatives() {
        let entries = [e("other form", "Obiente"), e("other name", "Obiante")];
        assert_eq!(sound("obi yente", &entries, None).text, "Obiente");
        let entries = [e("obi yente", "Obiente"), e("obi yente", "Obiante")];
        assert_eq!(sound("obi yente", &entries, None).text, "Obiente"); // Exact saved order.
        let entries = [e("same alias", "Obiente"), e("same alias", "Obiante")];
        assert_eq!(sound("saim alias", &entries, None).changed, 0);
    }
    #[test]
    fn sound_flags_migrate_persist_and_invalidate_cached_keys() {
        let mut entries = vec![e("obi ante", "Obiente")];
        assert!(entries[0].sound_alike);
        assert_eq!(sound("obi yente", &entries, None).changed, 1);
        entries[0].sound_alike = false;
        assert_eq!(sound("obi yente", &entries, None).changed, 0);
        assert_eq!(sound("obi ante", &entries, None).changed, 1);
        entries[0].sound_alike = true;
        assert_eq!(
            apply_with_sound("obi yente", &entries, None, false).changed,
            0
        );
        entries[0].wanted = "DifferentName".into();
        entries[0].heard = "different alias".into();
        assert_eq!(sound("obi yente", &entries, None).changed, 0);
        let legacy: Entry =
            serde_json::from_str(r#"{"heard":"obi ante","wanted":"Obiente"}"#).unwrap();
        assert!(legacy.sound_alike);
        let common: Entry = serde_json::from_str(r#"{"heard":"werd","wanted":"word"}"#).unwrap();
        assert!(!common.sound_alike);
        let mut explicit = legacy;
        explicit.sound_alike = false;
        let restored: Entry =
            serde_json::from_str(&serde_json::to_string(&explicit).unwrap()).unwrap();
        assert!(!restored.sound_alike);
    }
    #[test]
    fn cached_preview_budget_for_two_hundred_entries_and_words() {
        let mut entries: Vec<_> = (0..199)
            .map(|i| {
                let suffix = format!(
                    "{}{}",
                    (b'a' + (i / 26) as u8) as char,
                    (b'a' + (i % 26) as u8) as char
                );
                e(&format!("termon{suffix}"), &format!("Termun{suffix}"))
            })
            .collect();
        entries.push(e("obi ante", "Obiente"));
        let text = format!(
            "{} obi yente",
            "Please visit website example using vocabulary today with context ".repeat(22)
        );
        assert_eq!(apply_with_sound(&text, &entries, None, true).changed, 1);
        let mut times = Vec::new();
        for _ in 0..25 {
            let start = std::time::Instant::now();
            std::hint::black_box(apply_with_sound(
                std::hint::black_box(&text),
                &entries,
                None,
                true,
            ));
            times.push(start.elapsed());
        }
        times.sort();
        eprintln!("Cached vocabulary median: {:?}", times[times.len() / 2]);
        // The median avoids a scheduler interruption becoming a timing failure.
        assert!(times[times.len() / 2] < std::time::Duration::from_millis(1));
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
