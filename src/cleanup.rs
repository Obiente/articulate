// Conservative, transparent English disfluency rules. No generative rewriting.
// Raw text is retained. Other languages pass through except fragment stutters.
fn bare(s: &str) -> String {
    s.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
        .to_lowercase()
}

fn correction_group(s: &str) -> u8 {
    let s = bare(s);
    match s.as_str() {
        "monday" | "tuesday" | "wednesday" | "thursday" | "friday" | "saturday" | "sunday" => 1,
        "january" | "february" | "march" | "april" | "may" | "june" | "july" | "august"
        | "september" | "october" | "november" | "december" => 2,
        "zero" | "one" | "two" | "three" | "four" | "five" | "six" | "seven" | "eight" | "nine"
        | "ten" | "eleven" | "twelve" | "thirteen" | "fourteen" | "fifteen" | "sixteen"
        | "seventeen" | "eighteen" | "nineteen" | "twenty" | "thirty" | "forty" | "fifty"
        | "sixty" | "seventy" | "eighty" | "ninety" => 3,
        _ if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) => 3,
        _ => 0,
    }
}

pub fn apply(text: &str) -> (String, usize) {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut kept = Vec::new();
    let mut i = 0;
    let mut count = 0;
    while i < tokens.len() {
        let current = bare(tokens[i]);
        if i + 1 < tokens.len() {
            let next = bare(tokens[i + 1]);
            // Only function-word repeats; "very very", "no no", "had had",
            // and punctuation-separated sentences retain their meaning.
            let simple = matches!(
                current.as_str(),
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
            );
            let repeat = simple && current == next && tokens[i].chars().all(char::is_alphabetic);
            let fragment = tokens[i].ends_with('-')
                && current.len() >= 2
                && next.starts_with(&current)
                && current != next;
            if repeat || fragment {
                i += 1;
                count += 1;
                continue;
            }
        }
        // Explicit self-correction, confined to a single date/number token.
        // Bare "no" is never treated as a correction: it can be a negation.
        let marker_len = if tokens.get(i + 1).is_some_and(|s| bare(s) == "sorry") {
            1
        } else if tokens.get(i + 1).is_some_and(|s| bare(s) == "i")
            && tokens.get(i + 2).is_some_and(|s| bare(s) == "mean")
        {
            2
        } else {
            0
        };
        if marker_len > 0
            && let Some(replacement) = tokens.get(i + 1 + marker_len)
        {
            let group = correction_group(tokens[i]);
            if group != 0 && group == correction_group(replacement) {
                i += 1 + marker_len;
                count += 1;
                continue;
            }
        }
        kept.push(tokens[i]);
        i += 1;
    }
    if count == 0 {
        (text.to_owned(), 0)
    } else {
        (kept.join(" "), count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handles_stumbles_and_explicit_corrections() {
        assert_eq!(
            apply("I I sent the the pro- product on Tuesday, sorry, Thursday.").0,
            "I sent the product on Thursday."
        );
        assert_eq!(
            apply("It costs ten, I mean twenty euros.").0,
            "It costs twenty euros."
        );
    }
    #[test]
    fn preserves_negations_emphasis_and_ambiguous_corrections() {
        for s in [
            "No, no, do not send it.",
            "Very very important.",
            "She had had enough.",
            "I. I agree.",
            "I mean this is fine.",
            "Send it to Alice, sorry, Bob.",
            "The price is 1,245 euros, not 125.",
        ] {
            assert_eq!(apply(s).0, s);
        }
    }
    #[test]
    fn does_not_invent_missing_words_or_homophones() {
        let s = "The primary action is feigned. It is promotional evidence, not a functional transcript.";
        assert_eq!(apply(s).0, s);
    }
}
