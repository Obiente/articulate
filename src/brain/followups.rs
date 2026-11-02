//! Exports reviewed action items without guessing owners or reminder dates.
use super::{Item, Kind};
use sha2::{Digest, Sha256};
fn actions(items: &[Item]) -> impl Iterator<Item = &Item> {
    items.iter().filter(|i| i.kind == Kind::Action)
}
pub fn markdown(items: &[Item]) -> String {
    actions(items)
        .map(|i| {
            format!(
                "- [ ] {}\n",
                i.text
                    .lines()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .replace('[', "\\[")
                    .replace(']', "\\]")
            )
        })
        .collect()
}
fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\r', "")
        .replace('\n', "\\n")
        .replace(';', "\\;")
        .replace(',', "\\,")
}
fn line(output: &mut String, value: &str) {
    let mut width = 0;
    for c in value.chars() {
        if width + c.len_utf8() > 75 {
            output.push_str("\r\n ");
            width = 1;
        }
        output.push(c);
        width += c.len_utf8();
    }
    output.push_str("\r\n");
}
/// UTC stamp must be supplied by the caller as YYYYMMDDTHHMMSSZ.
/// Tasks have no inferred due date; callers can add reminders after import.
pub fn ics(items: &[Item], session: &str, timestamp: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        timestamp.len() == 16
            && timestamp.as_bytes()[8] == b'T'
            && timestamp.ends_with('Z')
            && timestamp
                .bytes()
                .enumerate()
                .all(|(i, b)| i == 8 || i == 15 || b.is_ascii_digit()),
        "Invalid calendar timestamp"
    );
    let mut output =
        String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Articulate//Follow-ups//EN\r\n");
    for (index, item) in actions(items).enumerate() {
        line(&mut output, "BEGIN:VTODO");
        line(
            &mut output,
            &format!(
                "UID:{:x}@articulate",
                Sha256::digest(format!("{session}\0{index}\0{}", item.text))
            ),
        );
        line(&mut output, &format!("DTSTAMP:{timestamp}"));
        line(&mut output, &format!("SUMMARY:{}", escape(&item.text)));
        line(&mut output, "STATUS:NEEDS-ACTION");
        line(&mut output, "END:VTODO");
    }
    line(&mut output, "END:VCALENDAR");
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_facts_and_escapes_calendar_injection() {
        let items = vec![
            Item {
                kind: Kind::Fact,
                text: "fact".into(),
                section: 1,
                sources: vec![],
            },
            Item {
                kind: Kind::Action,
                text: "Review, plan;\nEND:VTODO".into(),
                section: 1,
                sources: vec![],
            },
        ];
        assert_eq!(markdown(&items), "- [ ] Review, plan; END:VTODO\n");
        let text = ics(&items, "synthetic", "20260101T120000Z").unwrap();
        assert!(text.contains("SUMMARY:Review\\, plan\\;\\nEND:VTODO"));
        assert_eq!(text.matches("\r\nEND:VTODO\r\n").count(), 1);
        assert!(ics(&items, "synthetic", "invalid").is_err());
        let mut folded = String::new();
        line(&mut folded, &"é".repeat(100));
        assert!(folded.split("\r\n").all(|l| l.len() <= 75));
    }
}
