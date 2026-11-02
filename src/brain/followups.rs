//! Exports reviewed action items without guessing owners or reminder dates.
use super::{Item, Kind};
use sha2::{Digest, Sha256};
fn actions(items: &[Item]) -> impl Iterator<Item = &Item> {
    items.iter().filter(|i| i.kind == Kind::Action)
}

/// Generated action items the user has kept in the document's notes. An item
/// whose paragraph was removed or rewritten is no longer exported, so only
/// wording the user has seen and left in place leaves the app.
pub fn reviewed(session: &crate::history::Session) -> Vec<Item> {
    let Some(draft) = &session.generated_summary else {
        return Vec::new();
    };
    let blocks: Vec<&str> = session
        .personal_notes
        .split("

")
        .map(str::trim)
        .filter(|block| !block.is_empty())
        .collect();
    let mut kept: Vec<Item> = Vec::new();
    for item in actions(&draft.items) {
        let text = item.text.trim();
        if !text.is_empty()
            && blocks.contains(&text)
            && !kept.iter().any(|existing| existing.text.trim() == text)
        {
            kept.push(item.clone());
        }
    }
    kept
}

/// Number of exportable follow-ups, without copying them.
pub fn reviewed_count(session: &crate::history::Session) -> usize {
    let Some(draft) = &session.generated_summary else {
        return 0;
    };
    let mut seen: Vec<&str> = Vec::new();
    for item in actions(&draft.items) {
        let text = item.text.trim();
        if !text.is_empty()
            && !seen.contains(&text)
            && session
                .personal_notes
                .split("

")
                .any(|block| block.trim() == text)
        {
            seen.push(text);
        }
    }
    seen.len()
}

/// A calendar timestamp (`YYYYMMDDTHHMMSSZ`) for Unix milliseconds.
pub fn utc_stamp(ms: u64) -> anyhow::Result<String> {
    let time = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)?;
    Ok(format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        time.year(),
        u8::from(time.month()),
        time.day(),
        time.hour(),
        time.minute(),
        time.second()
    ))
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
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect::<String>()
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

/// Export tasks with an explicitly selected UTC reminder for every action.
pub fn ics_with_reminder(
    items: &[Item],
    session: &str,
    timestamp: &str,
    reminder: &str,
) -> anyhow::Result<String> {
    // Reuse timestamp validation before interpolating calendar properties.
    ics(&[], session, reminder)?;
    Ok(ics(items, session, timestamp)?.replace(
        "STATUS:NEEDS-ACTION\r\n",
        &format!("STATUS:NEEDS-ACTION\r\nDUE:{reminder}\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Meeting follow-up\r\nTRIGGER;VALUE=DATE-TIME:{reminder}\r\nEND:VALARM\r\n"),
    ))
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
    let number = |range: std::ops::Range<usize>| timestamp[range].parse::<u32>().unwrap();
    let year = number(0..4);
    let month = number(4..6);
    let day = number(6..8);
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    anyhow::ensure!(
        year > 0
            && day > 0
            && day <= days
            && number(9..11) < 24
            && number(11..13) < 60
            && number(13..15) < 60,
        "Invalid calendar date"
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
        assert!(ics(&items, "synthetic", "20260230T120000Z").is_err());
        assert!(
            ics_with_reminder(&items, "synthetic", "20260101T120000Z", "20260102T120000Z")
                .unwrap()
                .contains("BEGIN:VALARM\r\n")
        );
        assert_eq!(utc_stamp(1_767_225_600_000).unwrap(), "20260101T000000Z");
        let mut folded = String::new();
        line(&mut folded, &"é".repeat(100));
        assert!(folded.split("\r\n").all(|l| l.len() <= 75));
    }

    #[test]
    fn only_action_items_still_in_the_notes_are_reviewed() {
        let item = |kind, text: &str| Item {
            kind,
            text: text.into(),
            section: 1,
            sources: vec![],
        };
        let mut session = crate::history::Session::new(crate::history::Kind::Call);
        session.generated_summary = Some(crate::brain::Draft {
            schema: 1,
            source_id: session.id.clone(),
            source_hash: String::new(),
            model: String::new(),
            sections: 1,
            elapsed_ms: 0,
            items: vec![
                item(Kind::Fact, "The launch moved to Friday."),
                item(Kind::Action, "Send the checklist to Casey."),
                item(Kind::Action, "Book the review room."),
                item(Kind::Action, "Send the checklist to Casey."),
            ],
            title: None,
        });
        session.personal_notes =
            "The launch moved to Friday.

Send the checklist to Casey.

Book a room, any room."
                .into();
        let reviewed = reviewed(&session);
        assert_eq!(reviewed.len(), 1);
        assert_eq!(reviewed[0].text, "Send the checklist to Casey.");
        assert_eq!(reviewed_count(&session), 1);
        session.generated_summary = None;
        assert!(reviewed(&session).is_empty());
        assert_eq!(reviewed_count(&session), 0);
    }
}
