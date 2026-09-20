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
    let lower = text.to_lowercase().replace('’', "'");
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
                    | "won't"
                    | "wouldn't"
                    | "shouldn't"
                    | "couldn't"
                    | "mustn't"
                    | "aren't"
                    | "weren't"
                    | "doesn't"
                    | "didn't"
                    | "hasn't"
                    | "haven't"
                    | "hadn't"
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
    for entry in entries.iter().flat_map(Entry::variants) {
        if !entry.enabled || protected(&entry.heard) != protected(&entry.wanted) {
            continue;
        }
        let (proposed, count) =
            dictionary::apply_in(text, std::slice::from_ref(&entry), app_ref(&request));
        if count != 1
            || occurrences(text, &entry.heard, entry.ignore_case).len() != 1
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
        "No unique saved vocabulary matches need review. Repeated phrases can be edited directly."
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
            count == 1
                && occurrences(
                    &request.original,
                    &candidate.entry.heard,
                    candidate.entry.ignore_case
                )
                .len()
                    == 1
                && expected != request.original
                && expected == candidate.proposed,
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
        entries
            .iter()
            .flat_map(Entry::variants)
            .any(|entry| entry == candidate.entry),
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

fn occurrences(text: &str, phrase: &str, ignore_case: bool) -> Vec<std::ops::Range<usize>> {
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '\'' || c == '’';
    let length = phrase.chars().count();
    let folded = phrase.to_lowercase();
    text.char_indices()
        .filter_map(|(start, _)| {
            let bytes = text[start..]
                .chars()
                .take(length)
                .map(char::len_utf8)
                .sum::<usize>();
            let end = start + bytes;
            let found = &text[start..end];
            let matches = if ignore_case {
                found.to_lowercase() == folded
            } else {
                found == phrase
            };
            (matches
                && (start == 0 || !text[..start].chars().next_back().is_some_and(word))
                && !text[end..].chars().next().is_some_and(word))
            .then_some(start..end)
        })
        .collect()
}

/// Explicit literal-wording cues suppress a recommendation, not the user's choice.
/// This limited English guard is not a substitute for semantic understanding.
pub fn literal_context(review: &Review, index: usize) -> bool {
    let Some(candidate) = review.request.candidates.get(index) else {
        return false;
    };
    let Some(span) = occurrences(
        &review.request.original,
        &candidate.entry.heard,
        candidate.entry.ignore_case,
    )
    .into_iter()
    .next() else {
        return false;
    };
    let start = review.request.original[..span.start]
        .rfind(['.', '!', '?', '\n'])
        .map_or(0, |i| i + 1);
    let end = review.request.original[span.end..]
        .find(['.', '!', '?', '\n'])
        .map_or(review.request.original.len(), |i| span.end + i);
    let sentence = review.request.original[start..end].to_lowercase();
    let words: Vec<_> = sentence
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    words
        .iter()
        .any(|word| matches!(*word, "literal" | "verbatim" | "quote" | "quoted"))
        || words.windows(2).any(|pair| {
            matches!(
                pair,
                ["exact", "words"] | ["exact", "wording"] | ["spelled", "as"]
            )
        })
}

/// Show the concrete effect of one choice without exposing the model prompt.
pub fn excerpt(text: &str, phrase: &str, ignore_case: bool) -> String {
    let Some(span) = occurrences(text, phrase, ignore_case).into_iter().next() else {
        return text.chars().take(260).collect();
    };
    let before: String = text[..span.start]
        .chars()
        .rev()
        .take(100)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let after: String = text[span.end..].chars().take(140).collect();
    format!(
        "{}{}{}{}{}",
        if before.len() < span.start { "…" } else { "" },
        before,
        &text[span.clone()],
        after,
        if after.len() < text.len() - span.end {
            "…"
        } else {
            ""
        }
    )
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
    let original = occurrences(raw, &candidate.entry.heard, candidate.entry.ignore_case);
    ensure!(
        original.len() == 1,
        "This phrase is no longer uniquely identifiable."
    );
    let heard = &raw[original[0].clone()];
    let (from, to) = if saved_spelling {
        (heard, candidate.entry.wanted.as_str())
    } else {
        (candidate.entry.wanted.as_str(), heard)
    };
    let matches = occurrences(preview, from, candidate.entry.ignore_case);
    let target_matches = occurrences(preview, to, candidate.entry.ignore_case);
    if matches.is_empty() && target_matches.len() == 1 {
        return Ok(preview.to_owned());
    }
    ensure!(
        matches.len() == 1
            && (target_matches.is_empty() || from.to_lowercase() == to.to_lowercase()),
        "This spelling appears more than once or both spellings are present. Edit it directly in the preview."
    );
    let mut result = preview.to_owned();
    result.replace_range(matches[0].clone(), to);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry() -> Entry {
        dictionary::validate("cube win", "Qwen").unwrap()
    }

    #[test]
    fn merged_context_review_keeps_the_matching_case_policy_and_rejects_removed_clause() {
        let mut entry = entry();
        entry.cues = vec!["engine".into()];
        entry.contexts.push(dictionary::Context {
            cues: vec!["model".into()],
            ignore_case: true,
        });
        let text = "Load the CUBE WIN model.";
        let review = reviewed(text, std::slice::from_ref(&entry));
        assert!(review.request.candidates[0].entry.ignore_case);
        assert!(review.request.candidates[0].entry.contexts.is_empty());
        assert_eq!(
            accept(&review, 0, text, std::slice::from_ref(&entry), None).unwrap(),
            "Load the Qwen model."
        );
        assert!(
            prepare(
                "Load the CUBE WIN engine.",
                std::slice::from_ref(&entry),
                None
            )
            .is_err()
        );
        entry.contexts.clear();
        assert!(accept(&review, 0, text, &[entry], None).is_err());
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
            ("won’t finish", "will finish"),
            ("didn't agree", "did agree"),
            ("shouldn’t change", "should change"),
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

    fn reviewed(text: &str, entries: &[Entry]) -> Review {
        let request = prepare(text, entries, None).unwrap();
        let scores = (0..request.candidates.len())
            .map(|candidate| Score {
                candidate,
                replace_score: 0.9,
            })
            .collect();
        Review { request, scores }
    }

    #[test]
    fn repeated_rules_are_not_offered_as_unusable_choices() {
        assert!(prepare("cube win and cube win", &[entry()], None).is_err());
        let entries = [
            entry(),
            dictionary::validate("post grass", "Postgres").unwrap(),
        ];
        let request = prepare("cube win and cube win use post grass", &entries, None).unwrap();
        assert_eq!(request.candidates.len(), 1);
        assert_eq!(request.candidates[0].entry.wanted, "Postgres");
        let mut scoped = entry();
        scoped.cues = vec!["model".into()];
        assert!(prepare("Load the cube win model. Quote cube win.", &[scoped], None).is_err());
    }

    #[test]
    fn review_choices_handle_case_and_already_applied_spelling() {
        let entries = [dictionary::validate("at Casey", "@Casey").unwrap()];
        let raw = "At Casey please reply.";
        let review = reviewed(raw, &entries);
        let saved = "@Casey please reply.";
        for (text, use_saved, expected) in [
            (raw, true, saved),
            (saved, true, saved),
            (saved, false, raw),
            (raw, false, raw),
        ] {
            assert_eq!(
                apply_choice(
                    &review,
                    0,
                    use_saved,
                    Preview {
                        raw,
                        text,
                        baseline: text
                    },
                    &entries,
                    None
                )
                .unwrap(),
                expected
            );
        }
    }

    #[test]
    fn choices_for_separate_rules_preserve_each_other() {
        let entries = [
            entry(),
            dictionary::validate("post grass", "Postgres").unwrap(),
        ];
        let raw = "Load cube win and post grass.";
        let review = reviewed(raw, &entries);
        let first = apply_choice(
            &review,
            0,
            true,
            Preview {
                raw,
                text: raw,
                baseline: raw,
            },
            &entries,
            None,
        )
        .unwrap();
        let second = apply_choice(
            &review,
            1,
            true,
            Preview {
                raw,
                text: &first,
                baseline: &first,
            },
            &entries,
            None,
        )
        .unwrap();
        assert_eq!(second, "Load Qwen and Postgres.");
        let restored = apply_choice(
            &review,
            0,
            false,
            Preview {
                raw,
                text: &second,
                baseline: &second,
            },
            &entries,
            None,
        )
        .unwrap();
        assert_eq!(restored, "Load cube win and Postgres.");
    }

    #[test]
    fn both_spellings_in_preview_cannot_select_the_wrong_occurrence() {
        let entries = [entry()];
        let raw = "Compare cube win with Qwen.";
        let review = reviewed(raw, &entries);
        for saved in [false, true] {
            assert!(
                apply_choice(
                    &review,
                    0,
                    saved,
                    Preview {
                        raw,
                        text: raw,
                        baseline: raw
                    },
                    &entries,
                    None
                )
                .is_err()
            );
        }
    }

    #[test]
    fn literal_abstention_is_explicit_and_sentence_local() {
        for text in [
            "Quote cube win exactly.",
            "Keep the exact words cube win.",
            "Write cube win verbatim.",
        ] {
            let review = reviewed(text, &[entry()]);
            assert!(literal_context(&review, 0));
            assert_eq!(review.scores[0].replace_score, 0.9);
        }
        for text in [
            "Load cube win for the model.",
            "Quote the next sentence. Load cube win.",
            "Load cube win. Quote something else.",
        ] {
            assert!(!literal_context(&reviewed(text, &[entry()]), 0));
        }
    }

    #[test]
    fn unicode_excerpt_and_matches_preserve_boundaries() {
        assert_eq!(occurrences("Änne and Ännes", "änne", true), vec![0..5]);
        assert!(occurrences("speaker’s", "speaker", true).is_empty());
        let text = format!("{} cube win {}", "é".repeat(200), "世".repeat(200));
        let shown = excerpt(&text, "cube win", false);
        assert!(shown.starts_with('…') && shown.ends_with('…'));
        assert!(shown.contains("cube win"));
        assert!(shown.chars().count() <= 250);
    }
}
