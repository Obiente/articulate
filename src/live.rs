// Only the changed suffix is replaced. Boundaries are UTF-8-safe; Windows
// selection ranges handle character positions instead of guessed backspaces.
pub fn tail<'a>(before: &'a str, after: &'a str) -> (&'a str, &'a str) {
    let prefix = before
        .chars()
        .zip(after.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    (&before[prefix..], &after[prefix..])
}

#[cfg(any(windows, test))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct FieldText {
    document: String,
    left: String,
    selected: String,
    right: String,
}

#[cfg(any(windows, test))]
impl FieldText {
    fn consistent(&self) -> bool {
        self.document == format!("{}{}{}", self.left, self.selected, self.right)
    }

    fn matches(&self, document: &str, left: &str, selected: &str, right: &str) -> bool {
        self.document == document
            && self.left == left
            && self.selected == selected
            && self.right == right
            && self.consistent()
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
enum Acknowledgement {
    Pending,
    Confirmed,
    Changed,
}

// UIA can expose the document and selection from different render frames. Only
// our exact replacement, its known incremental prefixes, and the old document
// are valid while waiting. No further input is sent until both document and
// caret have acknowledged the complete replacement twice consecutively.
#[cfg(any(windows, test))]
struct OwnUpdate<'a> {
    before: &'a str,
    left: &'a str,
    inserted: &'a str,
    right: &'a str,
    confirmations: u8,
}

#[cfg(any(windows, test))]
impl OwnUpdate<'_> {
    fn observe(&mut self, field: &FieldText) -> Acknowledgement {
        let wanted_left = format!("{}{}", self.left, self.inserted);
        let wanted = format!("{}{}", wanted_left, self.right);
        if field.matches(&wanted, &wanted_left, "", self.right) {
            self.confirmations += 1;
            return if self.confirmations >= 2 {
                Acknowledgement::Confirmed
            } else {
                Acknowledgement::Pending
            };
        }
        self.confirmations = 0;
        if !field.consistent() || field.document == self.before {
            return Acknowledgement::Pending;
        }
        let known_progress = field
            .document
            .strip_prefix(self.left)
            .and_then(|value| value.strip_suffix(self.right))
            .is_some_and(|value| self.inserted.starts_with(value));
        if known_progress {
            Acknowledgement::Pending
        } else {
            Acknowledgement::Changed
        }
    }
}

#[cfg(windows)]
pub mod win {
    use crate::platform::{self, Target};
    use anyhow::{Result, ensure};
    use windows::Win32::UI::Accessibility::*;

    pub struct Session {
        pattern: IUIAutomationTextPattern,
        selected: String,
        pub before: String,
        current: String,
        prefix: String,
        suffix: String,
        previous: String,
        written: bool,
    }
    fn selection(pattern: &IUIAutomationTextPattern) -> Result<IUIAutomationTextRange> {
        unsafe {
            let selected = pattern.GetSelection()?;
            ensure!(
                selected.Length()? == 1,
                "Choose a single text insertion point"
            );
            Ok(selected.GetElement(0)?)
        }
    }
    fn text(range: &IUIAutomationTextRange) -> Result<String> {
        let text = unsafe { range.GetText(16001)? }.to_string();
        ensure!(
            text.len() <= 16000,
            "Live typing is unavailable in this large text field"
        );
        Ok(text)
    }
    fn snapshot(
        pattern: &IUIAutomationTextPattern,
    ) -> Result<(super::FieldText, IUIAutomationTextRange)> {
        unsafe {
            let caret = selection(pattern)?;
            let document = pattern.DocumentRange()?;
            let left = document.Clone()?;
            left.MoveEndpointByRange(
                TextPatternRangeEndpoint_End,
                &caret,
                TextPatternRangeEndpoint_Start,
            )?;
            let right = document.Clone()?;
            right.MoveEndpointByRange(
                TextPatternRangeEndpoint_Start,
                &caret,
                TextPatternRangeEndpoint_End,
            )?;
            Ok((
                super::FieldText {
                    document: text(&document)?,
                    left: text(&left)?,
                    selected: text(&caret)?,
                    right: text(&right)?,
                },
                caret,
            ))
        }
    }
    impl Session {
        pub fn new(field: &IUIAutomationElement) -> Result<Self> {
            unsafe {
                let pattern: IUIAutomationTextPattern =
                    field.GetCurrentPatternAs(UIA_TextPatternId)?;
                let (field, _) = snapshot(&pattern)?;
                ensure!(
                    field.consistent(),
                    "This app does not expose a stable text selection"
                );
                Ok(Self {
                    pattern,
                    selected: field.selected,
                    current: field.document.clone(),
                    before: field.document,
                    prefix: field.left,
                    suffix: field.right,
                    previous: String::new(),
                    written: false,
                })
            }
        }
        fn unchanged(&self) -> bool {
            snapshot(&self.pattern).is_ok_and(|(field, _)| {
                field.matches(
                    &self.current,
                    &format!("{}{}", self.prefix, self.previous),
                    &self.selected,
                    &self.suffix,
                )
            })
        }
        pub fn current(&self) -> &str {
            &self.current
        }
        pub fn update(
            &mut self,
            target: Target,
            next: &str,
            guard: impl Fn() -> bool,
        ) -> Result<()> {
            ensure!(
                guard() && self.unchanged(),
                "Live typing paused because the field or caret changed. Your transcript continues in Articulate."
            );
            if next == self.previous {
                return Ok(());
            }
            let expected = format!("{}{}{}", self.prefix, next, self.suffix);
            ensure!(
                expected.len() <= 16000,
                "Live typing paused in this large field. Your transcript continues in Articulate."
            );
            unsafe {
                let (old_tail, new_tail) = super::tail(&self.previous, next);
                let (field, caret) = snapshot(&self.pattern)?;
                ensure!(
                    field.matches(
                        &self.current,
                        &format!("{}{}", self.prefix, self.previous),
                        &self.selected,
                        &self.suffix
                    ),
                    "The field or caret changed. Live typing paused."
                );
                let mut range = caret.Clone()?;
                if self.written && !old_tail.is_empty() {
                    // Anchor the replacement at the freshly verified caret.
                    // FindText is deliberately avoided: providers can normalize
                    // its endpoints independently of the actual selected text.
                    // Verify text after each UIA character movement, so UTF-16
                    // and grapheme differences never become guessed backspaces.
                    range.MoveEndpointByUnit(
                        TextPatternRangeEndpoint_Start,
                        TextUnit_Character,
                        -(old_tail.chars().count() as i32),
                    )?;
                    if text(&range)? != old_tail {
                        range = caret.Clone()?;
                        for _ in 0..old_tail.encode_utf16().count() {
                            if range.MoveEndpointByUnit(
                                TextPatternRangeEndpoint_Start,
                                TextUnit_Character,
                                -1,
                            )? == 0
                            {
                                break;
                            }
                            if text(&range)? == old_tail {
                                break;
                            }
                        }
                    }
                    ensure!(
                        text(&range)? == old_tail
                            && range.CompareEndpoints(
                                TextPatternRangeEndpoint_End,
                                &caret,
                                TextPatternRangeEndpoint_Start
                            )? == 0,
                        "This app could not select the previous words. Live typing paused."
                    );
                }
                ensure!(
                    guard() && self.unchanged(),
                    "Focus or text changed. Live typing paused."
                );
                let common = &self.previous[..self.previous.len() - old_tail.len()];
                let replacement_left = format!("{}{}", self.prefix, common);
                let replacement_selected = if self.written {
                    old_tail
                } else {
                    &self.selected
                };
                range.Select()?;
                // Selecting a range also has an asynchronous UIA acknowledgement.
                // Retry reads only; never send input while a selection is stale.
                let mut selected = false;
                for _ in 0..12 {
                    ensure!(guard(), "Focus changed. Live typing paused.");
                    let (field, _) = snapshot(&self.pattern)?;
                    if field.matches(
                        &self.current,
                        &replacement_left,
                        replacement_selected,
                        &self.suffix,
                    ) {
                        selected = true;
                        break;
                    }
                    ensure!(
                        field.document == self.current,
                        "The field changed. Live typing paused."
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                ensure!(
                    selected,
                    "The app did not confirm the selection. Live typing paused."
                );
                platform::replace_selection(target, new_tail, || {
                    guard()
                        && snapshot(&self.pattern).is_ok_and(|(field, _)| {
                            field.matches(
                                &self.current,
                                &replacement_left,
                                replacement_selected,
                                &self.suffix,
                            )
                        })
                })?;
                let mut acknowledgement = super::OwnUpdate {
                    before: &self.current,
                    left: &replacement_left,
                    inserted: new_tail,
                    right: &self.suffix,
                    confirmations: 0,
                };
                for _ in 0..24 {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                    ensure!(guard(), "Focus changed. Live typing paused.");
                    let (field, _) = snapshot(&self.pattern)?;
                    match acknowledgement.observe(&field) {
                        super::Acknowledgement::Confirmed => {
                            self.selected.clear();
                            self.previous = next.into();
                            self.current = expected;
                            self.written = true;
                            return Ok(());
                        }
                        super::Acknowledgement::Changed => {
                            anyhow::bail!("The field changed. Live typing paused.")
                        }
                        super::Acknowledgement::Pending => {}
                    }
                }
            }
            anyhow::bail!(
                "The app did not confirm the update. Live typing paused; check its text before copying."
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replaces_only_changed_tail_without_splitting_unicode() {
        assert_eq!(tail("Hello Jon", "Hello John."), ("n", "hn."));
        assert_eq!(tail("café 😀", "café 😃"), ("😀", "😃"));
        assert_eq!(tail("你好", "你们好"), ("好", "们好"));
        assert_eq!(tail("First. extra", "First."), (" extra", ""));
        assert_eq!(tail("First.", "First. Next."), ("", " Next."));
        assert_eq!(tail("", ""), ("", ""));
    }

    fn field(left: &str, selected: &str, right: &str) -> FieldText {
        FieldText {
            document: format!("{left}{selected}{right}"),
            left: left.into(),
            selected: selected.into(),
            right: right.into(),
        }
    }

    #[test]
    fn own_revision_waits_for_incremental_text_and_two_complete_snapshots() {
        let mut ack = OwnUpdate {
            before: "Send Jon tomorrow",
            left: "Send Jo",
            inserted: "hn",
            right: " tomorrow",
            confirmations: 0,
        };
        assert_eq!(
            ack.observe(&field("Send Jo", "n", " tomorrow")),
            Acknowledgement::Pending
        );
        assert_eq!(
            ack.observe(&field("Send Joh", "", " tomorrow")),
            Acknowledgement::Pending
        );
        assert_eq!(
            ack.observe(&field("Send John", "", " tomorrow")),
            Acknowledgement::Pending
        );
        // A stale provider frame invalidates the first confirmation.
        assert_eq!(
            ack.observe(&field("Send Joh", "", " tomorrow")),
            Acknowledgement::Pending
        );
        assert_eq!(
            ack.observe(&field("Send John", "", " tomorrow")),
            Acknowledgement::Pending
        );
        assert_eq!(
            ack.observe(&field("Send John", "", " tomorrow")),
            Acknowledgement::Confirmed
        );
    }

    #[test]
    fn own_revision_rejects_manual_edits_inside_or_outside_the_dictated_span() {
        for changed in [
            field("Send Jack", "", " tomorrow"),
            field("Send John", "", " next week"),
            field("Email John", "", " tomorrow"),
        ] {
            let mut ack = OwnUpdate {
                before: "Send Jon tomorrow",
                left: "Send Jo",
                inserted: "hn",
                right: " tomorrow",
                confirmations: 0,
            };
            assert_eq!(ack.observe(&changed), Acknowledgement::Changed);
        }
    }

    #[test]
    fn own_revision_never_confirms_a_moved_caret_or_selection() {
        let mut ack = OwnUpdate {
            before: "Jon",
            left: "Jo",
            inserted: "hn",
            right: "",
            confirmations: 0,
        };
        for _ in 0..24 {
            assert_eq!(
                ack.observe(&field("Jo", "", "hn")),
                Acknowledgement::Pending
            );
            assert_eq!(
                ack.observe(&field("Jo", "hn", "")),
                Acknowledgement::Pending
            );
        }
        assert_eq!(
            ack.observe(&field("John", "", "")),
            Acknowledgement::Pending
        );
    }

    #[test]
    fn unicode_replacement_and_deletion_preserve_existing_suffix() {
        let mut ack = OwnUpdate {
            before: "Draft café 😀 end",
            left: "Draft café ",
            inserted: "😃",
            right: " end",
            confirmations: 0,
        };
        assert_eq!(
            ack.observe(&field("Draft café ", "", " end")),
            Acknowledgement::Pending
        );
        assert_eq!(
            ack.observe(&field("Draft café 😃", "", " end")),
            Acknowledgement::Pending
        );
        assert_eq!(
            ack.observe(&field("Draft café 😃", "", " end")),
            Acknowledgement::Confirmed
        );
        let mut ack = OwnUpdate {
            before: "Keep this extra.",
            left: "Keep this",
            inserted: "",
            right: ".",
            confirmations: 0,
        };
        assert_eq!(
            ack.observe(&field("Keep this", "", ".")),
            Acknowledgement::Pending
        );
        assert_eq!(
            ack.observe(&field("Keep this", "", ".")),
            Acknowledgement::Confirmed
        );
    }

    #[test]
    fn mixed_provider_frames_cannot_acknowledge_an_update() {
        let mut ack = OwnUpdate {
            before: "Jon",
            left: "Jo",
            inserted: "hn",
            right: "",
            confirmations: 0,
        };
        let mut mixed = field("Jon", "", "");
        mixed.document = "John".into();
        for _ in 0..24 {
            assert_eq!(ack.observe(&mixed), Acknowledgement::Pending);
        }
        assert_eq!(
            ack.observe(&field("John", "", "")),
            Acknowledgement::Pending
        );
        assert_eq!(
            ack.observe(&field("John", "", "")),
            Acknowledgement::Confirmed
        );
    }

    #[test]
    fn unchanged_requires_the_original_insertion_point_even_with_duplicate_words() {
        let original = field("Jon and ", "Jon", ".");
        assert!(original.matches("Jon and Jon.", "Jon and ", "Jon", "."));
        assert!(!field("", "Jon", " and Jon.").matches("Jon and Jon.", "Jon and ", "Jon", "."));
    }
}
