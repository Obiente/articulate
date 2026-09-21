//! Explicit, local formatting preferences. These rules do not paraphrase text.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WritingStyle {
    Verbatim,
    #[default]
    Clean,
    Chat,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleRule {
    pub app: String,
    pub style: WritingStyle,
}

/// Only a process basename is accepted. Paths, titles, and document content
/// never act as style context. Last matching rule wins if old data duplicates it.
pub fn effective(rules: &[StyleRule], app: Option<&str>, clean_speech: bool) -> WritingStyle {
    let fallback = if clean_speech {
        WritingStyle::Clean
    } else {
        WritingStyle::Verbatim
    };
    let Some(app) = app.and_then(|s| crate::dictionary::app_scope(s).ok().flatten()) else {
        return fallback;
    };
    rules
        .iter()
        .rev()
        .find(|rule| {
            crate::dictionary::app_scope(&rule.app)
                .ok()
                .flatten()
                .is_some_and(|scope| scope == app)
        })
        .map_or(fallback, |rule| rule.style)
}

pub fn apply(input: &str, style: WritingStyle, protected: &[String]) -> (String, usize) {
    if style == WritingStyle::Verbatim {
        return (input.to_owned(), 0);
    }
    let (baseline, prior_changes) = crate::cleanup::apply(input);
    let mut text = crate::polish::speech::clean(input, protected);
    let mut changes = if text == input {
        0
    } else {
        (prior_changes
            + baseline
                .split_whitespace()
                .count()
                .saturating_sub(text.split_whitespace().count()))
        .max(1)
    };
    if style == WritingStyle::Chat && removable_period(&text) {
        // Keep trailing whitespace exactly, including the destination's spacing.
        let period = text.trim_end().len() - 1;
        text.remove(period);
        changes += 1;
    }
    (text, changes)
}

fn removable_period(text: &str) -> bool {
    let text = text.trim_end();
    let Some(sentence) = text.strip_suffix('.') else {
        return false;
    };
    if sentence.is_empty()
        || text.chars().count() > 280
        || text.contains(['\n', '\r'])
        || sentence.contains(['.', '?', '!', ':', '/', '\\', '@'])
    {
        return false;
    }
    let last_word = sentence.split_whitespace().last().unwrap_or_default();
    // Do not change a numeric literal, initial, acronym or abbreviation.
    if !last_word.chars().all(char::is_alphabetic)
        || last_word.chars().count() == 1
        || last_word.chars().all(char::is_uppercase)
    {
        return false;
    }
    !matches!(
        last_word.to_ascii_lowercase().as_str(),
        "mr" | "mrs"
            | "ms"
            | "dr"
            | "prof"
            | "jr"
            | "sr"
            | "st"
            | "vs"
            | "etc"
            | "inc"
            | "ltd"
            | "co"
            | "dept"
            | "approx"
            | "eg"
            | "ie"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_cleanup_preference_is_preserved_without_rules() {
        assert_eq!(
            effective(&[], Some("editor.exe"), true),
            WritingStyle::Clean
        );
        assert_eq!(effective(&[], None, false), WritingStyle::Verbatim);
        let raw = "I I do not agree.\n\nNo, no.";
        assert_eq!(apply(raw, WritingStyle::Verbatim, &[]), (raw.into(), 0));
    }

    #[test]
    fn only_valid_process_basenames_match_and_serialize() {
        let rules = vec![StyleRule {
            app: "DISCORD.EXE".into(),
            style: WritingStyle::Chat,
        }];
        assert_eq!(
            effective(&rules, Some("discord.exe"), true),
            WritingStyle::Chat
        );
        assert_eq!(
            effective(&rules, Some("other.exe"), true),
            WritingStyle::Clean
        );
        assert_eq!(
            effective(&rules, Some("C:/apps/discord.exe"), true),
            WritingStyle::Clean
        );
        assert_eq!(
            effective(&rules, Some("Discord - private conversation"), false),
            WritingStyle::Verbatim
        );
        let encoded = serde_json::to_string(&rules).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<StyleRule>>(&encoded).unwrap(),
            rules
        );
        let invalid = [StyleRule {
            app: "C:/apps/discord.exe".into(),
            style: WritingStyle::Chat,
        }];
        assert_eq!(
            effective(&invalid, Some("discord.exe"), true),
            WritingStyle::Clean
        );
    }

    #[test]
    fn chat_applies_conservative_cleanup_and_preserves_negation() {
        assert_eq!(
            apply("I I do not agree.", WritingStyle::Chat, &[]),
            ("I do not agree".into(), 2)
        );
        assert_eq!(
            apply("No, no, do not send it.  ", WritingStyle::Chat, &[]),
            ("No, no, do not send it  ".into(), 1)
        );
        assert_eq!(
            apply("I I do not agree.", WritingStyle::Clean, &[]),
            ("I do not agree.".into(), 1)
        );
    }

    #[test]
    fn chat_does_not_guess_about_technical_or_multisentence_punctuation() {
        for text in [
            "Is this ready?",
            "This is urgent!",
            "Let me think...",
            "Use NASA.",
            "Ask Dr.",
            "Price is 1.25.",
            "Set it to 100.",
            "Visit example.com.",
            "First. Second.",
            "Line one\nLine two.",
            "The path is C:\\tools.",
            "Use hi@example.com.",
            "Use option A.",
            "Use command: cargo check.",
            "Good night。",
        ] {
            assert_eq!(
                apply(text, WritingStyle::Chat, &[]),
                (text.into(), 0),
                "{text}"
            );
        }
    }
}
