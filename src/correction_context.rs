//! Review-only scoring of existing vocabulary rules. The classifier never
//! invents replacement text and cannot change live typing or saved vocabulary.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dictionary::{self, Entry};

pub const QUESTION: &str = "Should this proposed dictionary correction replace the original text?";
pub const KEEP: &str = "Keep the original text unchanged; the replacement is unsafe, unconfirmed, ambiguous, or out of scope.";
pub const REPLACE: &str =
    "Apply this confirmed dictionary replacement in its matching context without changing meaning.";
pub const MAX_CANDIDATES: usize = 8;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub entry: Entry,
    pub proposed: String,
    pub context: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub original: String,
    pub app: Option<String>,
    pub candidates: Vec<Candidate>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Score {
    pub candidate: usize,
    /// Model preference, not calibrated confidence in correctness.
    pub replace_score: f32,
}

#[derive(Clone)]
pub struct Review {
    pub request: Request,
    pub scores: Vec<Score>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Output {
    pub request_hash: String,
    pub scores: Vec<Score>,
}

fn protected(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut words: Vec<_> = lower
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|word| {
            matches!(
                *word,
                "not"
                    | "never"
                    | "no"
                    | "without"
                    | "cannot"
                    | "can't"
                    | "don't"
                    | "isn't"
                    | "wasn't"
                    | "zero"
                    | "one"
                    | "two"
                    | "three"
                    | "four"
                    | "five"
                    | "six"
                    | "seven"
                    | "eight"
                    | "nine"
                    | "ten"
                    | "eleven"
                    | "twelve"
                    | "thirteen"
                    | "fourteen"
                    | "fifteen"
                    | "sixteen"
                    | "seventeen"
                    | "eighteen"
                    | "nineteen"
                    | "twenty"
                    | "thirty"
                    | "forty"
                    | "fifty"
                    | "sixty"
                    | "seventy"
                    | "eighty"
                    | "ninety"
                    | "hundred"
                    | "thousand"
                    | "million"
                    | "billion"
                    | "first"
                    | "second"
                    | "third"
                    | "fourth"
                    | "fifth"
                    | "half"
                    | "quarter"
            )
        })
        .map(str::to_owned)
        .collect();
    // Preserve punctuation around numbers, signs, percentages and currency.
    words.push(
        text.chars()
            .filter(|c| c.is_numeric() || "+-.,%$€£".contains(*c))
            .collect(),
    );
    words
}

pub fn prepare(text: &str, entries: &[Entry], app: Option<&str>) -> Result<Request> {
    ensure!(
        !text.trim().is_empty() && text.len() <= 16_000,
        "Use a finished passage under 16,000 bytes."
    );
    let app = app.map(str::to_lowercase);
    let mut request = Request {
        original: text.into(),
        app,
        candidates: Vec::new(),
    };
    for entry in entries {
        if !entry.enabled || protected(&entry.heard) != protected(&entry.wanted) {
            continue;
        }
        let (proposed, count) =
            dictionary::apply_in(text, std::slice::from_ref(entry), app_ref(&request));
        if count == 0
            || proposed == text
            || request.candidates.iter().any(|c| c.proposed == proposed)
        {
            continue;
        }
        // Context stays in a typed field. Model prompt text is never parsed back
        // into confirmation, scope or an executable replacement instruction.
        let common = text
            .chars()
            .zip(proposed.chars())
            .take_while(|(a, b)| a == b)
            .count();
        let chars: Vec<_> = text.chars().collect();
        let start = common.saturating_sub(160);
        let end = (common + 320).min(chars.len());
        let sentence: String = chars[start..end].iter().collect();
        let context = format!(
            "App: {}. Sentence: {}. Vocabulary context cues: {}.",
            request.app.as_deref().unwrap_or("unspecified"),
            sentence,
            entry.cues.join(", ")
        );
        request.candidates.push(Candidate {
            entry: entry.clone(),
            proposed,
            context,
        });
        if request.candidates.len() == MAX_CANDIDATES {
            break;
        }
    }
    validate(&request)?;
    Ok(request)
}

fn app_ref(request: &Request) -> Option<&str> {
    request.app.as_deref()
}

pub(crate) fn validate(request: &Request) -> Result<()> {
    ensure!(
        !request.original.trim().is_empty() && request.original.len() <= 16_000,
        "Unsupported correction passage."
    );
    ensure!(
        request
            .app
            .as_ref()
            .is_none_or(|app| app.len() <= 100 && !app.chars().any(char::is_control)),
        "Invalid app context."
    );
    ensure!(
        !request.candidates.is_empty() && request.candidates.len() <= MAX_CANDIDATES,
        "No matching saved vocabulary corrections need review in this passage."
    );
    for candidate in &request.candidates {
        dictionary::validate(&candidate.entry.heard, &candidate.entry.wanted)?;
        ensure!(
            candidate.entry.enabled
                && candidate.context.len() <= 4096
                && candidate.proposed.len() <= 24_000,
            "Invalid correction candidate."
        );
        ensure!(
            protected(&candidate.entry.heard) == protected(&candidate.entry.wanted),
            "This correction changes a protected number or negation."
        );
        let (expected, count) = dictionary::apply_in(
            &request.original,
            std::slice::from_ref(&candidate.entry),
            app_ref(request),
        );
        ensure!(
            count > 0 && expected != request.original && expected == candidate.proposed,
            "This correction is not eligible in the supplied app and sentence."
        );
    }
    Ok(())
}

pub(crate) fn hash(request: &Request) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(request)?)
    ))
}

pub(crate) fn model_request(candidate: &Candidate) -> assort_core::Request {
    assort_core::Request {
        state: format!(
            "Original: {}\nProposed: {}\nConfirmed dictionary entry: yes\nApp scope matches: yes\nContext: {}",
            candidate.entry.heard, candidate.entry.wanted, candidate.context
        ),
        questions: vec![assort_core::Question {
            id: "correction".into(),
            text: QUESTION.into(),
            candidates: vec![
                assort_core::Candidate::new("keep_original", KEEP),
                assort_core::Candidate::new("replace", REPLACE),
            ],
        }],
    }
}

pub(crate) fn review(request: Request, bytes: &[u8]) -> Result<Review> {
    validate(&request)?;
    ensure!(
        bytes.len() <= 16_384,
        "Correction scores exceed the output limit."
    );
    let output: Output = serde_json::from_slice(bytes)?;
    ensure!(
        output.request_hash == hash(&request)? && output.scores.len() == request.candidates.len(),
        "The correction review belongs to another passage."
    );
    for (index, score) in output.scores.iter().enumerate() {
        ensure!(
            score.candidate == index
                && score.replace_score.is_finite()
                && (0.0..=1.0).contains(&score.replace_score),
            "Invalid correction score."
        );
    }
    Ok(Review {
        request,
        scores: output.scores,
    })
}

/// An explicit click can apply a saved candidate only while all relevant state
/// still matches the review. A high model score never calls this automatically.
pub fn accept(
    review: &Review,
    index: usize,
    text: &str,
    entries: &[Entry],
    app: Option<&str>,
) -> Result<String> {
    ensure!(
        text == review.request.original && app.map(str::to_lowercase) == review.request.app,
        "The passage or app changed. Review it again before applying a suggestion."
    );
    validate(&review.request)?;
    let candidate = review
        .request
        .candidates
        .get(index)
        .ok_or_else(|| anyhow::anyhow!("Unknown correction suggestion."))?;
    ensure!(
        entries.contains(&candidate.entry),
        "The vocabulary rule changed. Review it again before applying a suggestion."
    );
    Ok(candidate.proposed.clone())
}

/// Apply one explicit review choice to an otherwise unchanged finished preview.
/// Multiple matches are deliberately left to manual editing.
pub struct Preview<'a> {
    pub raw: &'a str,
    pub text: &'a str,
    pub baseline: &'a str,
}

pub fn apply_choice(
    review: &Review,
    index: usize,
    saved_spelling: bool,
    current: Preview<'_>,
    entries: &[Entry],
    app: Option<&str>,
) -> Result<String> {
    let Preview {
        raw,
        text: preview,
        baseline,
    } = current;
    let _ = accept(review, index, raw, entries, app)?;
    ensure!(
        preview == baseline,
        "The edited preview changed. Review it again first."
    );
    let candidate = &review.request.candidates[index];
    let (from, to) = if saved_spelling {
        (&candidate.entry.heard, &candidate.entry.wanted)
    } else {
        (&candidate.entry.wanted, &candidate.entry.heard)
    };
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let matches: Vec<_> = preview
        .match_indices(from)
        .filter(|(at, _)| {
            (*at == 0 || !preview[..*at].chars().next_back().is_some_and(is_word))
                && !preview[*at + from.len()..]
                    .chars()
                    .next()
                    .is_some_and(is_word)
        })
        .map(|(at, _)| at)
        .collect();
    ensure!(
        matches.len() == 1,
        "This spelling appears more than once or changed. Edit it directly in the preview."
    );
    let mut result = preview.to_owned();
    result.replace_range(matches[0]..matches[0] + from.len(), to);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry() -> Entry {
        dictionary::validate("cube win", "Qwen").unwrap()
    }

    #[test]
    fn uses_saved_rule_sentence_and_app_without_generating_text() {
        let mut entry = entry();
        entry.app = Some("code.exe".into());
        entry.cues = vec!["model".into()];
        let request = prepare(
            "Load the cube win model.",
            std::slice::from_ref(&entry),
            Some("CODE.EXE"),
        )
        .unwrap();
        assert_eq!(request.candidates[0].proposed, "Load the Qwen model.");
        assert!(request.candidates[0].context.contains("code.exe"));
        assert!(
            prepare(
                "Load cube win for dinner.",
                std::slice::from_ref(&entry),
                Some("code.exe")
            )
            .is_err()
        );
        assert!(prepare("Load the cube win model.", &[entry], Some("chat.exe")).is_err());
    }

    #[test]
    fn stale_text_scope_and_rule_reject_acceptance_even_with_high_score() {
        let entry = entry();
        let request = prepare("Load cube win.", std::slice::from_ref(&entry), None).unwrap();
        let review = Review {
            request,
            scores: vec![Score {
                candidate: 0,
                replace_score: 1.0,
            }],
        };
        assert_eq!(
            accept(
                &review,
                0,
                "Load cube win.",
                std::slice::from_ref(&entry),
                None
            )
            .unwrap(),
            "Load Qwen."
        );
        assert!(
            accept(
                &review,
                0,
                "Unload cube win.",
                std::slice::from_ref(&entry),
                None
            )
            .is_err()
        );
        assert!(accept(&review, 0, "Load cube win.", &[entry], Some("code.exe")).is_err());
        assert!(accept(&review, 0, "Load cube win.", &[], None).is_err());
    }

    #[test]
    fn protects_numbers_negations_and_rejects_forged_candidates() {
        for (before, after) in [
            ("not ready", "ready"),
            ("two days", "three days"),
            ("10 euros", "100 euros"),
        ] {
            assert!(
                prepare(
                    before,
                    &[dictionary::validate(before, after).unwrap()],
                    None
                )
                .is_err()
            );
        }
        let mut request = prepare("Load cube win.", &[entry()], None).unwrap();
        request.candidates[0].proposed = "Invented instructions".into();
        assert!(validate(&request).is_err());
    }

    #[test]
    fn explicit_choice_preserves_other_edits_and_refuses_multiple_matches() {
        let entry = entry();
        let request = prepare("please load cube win", std::slice::from_ref(&entry), None).unwrap();
        let review = Review {
            request,
            scores: vec![Score {
                candidate: 0,
                replace_score: 0.1,
            }],
        };
        let preview = "Please load Qwen.";
        assert_eq!(
            apply_choice(
                &review,
                0,
                false,
                Preview {
                    raw: "please load cube win",
                    text: preview,
                    baseline: preview
                },
                std::slice::from_ref(&entry),
                None
            )
            .unwrap(),
            "Please load cube win."
        );
        assert!(
            apply_choice(
                &review,
                0,
                false,
                Preview {
                    raw: "please load cube win",
                    text: "Qwen and Qwen",
                    baseline: "Qwen and Qwen"
                },
                std::slice::from_ref(&entry),
                None
            )
            .is_err()
        );
        assert!(
            apply_choice(
                &review,
                0,
                false,
                Preview {
                    raw: "please load cube win",
                    text: "Edited preview",
                    baseline: preview
                },
                &[entry],
                None
            )
            .is_err()
        );
    }

    #[test]
    fn scores_must_belong_to_the_exact_request_and_candidate_order() {
        let request = prepare("Load cube win.", &[entry()], None).unwrap();
        let mut output = Output {
            request_hash: hash(&request).unwrap(),
            scores: vec![Score {
                candidate: 0,
                replace_score: 0.5,
            }],
        };
        assert!(review(request.clone(), &serde_json::to_vec(&output).unwrap()).is_ok());
        output.scores[0].candidate = 1;
        assert!(review(request.clone(), &serde_json::to_vec(&output).unwrap()).is_err());
        output.scores[0].candidate = 0;
        output.request_hash = "different".into();
        assert!(review(request, &serde_json::to_vec(&output).unwrap()).is_err());
    }
}
