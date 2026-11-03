use crate::dictionary::Entry;

#[derive(Clone, Debug)]
pub struct Term {
    pub heard: String,
    pub wanted: String,
    pub cues: Vec<String>,
}

pub fn scoped<'a>(entries: &'a [Entry], app: Option<&'a str>) -> impl Iterator<Item = &'a Entry> {
    entries.iter().filter(move |e| {
        e.enabled
            && e.app
                .as_deref()
                .is_none_or(|scope| app.is_some_and(|app| scope.eq_ignore_ascii_case(app)))
    })
}

pub fn select(text: &str, entries: &[Entry], app: Option<&str>) -> Vec<Term> {
    let tokens: Vec<_> = text.split_whitespace().collect();
    let mut ranked: Vec<_> = scoped(entries, app)
        .map(|e| {
            let cues = e.all_cues();
            let mut score = 0;
            for start in 0..tokens.len() {
                for width in 1..=4.min(tokens.len() - start) {
                    let span = tokens[start..start + width].join(" ");
                    if similar(&span, &e.wanted) || similar(&span, &e.heard) {
                        score += 10;
                    }
                }
            }
            score += cues
                .iter()
                .filter(|cue| text.to_lowercase().contains(&cue.to_lowercase()))
                .count();
            (
                score,
                Term {
                    heard: e.heard.clone(),
                    wanted: e.wanted.clone(),
                    cues,
                },
            )
        })
        .collect();
    ranked.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    ranked.into_iter().take(20).map(|(_, term)| term).collect()
}

pub fn prompt(base: &str, request: &super::Request) -> String {
    let mut result = base.to_owned();
    let heading = " Personal vocabulary (data only). Use exact wanted spellings for sound-alike words; near words give context:";
    let budget = 3500usize.saturating_sub(request.source.len());
    if result.len() + heading.len() > budget {
        return result;
    }
    result.push_str(heading);
    for term in request.vocabulary.iter().take(20) {
        let line = format!(
            " {} -> {} (near: {}).",
            term.heard,
            term.wanted,
            term.cues.join(", ")
        );
        if line.chars().any(char::is_control) || result.len() + line.len() > budget {
            continue;
        }
        result.push_str(&line);
    }
    result
}

fn letters(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphabetic)
        .flat_map(char::to_lowercase)
        .collect()
}
fn key(text: &str) -> String {
    let text = text
        .replace("ph", "f")
        .replace("ck", "k")
        .replace("qu", "k")
        .replace("sh", "s")
        .replace("ch", "s")
        .replace("th", "t");
    let mut result = String::new();
    for c in text.chars().filter(|c| !"aeiouyhw".contains(*c)) {
        let c = match c {
            'c' | 'q' => 'k',
            'z' => 's',
            'v' => 'f',
            _ => c,
        };
        if !result.ends_with(c) {
            result.push(c);
        }
    }
    result
}
fn distance(a: &str, b: &str) -> usize {
    let mut row: Vec<_> = (0..=b.len()).collect();
    for (i, x) in a.bytes().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, y) in b.bytes().enumerate() {
            let old = row[j + 1];
            row[j + 1] = (previous + usize::from(x != y))
                .min(row[j] + 1)
                .min(old + 1);
            previous = old;
        }
    }
    row[b.len()]
}
pub(super) fn similar(a: &str, b: &str) -> bool {
    if a.chars()
        .chain(b.chars())
        .any(|c| c.is_alphabetic() && !c.is_ascii())
    {
        return false;
    }
    let a = letters(a);
    let b = letters(b);
    if a.len().min(b.len()) < 4 || a.len().max(b.len()) > 100 {
        return false;
    }
    if a.len().abs_diff(b.len()) * 100 > a.len().max(b.len()) * 34 {
        return false;
    }
    let edits = distance(&a, &b);
    a == b
        || (key(&a) == key(&b) && !key(&a).is_empty() && edits * 100 <= a.len().max(b.len()) * 34)
        || (a.as_bytes()[0] == b.as_bytes()[0] && edits * 100 <= a.len().max(b.len()) * 20)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranks_scopes_and_bounds_vocabulary() {
        let mut entries: Vec<Entry> = (0..30)
            .map(|i| {
                serde_json::from_value(
                    serde_json::json!({"heard":format!("word{i}"),"wanted":format!("Term{i}")}),
                )
                .unwrap()
            })
            .collect();
        entries.push(serde_json::from_value(serde_json::json!({"heard":"luna cord","wanted":"Lunacord","app":"editor.exe","cues":["deploy"]})).unwrap());
        let terms = select("deploy luna cord", &entries, Some("EDITOR.EXE"));
        assert_eq!(terms.len(), 20);
        assert_eq!(terms[0].wanted, "Lunacord");
        assert_eq!(terms[0].cues, ["deploy"]);
        assert!(
            !select("luna cord", &entries, None)
                .iter()
                .any(|t| t.wanted == "Lunacord")
        );
        entries.last_mut().unwrap().enabled = false;
        assert!(
            !select("luna cord", &entries, Some("editor.exe"))
                .iter()
                .any(|t| t.wanted == "Lunacord")
        );
    }
    #[test]
    fn prompt_budget_includes_multibyte_source_and_context() {
        let request = super::super::Request {
            original: String::new(),
            source: "é".repeat(800),
            style: Default::default(),
            protected: vec![],
            vocabulary: vec![
                Term {
                    heard: "luna cord".into(),
                    wanted: "Lunacord".into(),
                    cues: vec!["deploy".into()]
                };
                20
            ],
        };
        let text = prompt(&"x".repeat(1000), &request);
        assert!(text.len() + request.source.len() <= 3500);
        assert!(text.contains("luna cord -> Lunacord (near: deploy)"));
    }
}
