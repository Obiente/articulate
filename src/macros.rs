use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct VoiceMacro {
    pub trigger: String,
    pub expansion: String,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}
fn enabled() -> bool {
    true
}

pub fn validate(trigger: &str, expansion: &str, app: &str) -> anyhow::Result<VoiceMacro> {
    let trigger = trigger
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    anyhow::ensure!(
        !trigger.is_empty()
            && trigger.len() <= 80
            && trigger.split_whitespace().count() <= 4
            && trigger.chars().all(|c| c.is_alphanumeric() || c == ' '),
        "Use one to four spoken words for the trigger"
    );
    anyhow::ensure!(
        trigger.split_whitespace().next() != Some("bang"),
        "Enter the trigger without the word bang"
    );
    let expansion = expansion.replace("\r\n", "\n").trim().to_owned();
    anyhow::ensure!(
        !expansion.is_empty() && expansion.len() <= 8000,
        "Add expansion text, up to 8,000 bytes"
    );
    anyhow::ensure!(
        !expansion.chars().any(|c| c.is_control() && c != '\n'),
        "Use plain text and line breaks in the expansion"
    );
    anyhow::ensure!(
        expansion.matches("{text}").count() <= 1,
        "Use {{text}} at most once"
    );
    Ok(VoiceMacro {
        trigger,
        expansion,
        app: crate::dictionary::app_scope(app)?,
        enabled: true,
    })
}

fn words(input: &str) -> Vec<(usize, usize, String)> {
    let mut result = Vec::new();
    let mut start = None;
    for (i, c) in input
        .char_indices()
        .chain(std::iter::once((input.len(), ' ')))
    {
        if c.is_alphanumeric() || c == '_' {
            start.get_or_insert(i);
        } else if let Some(a) = start.take() {
            result.push((a, i, input[a..i].to_lowercase()));
        }
    }
    result
}
pub fn candidate(input: &str) -> bool {
    words(input).first().is_some_and(|t| t.2 == "bang")
}

pub struct Expansion {
    pub text: String,
    pub trigger: String,
}

/// Explicit whole-utterance commands only. Never evaluate code or recurse into output.
pub fn expand(
    input: &str,
    macros: &[VoiceMacro],
    app: Option<&str>,
) -> anyhow::Result<Option<Expansion>> {
    let tokens = words(input);
    if !tokens.first().is_some_and(|t| t.2 == "bang") {
        return Ok(None);
    }
    let mut available: Vec<_> = macros
        .iter()
        .filter(|m| {
            m.enabled
                && m.app
                    .as_deref()
                    .is_none_or(|scope| app.is_some_and(|a| a.eq_ignore_ascii_case(scope)))
        })
        .collect();
    available.sort_by_key(|m| {
        std::cmp::Reverse((m.trigger.split_whitespace().count(), m.app.is_some()))
    });
    for m in available {
        let trigger: Vec<_> = m.trigger.split_whitespace().collect();
        if tokens.len() < trigger.len() + 1
            || !trigger.iter().zip(&tokens[1..]).all(|(a, b)| *a == b.2)
        {
            continue;
        }
        let has_args = m.expansion.contains("{text}");
        if !has_args && tokens.len() != trigger.len() + 1 {
            continue;
        }
        let args = input[tokens[trigger.len()].1..]
            .trim_start_matches(|c: char| c.is_whitespace() || ",:;.!?".contains(c));
        anyhow::ensure!(
            !has_args || args.chars().any(char::is_alphanumeric),
            "Say bang {}, followed by the words to insert",
            m.trigger
        );
        let text = if has_args {
            m.expansion.replace("{text}", args)
        } else {
            m.expansion.clone()
        };
        anyhow::ensure!(
            text.len() <= 16000,
            "This macro expansion is too long. Shorten its text"
        );
        return Ok(Some(Expansion {
            text,
            trigger: m.trigger.clone(),
        }));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_triggers_expand_once_and_keep_template_text_literal() {
        let m = validate("signature", "Thanks,\nExample Team", "").unwrap();
        assert_eq!(
            expand("Bang, signature.", std::slice::from_ref(&m), None)
                .unwrap()
                .unwrap()
                .text,
            m.expansion
        );
        assert!(
            expand(
                "Tell them about bang signature",
                std::slice::from_ref(&m),
                None
            )
            .unwrap()
            .is_none()
        );
        assert!(
            expand("Bang signature for tomorrow", &[m], None)
                .unwrap()
                .is_none()
        );
        let m = validate("reply", "Thanks for the update. {text}", "").unwrap();
        assert_eq!(
            expand(
                "Bang reply I will check tomorrow.",
                std::slice::from_ref(&m),
                None
            )
            .unwrap()
            .unwrap()
            .text,
            "Thanks for the update. I will check tomorrow."
        );
        assert_eq!(
            expand(
                "Bang reply. I will check tomorrow.",
                std::slice::from_ref(&m),
                None
            )
            .unwrap()
            .unwrap()
            .text,
            "Thanks for the update. I will check tomorrow."
        );
        assert!(expand("Bang reply.", &[m], None).is_err());
    }
    #[test]
    fn app_scopes_disable_and_validation_are_enforced() {
        let global = validate("signature", "Global", "").unwrap();
        let mut scoped = validate("signature", "Work", "outlook.exe").unwrap();
        assert_eq!(
            expand(
                "bang signature",
                &[global.clone(), scoped.clone()],
                Some("OUTLOOK.EXE")
            )
            .unwrap()
            .unwrap()
            .text,
            "Work"
        );
        scoped.enabled = false;
        assert_eq!(
            expand("bang signature", &[global, scoped], Some("outlook.exe"))
                .unwrap()
                .unwrap()
                .text,
            "Global"
        );
        assert!(validate("bang signature", "text", "").is_err());
        assert!(validate("signature", "{text}{text}", "").is_err());
    }
}

/// Settings and state for sentence editing, disabled by default.
#[allow(
    dead_code,
    reason = "Backend API awaiting settings and text pipeline wiring"
)]
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct EditingSettings {
    pub spoken_editing_commands: bool,
}

#[allow(dead_code, reason = "Backend API awaiting text pipeline wiring")]
#[derive(Default)]
pub struct DictationEditor {
    segments: Vec<String>,
    undo: Vec<Vec<String>>,
    pub notice: Option<String>,
    prior: Option<(Vec<String>, Vec<Vec<String>>)>,
}
#[allow(dead_code, reason = "Backend API awaiting text pipeline wiring")]
impl DictationEditor {
    pub fn begin_recording(&mut self, unchanged: bool) {
        let prior = if unchanged && (!self.segments.is_empty() || !self.undo.is_empty()) {
            Some((
                std::mem::take(&mut self.segments),
                std::mem::take(&mut self.undo),
            ))
        } else {
            None
        };
        *self = Self {
            prior,
            ..Default::default()
        };
    }
    /// Call exactly once per finalized segment, before applying bang macros.
    pub fn apply(&mut self, segment: &str, settings: &EditingSettings) -> String {
        if segment.trim().is_empty() {
            return self.text();
        }
        self.notice = None;
        if !segment.trim().is_empty() && previous_command(segment).is_none() {
            self.prior = None;
        }
        if settings.spoken_editing_commands
            && self.segments.is_empty()
            && previous_command(segment).is_some()
            && let Some((segments, undo)) = self.prior.take()
        {
            self.segments = segments;
            self.undo = undo;
            if previous_command(segment) == Some(false) {
                self.undo.push(std::mem::take(&mut self.segments));
                self.notice = Some("Previous dictation removed.".into());
                return self.text();
            }
            if !self.segments.is_empty() {
                self.undo.push(Vec::new());
            }
        }
        if settings.spoken_editing_commands {
            let mut start = 0;
            let mut sentences = Vec::new();
            for (index, ch) in segment.char_indices() {
                let end = index + ch.len_utf8();
                if matches!(ch, '.' | '!' | '?' | '\n')
                    && segment[end..]
                        .chars()
                        .next()
                        .is_none_or(char::is_whitespace)
                {
                    sentences.push(&segment[start..end]);
                    start = end;
                }
            }
            if !segment[start..].trim().is_empty() {
                sentences.push(&segment[start..]);
            }
            sentences.retain(|s| !s.trim().is_empty());
            if sentences.len() > 1 {
                for sentence in sentences {
                    self.apply(sentence.trim(), settings);
                }
                return self.text();
            }
        }
        let command = segment
            .trim()
            .trim_end_matches(['.', '!', '?'])
            .trim()
            .to_ascii_lowercase();
        if settings.spoken_editing_commands && command == "undo that" {
            if let Some(previous) = self.undo.pop() {
                self.segments = previous;
                self.notice = Some("Previous edit undone.".into());
            } else {
                self.notice = Some("Nothing to undo.".into());
            }
        } else {
            let continuation = self
                .segments
                .last()
                .is_some_and(|s| !s.ends_with(['.', '!', '?', '\n']))
                && !matches!(
                    command.as_str(),
                    "scratch that" | "delete that" | "new line" | "new paragraph"
                );
            if !continuation {
                self.undo.push(self.segments.clone());
            }
            if self.undo.len() > 32 {
                self.undo.remove(0);
            }
            match (settings.spoken_editing_commands, command.as_str()) {
                (true, "scratch that" | "delete that") => {
                    if self.segments.pop().is_none() {
                        self.undo.pop();
                        self.notice = Some("Nothing to remove from this dictation.".into());
                    } else {
                        self.notice = Some("Previous sentence removed.".into());
                    }
                }
                (true, "new line") => self.segments.push("\n".into()),
                (true, "new paragraph") => self.segments.push("\n\n".into()),
                _ => {
                    if self
                        .segments
                        .last()
                        .is_some_and(|s| !s.ends_with(['.', '!', '?', '\n']))
                    {
                        let previous = self.segments.last_mut().unwrap();
                        previous.push(' ');
                        previous.push_str(segment);
                    } else {
                        self.segments.push(segment.into());
                    }
                }
            }
        }
        self.text()
    }
    pub fn text(&self) -> String {
        let mut text = String::new();
        for segment in &self.segments {
            if !text.is_empty() && !text.ends_with('\n') && !segment.starts_with('\n') {
                text.push(' ');
            }
            text.push_str(segment);
        }
        text
    }
}
#[cfg(test)]
mod editing_tests {
    use super::*;
    #[test]
    fn disabled_and_ambiguous_commands_are_literal() {
        for phrase in [
            "new line",
            "Please add a new line",
            "scratch that idea",
            "I said undo that",
            "\"new line\"",
            "new line, please",
        ] {
            let mut editor = DictationEditor::default();
            assert_eq!(editor.apply(phrase, &EditingSettings::default()), phrase);
        }
        for phrase in [
            "Please add a new line",
            "scratch that idea",
            "I said undo that",
            "\"new line\"",
            "new line, please",
        ] {
            assert_eq!(
                DictationEditor::default().apply(
                    phrase,
                    &EditingSettings {
                        spoken_editing_commands: true
                    }
                ),
                phrase
            );
        }
    }
    #[test]
    fn local_recordings_keep_history_only_when_unchanged() {
        let settings = EditingSettings {
            spoken_editing_commands: true,
        };
        let mut e = DictationEditor::default();
        e.apply("First sentence. Second sentence.", &settings);
        e.begin_recording(true);
        assert_eq!(e.apply("scratch that", &settings), "");
        e.begin_recording(true);
        assert_eq!(
            e.apply("undo that", &settings),
            "First sentence. Second sentence."
        );
        e.begin_recording(false);
        assert_eq!(e.apply("scratch that", &settings), "");
        assert!(e.notice.is_some());
        let mut e = DictationEditor::default();
        e.apply("Old recording.", &settings);
        e.begin_recording(true);
        assert_eq!(
            e.apply("New recording. Scratch that. Undo that.", &settings),
            "New recording."
        );
        let mut e = DictationEditor::default();
        assert_eq!(
            e.apply("The price is 1.25.", &settings),
            "The price is 1.25."
        );
    }
    #[test]
    fn sentences_and_capture_windows_are_independent() {
        let settings = EditingSettings {
            spoken_editing_commands: true,
        };
        let mut e = DictationEditor::default();
        assert_eq!(
            e.apply("First sentence. Second sentence. Scratch that.", &settings),
            "First sentence."
        );
        assert_eq!(
            e.apply("undo that", &settings),
            "First sentence. Second sentence."
        );
        assert_eq!(
            e.apply("I might scratch that tomorrow.", &settings),
            "First sentence. Second sentence. I might scratch that tomorrow."
        );
        let mut e = DictationEditor::default();
        e.apply("Sentence across", &settings);
        e.apply("windows.", &settings);
        assert_eq!(e.apply("scratch that", &settings), "");
        assert!(e.apply("delete that", &settings).is_empty());
        assert!(e.notice.is_some());
        assert_eq!(previous_command("Scratch that!"), Some(false));
        assert_eq!(previous_command("I said scratch that."), None);
    }
    #[test]
    fn editing_and_undo_preserve_segment_boundaries() {
        let mut e = DictationEditor::default();
        let settings = EditingSettings {
            spoken_editing_commands: true,
        };
        for s in ["First.", "New line.", "Second.", "new paragraph", "Third."] {
            e.apply(s, &settings);
        }
        assert_eq!(e.text(), "First.\nSecond.\n\nThird.");
        assert_eq!(e.apply("Scratch that!", &settings), "First.\nSecond.\n\n");
        assert_eq!(e.apply("undo that", &settings), "First.\nSecond.\n\nThird.");
        e.apply("delete that", &settings);
        assert_eq!(
            e.apply("Replacement.", &settings),
            "First.\nSecond.\n\nReplacement."
        );
    }
}

/// Only an entire utterance can edit an earlier insertion.
pub fn previous_command(text: &str) -> Option<bool> {
    match text
        .trim()
        .trim_end_matches(['.', '!', '?'])
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "scratch that" | "delete that" => Some(false),
        "undo that" => Some(true),
        _ => None,
    }
}

/// A replacement is committed only after the field confirms the edit.
pub struct InsertionHistory {
    text: String,
    undo: String,
}
impl InsertionHistory {
    pub fn new(text: String, replaced: String) -> Self {
        Self {
            text,
            undo: replaced,
        }
    }
    pub fn replacement(&self, undo: bool) -> anyhow::Result<String> {
        if undo {
            return Ok(self.undo.clone());
        }
        anyhow::ensure!(
            !self.text.is_empty(),
            "Nothing to remove from the previous insertion."
        );
        Ok(String::new())
    }
    pub fn commit(&mut self, replacement: String) {
        self.undo = std::mem::replace(&mut self.text, replacement);
    }
}
#[cfg(test)]
mod insertion_history_tests {
    use super::*;
    #[test]
    fn undo_restores_replaced_selection_and_removed_text() {
        let mut history = InsertionHistory::new("New text".into(), "Selected text".into());
        assert_eq!(history.replacement(true).unwrap(), "Selected text");
        assert_eq!(history.replacement(false).unwrap(), "");
        // Preparing an edit does not change history if the field rejects it.
        assert_eq!(history.replacement(true).unwrap(), "Selected text");
        history.commit(history.replacement(false).unwrap());
        assert!(history.replacement(false).is_err());
        assert_eq!(history.replacement(true).unwrap(), "New text");
        history.commit(history.replacement(true).unwrap());
        assert_eq!(history.replacement(true).unwrap(), "");
    }
}
