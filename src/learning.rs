use crate::dictionary::{self, Entry};

pub struct Change {
    pub entry: Entry,
    previous: Option<Entry>,
    saved: Option<Entry>,
}
impl Change {
    pub fn apply(entries: &mut Vec<Entry>, mut entry: Entry) -> Option<Self> {
        let previous = entries.iter().find(|e| e.same_key(&entry)).cloned();
        if let Some(previous) = &previous {
            // Disabling a rule is an explicit user choice. Learning must not
            // silently reactivate it, or reset its matching policy.
            if !previous.enabled {
                return None;
            }
            entry.ignore_case = previous.ignore_case;
        }
        let saved = (entry.heard != entry.wanted).then_some(entry.clone());
        if previous == saved {
            return None;
        }
        entries.retain(|e| !e.same_key(&entry));
        if let Some(saved) = &saved {
            entries.push(saved.clone());
        }
        Some(Self {
            entry,
            previous,
            saved,
        })
    }
    pub fn undo(self, entries: &mut Vec<Entry>) -> bool {
        if entries.iter().find(|e| e.same_key(&self.entry)) != self.saved.as_ref() {
            return false;
        }
        entries.retain(|e| !e.same_key(&self.entry));
        if let Some(previous) = self.previous {
            entries.push(previous);
        }
        true
    }
}

/// Resolve a revision of an applied correction using only its local sentence
/// and destination app. Never infer an origin from a different app or cue.
pub fn bind_context(mut entry: Entry, before: &str, entries: &[Entry]) -> Entry {
    let mut origins: Vec<_> = entries
        .iter()
        .filter(|rule| {
            rule.enabled
                && rule.wanted == entry.heard
                && rule.app.as_deref().is_none_or(|app| {
                    entry
                        .app
                        .as_deref()
                        .is_some_and(|active| active.eq_ignore_ascii_case(app))
                })
                && dictionary::output_in_context(rule, before)
        })
        .collect();
    origins.sort_by_key(|rule| std::cmp::Reverse((rule.app.is_some(), !rule.cues.is_empty())));
    if let Some(origin) = origins.first() {
        let priority = (origin.app.is_some(), !origin.cues.is_empty());
        if origins
            .iter()
            .skip(1)
            .any(|other| (other.app.is_some(), !other.cues.is_empty()) == priority)
        {
            return entry;
        }
        entry.heard.clone_from(&origin.heard);
        entry.cues.clone_from(&origin.cues);
        entry.ignore_case = origin.ignore_case;
    }
    entry
}

fn tokens(text: &str) -> Vec<(usize, usize, &str)> {
    let mut result = Vec::new();
    let mut start = None;
    for (i, c) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        if c.is_alphanumeric() || c == '\'' || c == '’' {
            start.get_or_insert(i);
        } else if let Some(a) = start.take() {
            result.push((a, i, &text[a..i]));
        }
    }
    result
}

/// Learn one short lexical substitution, not additions, deletions or rewrites.
pub fn correction(before: &str, after: &str) -> Option<Entry> {
    if before == after || before.len() > 16000 || after.len() > 16000 {
        return None;
    }
    // A spoken mention is a lexical correction even though token-only diffing
    // sees the word "at" disappear. Preserve the handle in the saved rule.
    for (at, c) in after.char_indices() {
        if c != '@'
            || after[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        let handle_len = after[at + 1..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .map(char::len_utf8)
            .sum::<usize>();
        if handle_len == 0 {
            continue;
        }
        let end = at + 1 + handle_len;
        for word in ["at", "At"] {
            let heard = format!("{word} {}", &after[at + 1..end]);
            if before == format!("{}{}{}", &after[..at], heard, &after[end..]) {
                return dictionary::validate(&heard, &after[at..end]).ok();
            }
        }
    }
    let a = tokens(before);
    let b = tokens(after);
    let prefix = a.iter().zip(&b).take_while(|(a, b)| a.2 == b.2).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(a, b)| a.2 == b.2)
        .count();
    let aa = &a[prefix..a.len() - suffix];
    let bb = &b[prefix..b.len() - suffix];
    if aa.is_empty() || bb.is_empty() || aa.len() > 3 || bb.len() > 3 {
        return None;
    }
    // Separated edits and changes to meaning-bearing operators are not spelling corrections.
    if aa.iter().any(|a| bb.iter().any(|b| a.2 == b.2)) {
        return None;
    }
    const PROTECTED: &[&str] = &[
        "no",
        "not",
        "never",
        "don't",
        "dont",
        "do",
        "can",
        "can't",
        "cannot",
        "will",
        "won't",
        "is",
        "isn't",
        "a",
        "an",
        "the",
        "and",
        "or",
        "to",
        "from",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "hundred",
        "thousand",
        "million",
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ];
    if aa.iter().chain(bb).any(|t| {
        t.2.chars().any(char::is_numeric) || PROTECTED.contains(&t.2.to_lowercase().as_str())
    }) {
        return None;
    }
    let heard = &before[aa.first()?.0..aa.last()?.1];
    let wanted = &after[bb.first()?.0..bb.last()?.1];
    if heard
        .chars()
        .chain(wanted.chars())
        .any(|c| matches!(c, '.' | '!' | '?' | '\n' | '\r' | ':' | '@' | '/' | '\\'))
    {
        return None;
    }
    dictionary::validate(heard, wanted).ok()
}

/// Retain just enough context to recognize the inserted span, in memory only.
pub struct Span {
    prefix: String,
    suffix: String,
    pub original: String,
}
impl Span {
    pub fn inserted(before: &str, after: &str, inserted: &str) -> Option<Self> {
        if inserted.is_empty() || before.len() > 16000 || after.len() > 16000 {
            return None;
        }
        let mut matches = after.match_indices(inserted).filter(|(at, _)| {
            let prefix = &after[..*at];
            let suffix = &after[*at + inserted.len()..];
            before.len() >= prefix.len() + suffix.len()
                && before.starts_with(prefix)
                && before.ends_with(suffix)
        });
        let (at, _) = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(Self {
            prefix: after[..at].into(),
            suffix: after[at + inserted.len()..].into(),
            original: inserted.into(),
        })
    }
    pub fn current<'a>(&self, field: &'a str) -> Option<&'a str> {
        if field.len() > 16000 || field.len() < self.prefix.len() + self.suffix.len() {
            return None;
        }
        let middle = field
            .strip_prefix(&self.prefix)?
            .strip_suffix(&self.suffix)?;
        Some(middle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn refine(entries: &mut Vec<Entry>, heard: &str, wanted: &str) -> Change {
        let entry = bind_context(dictionary::validate(heard, wanted).unwrap(), heard, entries);
        Change::apply(entries, entry).unwrap()
    }
    #[test]
    fn learns_spoken_mentions_and_reapplies_them_after_serialization() {
        let entry = correction("Hello at ExampleHandle.", "Hello @ExampleHandle.").unwrap();
        assert_eq!(
            (&*entry.heard, &*entry.wanted),
            ("at ExampleHandle", "@ExampleHandle")
        );
        let json = serde_json::to_vec(&vec![entry]).unwrap();
        let entries: Vec<Entry> = serde_json::from_slice(&json).unwrap();
        assert_eq!(
            dictionary::apply("Let's try again. Hello at ExampleHandle.", &entries).0,
            "Let's try again. Hello @ExampleHandle."
        );
        assert!(correction("Meet at home.", "Meet home.").is_none());
        assert!(
            correction(
                "Hello at ExampleHandle today.",
                "Hello @ExampleHandle tomorrow."
            )
            .is_none()
        );
    }
    #[test]
    fn refining_and_undoing_learning_restores_the_previous_rule() {
        let mut entries = vec![dictionary::validate("Jon", "John").unwrap()];
        let change = refine(&mut entries, "John", "Johnny");
        assert_eq!(dictionary::apply("Hi Jon.", &entries).0, "Hi Johnny.");
        assert!(change.undo(&mut entries));
        assert_eq!(dictionary::apply("Hi Jon.", &entries).0, "Hi John.");
        let change = refine(&mut entries, "John", "Jon");
        assert!(entries.is_empty());
        assert!(change.undo(&mut entries));
        assert_eq!(entries.len(), 1);
        let change = refine(&mut entries, "John", "Johnny");
        entries[0].wanted = "Jonathan".into();
        assert!(!change.undo(&mut entries));
        assert_eq!(entries[0].wanted, "Jonathan");
    }
    #[test]
    fn contextual_refinement_preserves_policy_and_isolates_apps() {
        let mut rule = dictionary::validate("mercury", "Mercury").unwrap();
        rule.app = Some("editor.exe".into());
        rule.cues = vec!["function".into()];
        rule.ignore_case = true;
        let mut entries = vec![rule.clone()];
        let mut observed = dictionary::validate("Mercury", "mercury_fn").unwrap();
        observed.app = rule.app.clone();
        let refined = bind_context(observed.clone(), "Call function Mercury", &entries);
        assert_eq!(refined.heard, "mercury");
        assert_eq!(refined.cues, rule.cues);
        assert!(refined.ignore_case);
        let change = Change::apply(&mut entries, refined).unwrap();
        assert_eq!(
            dictionary::apply_in("Call function MERCURY", &entries, Some("editor.exe")).0,
            "Call function mercury_fn"
        );
        assert_eq!(
            dictionary::apply_in("Call function MERCURY", &entries, Some("chat.exe")).1,
            0
        );
        assert!(change.undo(&mut entries));
        assert!(entries[0] == rule);
        assert_eq!(
            bind_context(observed.clone(), "Mercury is a planet", &entries).heard,
            "Mercury"
        );
        observed.app = Some("chat.exe".into());
        assert_eq!(
            bind_context(observed, "Call function Mercury", &entries).heard,
            "Mercury"
        );
    }
    #[test]
    fn global_rule_refinement_stays_in_the_observed_app() {
        let global = dictionary::validate("Jon", "John").unwrap();
        let mut entries = vec![global.clone()];
        let mut observed = dictionary::validate("John", "Johnny").unwrap();
        observed.app = Some("chat.exe".into());
        let refined = bind_context(observed, "Hi John.", &entries);
        let change = Change::apply(&mut entries, refined).unwrap();
        assert_eq!(
            dictionary::apply_in("Jon", &entries, Some("chat.exe")).0,
            "Johnny"
        );
        assert_eq!(
            dictionary::apply_in("Jon", &entries, Some("editor.exe")).0,
            "John"
        );
        assert!(change.undo(&mut entries));
        assert!(entries == vec![global]);
    }
    #[test]
    fn learning_does_not_reenable_disabled_rules_or_overwrite_manual_edits_on_undo() {
        let mut rule = dictionary::validate("Jon", "John").unwrap();
        rule.enabled = false;
        let mut entries = vec![rule.clone()];
        assert!(
            Change::apply(&mut entries, dictionary::validate("Jon", "Johnny").unwrap()).is_none()
        );
        assert!(entries[0] == rule);
        entries[0].enabled = true;
        entries[0].ignore_case = true;
        let change =
            Change::apply(&mut entries, dictionary::validate("Jon", "Johnny").unwrap()).unwrap();
        assert!(entries[0].ignore_case);
        entries[0].cues = vec!["team".into()];
        assert!(!change.undo(&mut entries));
        assert_eq!(entries[0].cues, vec!["team"]);
    }
    #[test]
    fn ambiguous_origins_and_other_sentences_are_not_refined() {
        let entries = vec![
            dictionary::validate("Jon", "John").unwrap(),
            dictionary::validate("Jonn", "John").unwrap(),
        ];
        let observed = dictionary::validate("John", "Johnny").unwrap();
        assert_eq!(bind_context(observed, "John", &entries).heard, "John");
        let mut contextual = dictionary::validate("rust", "Rust").unwrap();
        contextual.cues = vec!["crate".into()];
        let observed = dictionary::validate("Rust", "rustlang").unwrap();
        assert_eq!(
            bind_context(observed, "The crate arrived. Rust is here.", &[contextual]).heard,
            "Rust"
        );
    }
    #[test]
    fn learns_names_and_phrases_with_unicode_boundaries() {
        let e = correction("Send this to Jon tomorrow.", "Send this to John tomorrow.").unwrap();
        assert_eq!((&*e.heard, &*e.wanted), ("Jon", "John"));
        let e = correction("Use whisper flow.", "Use Wispr Flow.").unwrap();
        assert_eq!(e.wanted, "Wispr Flow");
        assert!(correction("Bonjour André.", "Bonjour Andrés.").is_some());
    }
    #[test]
    fn ignores_composition_numbers_negations_and_separated_edits() {
        for (a, b) in [
            ("Hello", "Hello there"),
            ("Hello there", "Hello"),
            ("Send 12 euros", "Send 15 euros"),
            ("Do send it", "Do not send it"),
            ("Jon met Sera", "John met Sarah"),
            ("It is good.", "It is good!"),
        ] {
            assert!(correction(a, b).is_none(), "{a} => {b}");
        }
    }
    #[test]
    fn only_tracks_the_inserted_text_not_neighboring_content() {
        let span = Span::inserted("Before.  After.", "Before. Hi Jon. After.", "Hi Jon.").unwrap();
        assert_eq!(span.current("Before. Hi John. After."), Some("Hi John."));
        assert!(span.current("Changed. Hi John. After.").is_none());
        assert!(Span::inserted("x", "x x", "x").is_none());
        let span = Span::inserted("replace me", "Jon", "Jon").unwrap();
        assert_eq!(span.current("John"), Some("John"));
    }
}
