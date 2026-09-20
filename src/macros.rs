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
