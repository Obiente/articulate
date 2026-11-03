//! Conservative English sound keys for saved vocabulary.
//!
//! `similar` combines a simplified Metaphone consonant key with letter edit
//! similarity. Spaces do not contribute to either key. `is_distinctive` is the
//! shared eligibility gate; neither function rewrites text on its own.

pub const THRESHOLD: f32 = 0.92;
const MAX_LETTERS: usize = 160;

// Frequent prose words are unsuitable automatic replacement destinations, even
// when capitalized at a sentence boundary. This is deliberately conservative.
const COMMON: &str = "about above across actually after again against almost already also always another answer anyone anything around asked asking away back because become before began begin behind being below best better between both bring brought build built business call called came change changed check children city clear close come coming company complete could country course current days different doing done down during each early either else enough even ever every everyone everything example face fact family feel felt find first following found friends full further gave general getting give given going gone good great group hand happen hard having head hear heard help hello here high home house however idea important information inside instead into issue join joined just keep kind knew know known large last later least leave left less life likely line little live long look looking made main make making many matter maybe mean means might mind minutes money more morning most much must name near need needs never next night nothing number often once only open order other others outside over part people person place plan point possible present problem process program project public question quite read ready real really reason remember right room said same saved saying says school second seem send sent service several shall should show side since small software some someone something sometimes sound start started state still stop story such sure system take taken taking talk team tell than thank thanks that their theirs them themselves then there these they thing things think third this those though thought through time today together told tomorrow took toward tried true trying turn under until upon used useful using usually very visit visiting want wanted wants water ways week well went were what when where whether which while white whole whom whose why will wish within without word words work working world would write wrong years yesterday your yours also as at be been but by for get got had has have he her hers herself him himself his how if in its itself let lot may me mine my myself new now off old our ours ourselves out own put ran run saw say see she so too try two us use was way who yes yet you yourself yourselves there their weather whether flower flour peace piece plain plane board bored brake break here hear write right night knight mail male pair pear sale sail tale tail scene seen steel steal week weak wear where whole hole blue blew red read sea see son sun fair fare allowed aloud affect effect accept except desert dessert stationary stationery principal principle practice practise mercury earth march april june july august september october november december rust apple banana orange fruit table chair computer window button screen phone friend brother sister mother father child woman women man men human animal house office address garden street town village car train bus plane music movie sound video picture image photo food bread coffee dinner breakfast lunch happy angry sorry quiet loud beautiful simple easy difficult short longer smaller bigger strong light dark black green yellow purple brown grey gray color colour sight site page paper book message email letter password account access network internet server data file folder code language function support setting option sentence paragraph camera screen mouse keyboard dog cat bird horse plant tree leaf river ocean mountain field beach snow rain wind sun summer winter spring fall autumn read write reading writing spelling grammar English english French french German german Dutch dutch quick slow fast later soon please welcome goodbye tomorrow today yesterday now tonight none neither nor somewhere nowhere anyone anybody anything everybody somebody something myself ourselves yourself themselves itself oneanother";

/// Meaning-bearing words shared by correction learning and sound matching.
/// Numeric strings, number names, negations, weekdays, articles and pronouns
/// must never be consumed by a sound-alike span.
pub fn protected(word: &str) -> bool {
    if word.chars().any(char::is_numeric) {
        return true;
    }
    let lower = word.to_lowercase().replace('’', "'");
    lower.ends_with("n't")
        || matches!(
            lower.as_str(),
            "no" | "not"
                | "nor"
                | "neither"
                | "none"
                | "nowhere"
                | "never"
                | "without"
                | "dont"
                | "cannot"
                | "do"
                | "can"
                | "will"
                | "is"
                | "a"
                | "an"
                | "the"
                | "and"
                | "or"
                | "to"
                | "from"
                | "i"
                | "i'm"
                | "i've"
                | "i'll"
                | "i'd"
                | "me"
                | "my"
                | "mine"
                | "myself"
                | "you"
                | "your"
                | "yours"
                | "yourself"
                | "yourselves"
                | "you're"
                | "you've"
                | "you'll"
                | "you'd"
                | "he"
                | "him"
                | "his"
                | "himself"
                | "he's"
                | "he'll"
                | "he'd"
                | "she"
                | "her"
                | "hers"
                | "herself"
                | "she's"
                | "she'll"
                | "she'd"
                | "it"
                | "its"
                | "itself"
                | "it's"
                | "it'll"
                | "it'd"
                | "we"
                | "us"
                | "our"
                | "ours"
                | "ourselves"
                | "we're"
                | "we've"
                | "we'll"
                | "we'd"
                | "they"
                | "them"
                | "their"
                | "theirs"
                | "themselves"
                | "they're"
                | "they've"
                | "they'll"
                | "they'd"
                | "this"
                | "that"
                | "these"
                | "those"
                | "who"
                | "whom"
                | "whose"
                | "which"
                | "what"
                | "anyone"
                | "someone"
                | "everyone"
                | "nobody"
                | "nothing"
                | "something"
                | "everything"
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
                | "trillion"
                | "first"
                | "second"
                | "third"
                | "fourth"
                | "fifth"
                | "sixth"
                | "seventh"
                | "eighth"
                | "ninth"
                | "tenth"
                | "half"
                | "quarter"
                | "monday"
                | "tuesday"
                | "wednesday"
                | "thursday"
                | "friday"
                | "saturday"
                | "sunday"
        )
}

pub fn common(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    static WORDS: std::sync::OnceLock<std::collections::HashSet<&str>> = std::sync::OnceLock::new();
    WORDS
        .get_or_init(|| COMMON.split_ascii_whitespace().collect())
        .contains(lower.as_str())
}

/// English names and specialist terms only. Capitalization alone does not make
/// a common word distinctive. Unsupported scripts and punctuation abstain.
pub fn is_distinctive(term: &str) -> bool {
    let letters = term.bytes().filter(u8::is_ascii_alphabetic).count();
    letters >= 4
        && (letters >= 5
            || term
                .bytes()
                .any(|b| b.is_ascii_uppercase() || b.is_ascii_digit()))
        && term.len() <= MAX_LETTERS
        && term.bytes().all(|b| b.is_ascii_alphabetic() || b == b' ')
        && !term.split_ascii_whitespace().any(protected)
        && !term.split_ascii_whitespace().all(common)
}

#[derive(Clone, Debug)]
pub struct Key {
    letters: Vec<u8>,
    sounds: Vec<u8>,
}
impl Key {
    pub fn new(text: &str) -> Option<Self> {
        if text.len() > MAX_LETTERS || !text.bytes().all(|b| b.is_ascii_alphabetic() || b == b' ') {
            return None;
        }
        let letters: Vec<_> = text
            .bytes()
            .filter(u8::is_ascii_alphabetic)
            .map(|b| b.to_ascii_lowercase())
            .collect();
        if letters.len() < 4 {
            return None;
        }
        let mut sounds = Vec::with_capacity(letters.len());
        let mut i = usize::from(
            [b"kn", b"gn", b"pn", b"wr", b"ps"]
                .iter()
                .any(|p| letters.starts_with(*p)),
        );
        while i < letters.len() {
            let c = letters[i];
            let next = letters.get(i + 1).copied().unwrap_or(0);
            let prev = i.checked_sub(1).map_or(0, |p| letters[p]);
            let mut skip = 1;
            let code: &[u8] = match c {
                b'a' | b'e' | b'i' | b'o' | b'u' | b'y' if i == 0 => b"A",
                b'a' | b'e' | b'i' | b'o' | b'u' | b'y' => b"",
                b'b' if prev == b'm' && i + 1 == letters.len() => b"",
                b'b' | b'p' if c == b'p' && next == b'h' => {
                    skip = 2;
                    b"F"
                }
                b'b' | b'p' => b"P",
                b'c' if next == b'h' => {
                    skip = 2;
                    b"X"
                }
                b'c' if matches!(next, b'e' | b'i' | b'y') => b"S",
                b'c' | b'k' | b'q' => b"K",
                b'd' if next == b'g' && letters.get(i + 2).is_some_and(|b| b"eiy".contains(b)) => {
                    skip = 2;
                    b"J"
                }
                b'd' | b't' if c == b't' && next == b'h' => {
                    skip = 2;
                    b"0"
                }
                b'd' | b't' => b"T",
                b'f' | b'v' => b"F",
                b'g' if next == b'h' && i > 0 => {
                    skip = 2;
                    b""
                }
                b'g' if matches!(next, b'e' | b'i' | b'y') => b"J",
                b'g' => b"K",
                b'h' if (i == 0 || !b"aeiouy".contains(&prev)) && b"aeiouy".contains(&next) => b"H",
                b'h' => b"",
                b'j' => b"J",
                b'l' => b"L",
                b'm' => b"M",
                b'n' => b"N",
                b'r' => b"R",
                b's' if next == b'h' => {
                    skip = 2;
                    b"X"
                }
                b's' | b'z' => b"S",
                // Initial w/v confusion is common in dictated names. Letter
                // similarity still has to pass the same strict threshold.
                b'w' if i == 0 => {
                    if next == b'h' {
                        skip = 2;
                    }
                    b"F"
                }
                b'w' => b"",
                b'x' => b"KS",
                _ => b"",
            };
            for b in code {
                if sounds.last() != Some(b) {
                    sounds.push(*b);
                }
            }
            i += skip;
        }
        (sounds.len() >= 2).then_some(Self { letters, sounds })
    }
    pub(crate) fn bucket(&self) -> (u8, usize) {
        (self.sounds[0], self.sounds.len())
    }
    pub(crate) fn score(&self, other: &Self) -> f32 {
        let longest = self.sounds.len().max(other.sounds.len());
        let gap = self.sounds.len().abs_diff(other.sounds.len());
        if 1.0 - 0.75 * gap as f32 / (longest as f32) < THRESHOLD {
            return 0.0;
        }
        let phonetic = similarity(&self.sounds, &other.sounds);
        if 0.75 * phonetic + 0.25 < THRESHOLD {
            return 0.0;
        }
        let letters = similarity(&self.letters, &other.letters);
        if letters < 0.70 {
            return 0.0;
        }
        0.75 * phonetic + 0.25 * letters
    }
}

fn similarity(a: &[u8], b: &[u8]) -> f32 {
    if a == b {
        return 1.0;
    }
    let mut row = [0usize; MAX_LETTERS * 2 + 1];
    for (i, cell) in row.iter_mut().enumerate().take(b.len() + 1) {
        *cell = i;
    }
    for (i, left) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, right) in b.iter().enumerate() {
            let old = row[j + 1];
            row[j + 1] = (diagonal + usize::from(left != right))
                .min(row[j] + 1)
                .min(old + 1);
            diagonal = old;
        }
    }
    1.0 - row[b.len()] as f32 / a.len().max(b.len()) as f32
}

/// A bounded score in 0..=1, returning zero for short or unsupported input.
/// Callers must separately enforce distinctiveness, protected words and scope.
pub fn similar(a: &str, b: &str) -> f32 {
    match (Key::new(a), Key::new(b)) {
        (Some(a), Some(b)) => a.score(&b),
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn consonant_rules_handle_silent_starts_and_letter_groups() {
        for (left, right) in [
            ("knapsack", "napsack"),
            ("wrightson", "rightson"),
            ("phillips", "fillips"),
            ("sherryl", "cherryl"),
            ("jonn", "John"),
        ] {
            assert!(similar(left, right) >= THRESHOLD, "{left} {right}");
        }
        assert!(Key::new("Qwen").is_some());
        assert!(Key::new("a").is_none());
    }
    #[test]
    fn names_cross_spaces_and_vowels() {
        for (a, b) in [
            ("Obiente", "obi ante"),
            ("Obiente", "Obi Yente"),
            ("Vencord", "Ven cord"),
            ("Vencord", "when cord"),
            ("TypeScript", "type script"),
            ("Stephen", "Steven"),
        ] {
            assert!(similar(a, b) >= THRESHOLD, "{a} {b}: {}", similar(a, b));
            assert_eq!(similar(a, b), similar(b, a));
        }
    }
    #[test]
    fn unrelated_short_and_numeric_inputs_abstain() {
        for (a, b) in [
            ("Obiente", "there"),
            ("Obiente", "their"),
            ("Vencord", "record"),
            ("at", "ad"),
            ("ten", "10"),
            ("word2", "word3"),
            ("", ""),
            ("éclair", "eclair"),
        ] {
            assert!(similar(a, b) < THRESHOLD, "{a} {b}");
        }
    }
    #[test]
    fn eligibility_and_protections() {
        for text in ["Obiente", "Vencord", "Qwen", "Postgres", "obiente"] {
            assert!(is_distinctive(text), "{text}");
        }
        for text in [
            "There",
            "their",
            "Words",
            "Project",
            "hello",
            "Monday",
            "12345",
            "A",
            "Rust",
            "when",
            "you",
            "plain words",
        ] {
            assert!(!is_distinctive(text), "{text}");
        }
        for text in [
            "not", "didn’t", "we", "the", "twenty", "Friday", "12", "you've", "nor",
        ] {
            assert!(protected(text));
        }
        assert!(!protected("when"));
    }
    #[test]
    fn bounded_deterministic_and_case_independent() {
        for text in [
            "Obiente",
            "when cord",
            "Phonebook",
            "knapsack",
            "Psychology",
            "Wrightson",
        ] {
            assert_eq!(similar(text, text), 1.0);
            assert_eq!(similar(text, &text.to_uppercase()), 1.0);
        }
        assert_eq!(similar(&"x".repeat(161), "example"), 0.0);
    }
}
