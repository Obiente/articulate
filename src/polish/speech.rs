//! Bounded speech repairs shared by the editor input and its acceptance gate.
//! Keep paragraph boundaries and protect saved phrases before asking the model
//! for punctuation. A rejected rewrite can still return these useful repairs.

fn bare(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
        .to_lowercase()
}

// Repetition matching may ignore ASR casing and ordinary comma/period
// placement, but not currency, operators, identifiers or question marks.
// `bare` is only suitable for recognizing known speech markers.
fn repeated_word(word: &str) -> String {
    if word.chars().any(char::is_alphanumeric) {
        word.trim_matches([',', '.']).to_lowercase()
    } else {
        word.to_owned()
    }
}

fn function_word(word: &str) -> bool {
    matches!(
        word,
        "i" | "we"
            | "you"
            | "they"
            | "the"
            | "a"
            | "an"
            | "to"
            | "of"
            | "and"
            | "my"
            | "our"
            | "it"
    )
}

fn repair_restatement(tokens: &mut Vec<&str>) {
    let mut at = 0;
    while at < tokens.len() {
        let marker = if bare(tokens[at]) == "sorry" {
            1
        } else if bare(tokens[at]) == "i"
            && tokens
                .get(at + 1)
                .is_some_and(|token| bare(token) == "mean")
        {
            2
        } else {
            at += 1;
            continue;
        };
        // Accept a fully restated short clause with one corrected term. Never
        // infer an abandoned clause from bare "no" or an unrelated apology.
        let width = (3..=at.min(24)).rev().find(|&width| {
            let start = at - width;
            let end = at + marker + width;
            if end > tokens.len()
                || (start > 0 && !tokens[start - 1].ends_with(['.', '?', '!']))
                || (end < tokens.len() && !tokens[end - 1].ends_with(['.', '?', '!']))
            {
                return false;
            }
            let left = &tokens[start..at];
            let right = &tokens[at + marker..end];
            !left[..width - 1]
                .iter()
                .chain(&right[..width - 1])
                .any(|token| token.ends_with(['.', '?', '!']))
                && left[..2]
                    .iter()
                    .zip(&right[..2])
                    .all(|(a, b)| bare(a) == bare(b))
                && left
                    .iter()
                    .zip(right)
                    .filter(|(a, b)| bare(a) != bare(b))
                    .count()
                    == 1
        });
        if let Some(width) = width {
            // The replacement occupies the old sentence start. Preserve the
            // speaker's original casing for that same token, including product
            // names such as iPhone, without guessing how to capitalize new text.
            let original_start = tokens[at - width];
            if original_start.to_lowercase() == tokens[at + marker].to_lowercase() {
                tokens[at + marker] = original_start;
            }
            tokens.drain(at - width..at + marker);
            at -= width;
        } else {
            at += marker;
        }
    }
}

pub(crate) fn clean(source: &str, protected: &[String]) -> String {
    let result = source
        .split('\n')
        .map(|original_line| {
            let (line, _) = crate::cleanup::apply(original_line);
            let mut tokens: Vec<&str> = line.split_whitespace().collect();
            let mut capitalize = std::collections::HashSet::new();
            let mut sentence_start = true;
            let mut removed_leading = false;
            tokens.retain(|token| {
                let filler = matches!(
                    token.trim_end_matches([',', '.', ';', '!', '?', '…']),
                    "um" | "Um" | "uh" | "Uh" | "erm" | "Erm"
                );
                if filler {
                    removed_leading |= sentence_start;
                    return false;
                }
                if removed_leading {
                    capitalize.insert(token.as_ptr() as usize);
                    removed_leading = false;
                }
                sentence_start = token.ends_with(['.', '?', '!']);
                true
            });
            repair_restatement(&mut tokens);
            let mut at = 0;
            while at < tokens.len() {
                let width = (1..=((tokens.len() - at) / 2).min(40))
                    .rev()
                    .find(|&width| {
                        let left = &tokens[at..at + width];
                        let right = &tokens[at + width..at + width * 2];
                        let equal = left
                            .iter()
                            .zip(right)
                            .all(|(a, b)| repeated_word(a) == repeated_word(b));
                        // Single-word emphasis (very very, no no) and grammatical
                        // repetitions (had had) are not disfluencies. Short
                        // restarts must contain a function word and no sentence
                        // boundary. Whole exact clauses can be deduplicated.
                        equal
                            && if width < 3 {
                                left.iter().any(|word| function_word(&bare(word)))
                                    && left.iter().all(|word| {
                                        word.chars().all(|c| c.is_alphanumeric() || c == '\'')
                                    })
                            } else {
                                !left[..width - 1]
                                    .iter()
                                    .any(|word| word.ends_with(['.', '?', '!']))
                            }
                    });
                if let Some(width) = width {
                    tokens.drain(at..at + width);
                    at = 0;
                } else {
                    at += 1;
                }
            }
            let cleaned = tokens
                .iter()
                .map(|token| {
                    if capitalize.contains(&(token.as_ptr() as usize))
                        && matches!(
                            bare(token).as_str(),
                            "i" | "we"
                                | "you"
                                | "they"
                                | "it"
                                | "he"
                                | "she"
                                | "the"
                                | "a"
                                | "an"
                                | "so"
                                | "when"
                                | "what"
                                | "where"
                                | "why"
                                | "how"
                                | "please"
                                | "can"
                                | "could"
                                | "would"
                                | "do"
                                | "did"
                                | "is"
                                | "are"
                                | "will"
                                | "this"
                                | "that"
                                | "these"
                                | "those"
                        )
                    {
                        let mut word = (*token).to_owned();
                        if word.starts_with(|c: char| c.is_ascii_lowercase()) {
                            word[..1].make_ascii_uppercase();
                        }
                        word
                    } else {
                        (*token).to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            if cleaned
                == original_line
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            {
                original_line.to_owned()
            } else {
                let leading =
                    &original_line[..original_line.len() - original_line.trim_start().len()];
                let trailing = &original_line[original_line.trim_end().len()..];
                format!("{leading}{cleaned}{trailing}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if protected.iter().any(|phrase| {
        !phrase.is_empty()
            && super::guard::protected_occurrences(source, phrase)
                != super::guard::protected_occurrences(&result, phrase)
    }) {
        source.to_owned()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repetition_cleanup_preserves_different_symbols_and_questions() {
        for source in [
            "Send $42 now. Send €42 now.",
            "Use x = y. Use x != y.",
            "Use --force here. Use force here.",
            "Send it now? Send it now.",
            "Set x to -42. Set x to 42.",
        ] {
            assert_eq!(clean(source, &[]), source, "{source}");
        }
        assert_eq!(clean("Send $42 now. Send $42 now.", &[]), "Send $42 now.");
    }

    #[test]
    fn removes_fillers_short_restarts_and_duplicate_clauses() {
        assert_eq!(
            clean("I was checking up on the um on the invoice.", &[]),
            "I was checking up on the invoice."
        );
        assert_eq!(
            clean("I I need to need to send it. I need to send it.", &[]),
            "I need to send it."
        );
        assert_eq!(
            clean(
                "Send 42 euros. Send 42 euros.\n\nDo not send it today.",
                &[]
            ),
            "Send 42 euros.\n\nDo not send it today."
        );
    }

    #[test]
    fn honors_explicit_date_and_number_repairs_but_not_ambiguous_negation() {
        assert_eq!(
            clean("Meet Tuesday, sorry, Thursday at 3, I mean 4 pm.", &[]),
            "Meet Thursday at 4 pm."
        );
        for source in [
            "No, no, do not send it.",
            "Very very important.",
            "She had had enough.",
            "What it is is unclear.",
            "Carry on on Monday.",
            "I. I agree.",
            "Send it to Alice, sorry, Bob.",
            "Tuesday, no Thursday.",
        ] {
            assert_eq!(clean(source, &[]), source);
        }
        assert_eq!(clean("Ask The The.", &["The The".into()]), "Ask The The.");
    }

    #[test]
    fn accepts_explicit_restatement_without_guessing_about_apologies() {
        assert_eq!(
            clean(
                "Send the invoice to Alice, I mean send the invoice to Bob.",
                &[]
            ),
            "Send the invoice to Bob."
        );
        assert_eq!(
            clean("Meet me on Tuesday, sorry meet me on Thursday.", &[]),
            "Meet me on Thursday."
        );
        assert_eq!(
            clean(
                "Send the invoice to Alice, I mean send the invoice to Bob.",
                &["Alice".into()]
            ),
            "Send the invoice to Alice, I mean send the invoice to Bob."
        );
        for source in [
            "I called Alice. Sorry, she was busy.",
            "I sent it, I mean I could send it.",
            "Call Alice. Call Bob.",
            "I will call Alice, I mean I will call Bob tomorrow.",
        ] {
            assert_eq!(clean(source, &[]), source);
        }
    }

    #[test]
    fn restatement_reuses_sentence_start_case_without_capitalizing_technical_text() {
        assert_eq!(
            clean(
                "Okay. Send the invoice to Alice, I mean send the invoice to Bob.",
                &[]
            ),
            "Okay. Send the invoice to Bob."
        );
        assert_eq!(
            clean(
                "iPhone uses version three, I mean iphone uses version four.",
                &[]
            ),
            "iPhone uses version four."
        );
        assert_eq!(
            clean(
                "cargo uses version three, I mean cargo uses version four.",
                &[]
            ),
            "cargo uses version four."
        );
    }
}
