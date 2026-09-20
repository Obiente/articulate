use crate::{dictionary::Entry, platform::Target};
use std::sync::mpsc::{self, Receiver, Sender};

pub enum Action {
    Arm(u64, Target, bool),
    Preview {
        id: u64,
        target: Target,
        text: String,
        cancel: transcribe_cpp::CancelToken,
    },
    Insert {
        id: u64,
        target: Target,
        text: String,
        learn: bool,
        cancel: transcribe_cpp::CancelToken,
    },
    Cancel,
    StopLearning,
}
pub enum Event {
    LiveSupport(u64, bool),
    Previewed(u64, Result<(), String>),
    Inserted(u64, Result<bool, String>),
    Learned(Entry, String),
}

pub fn start(wake: impl Fn() + Send + 'static) -> (Sender<Action>, Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    let (events, output) = mpsc::channel();
    std::thread::spawn(move || {
        let emit = |event| {
            let _ = events.send(event);
            wake();
        };
        #[cfg(windows)]
        win::run(rx, emit);
        #[cfg(not(windows))]
        while let Ok(action) = rx.recv() {
            if let Action::Insert { id, .. } = action {
                emit(Event::Inserted(
                    id,
                    Err("App insertion currently requires Windows".into()),
                ));
            }
        }
    });
    (tx, output)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Interactive: focus one disposable field, then move to another after the armed message"]
    fn rejects_insertion_after_focus_moves() {
        assert_eq!(std::env::var("TRANSCRIBE_UI_TEST").as_deref(), Ok("1"));
        let (tx, rx) = start(|| {});
        println!("Focus the first disposable field within five seconds.");
        std::thread::sleep(std::time::Duration::from_secs(5));
        let target = crate::platform::target().expect("A different app must have focus");
        tx.send(Action::Arm(1, target, false)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        println!("Armed. Move focus to a different field within fifteen seconds.");
        std::thread::sleep(std::time::Duration::from_secs(15));
        tx.send(Action::Insert {
            id: 1,
            target,
            text: "This must not be inserted.".into(),
            learn: false,
            cancel: transcribe_cpp::CancelToken::new(),
        })
        .unwrap();
        assert!(matches!(
            rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap(),
            Event::Inserted(1, Err(_))
        ));
    }
    #[test]
    #[ignore = "Interactive: focus a disposable empty text field, then correct Jon to John after insertion"]
    fn external_field_insertion_and_learning() {
        assert_eq!(std::env::var("TRANSCRIBE_UI_TEST").as_deref(), Ok("1"));
        let (tx, rx) = start(|| {});
        println!("Focus the disposable text field within five seconds.");
        std::thread::sleep(std::time::Duration::from_secs(5));
        let target = crate::platform::target().expect("A different app must have focus");
        tx.send(Action::Arm(1, target, false)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        tx.send(Action::Insert {
            id: 1,
            target,
            text: "Send this to Jon tomorrow.".into(),
            learn: true,
            cancel: transcribe_cpp::CancelToken::new(),
        })
        .unwrap();
        match rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap() {
            Event::Inserted(1, Ok(true)) => {
                println!("Insertion succeeded. Correct Jon to John in the same field.")
            }
            Event::Inserted(_, result) => panic!("Unexpected insertion: {result:?}"),
            _ => panic!("Expected insertion result"),
        }
        match rx.recv_timeout(std::time::Duration::from_secs(60)).unwrap() {
            Event::Learned(entry, before) => {
                assert_eq!(entry.heard, "Jon");
                assert_eq!(entry.wanted, "John");
                assert_eq!(before, "Send this to Jon tomorrow.");
                assert!(entry.app.is_some());
            }
            _ => panic!("Expected a learned correction"),
        }
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use crate::{learning, platform};
    use anyhow::{Context, Result, ensure};
    use std::time::{Duration, Instant};
    use windows::{
        Win32::{
            System::{Com::*, Variant::*},
            UI::Accessibility::*,
        },
        core::Interface,
    };

    struct Com;
    impl Drop for Com {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }
    struct Armed {
        id: u64,
        target: Target,
        field: IUIAutomationElement,
        live: Option<crate::live::win::Session>,
    }
    struct Watch {
        target: Target,
        field: IUIAutomationElement,
        span: learning::Span,
        pending: String,
        changed: Instant,
        expires: Instant,
    }
    fn automation() -> Result<IUIAutomation> {
        unsafe {
            let ui: IUIAutomation2 = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
            ui.SetConnectionTimeout(500)?;
            ui.SetTransactionTimeout(500)?;
            Ok(ui.cast()?)
        }
    }
    #[test]
    #[ignore = "Read-only diagnostic of the currently focused external editor"]
    fn focused_editor_capabilities() {
        assert_eq!(
            std::env::var("ARTICULATE_EDITOR_DIAGNOSTIC").as_deref(),
            Ok("1")
        );
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).unwrap();
        }
        let _com = Com;
        let ui = automation().unwrap();
        let target = platform::target().expect("Focus the external editor first");
        let field = unsafe { ui.GetFocusedElement() }.unwrap();
        println!(
            "same={}; editable={:?}; live={:?}",
            same(&ui, target, &field),
            editable(&field).map_err(|e| e.to_string()),
            crate::live::win::Session::new(&field)
                .map(|_| ())
                .map_err(|e| e.to_string())
        );
        unsafe {
            println!(
                "control={:?}; focused={:?}; value_pattern={}; text_pattern={}",
                field.CurrentControlType(),
                field.CurrentHasKeyboardFocus(),
                field
                    .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                    .is_ok(),
                field
                    .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
                    .is_ok()
            );
        }
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Focus {
        Same,
        Changed,
        Unavailable,
    }
    fn focus(ui: &IUIAutomation, target: Target, field: &IUIAutomationElement) -> Focus {
        if platform::target() != Some(target) {
            return Focus::Changed;
        }
        unsafe {
            match ui
                .GetFocusedElement()
                .and_then(|now| ui.CompareElements(field, &now))
            {
                Ok(equal) if !equal.as_bool() => Focus::Changed,
                Ok(_)
                    if field
                        .CurrentHasKeyboardFocus()
                        .is_ok_and(|value| value.as_bool()) =>
                {
                    Focus::Same
                }
                _ => Focus::Unavailable,
            }
        }
    }
    fn same(ui: &IUIAutomation, target: Target, field: &IUIAutomationElement) -> bool {
        // Chromium accessibility providers can briefly stop answering during a
        // render. Retry reads only; a real focus change still rejects immediately.
        for attempt in 0..3 {
            match focus(ui, target, field) {
                Focus::Same => return true,
                Focus::Changed => return false,
                Focus::Unavailable if attempt < 2 => std::thread::sleep(Duration::from_millis(20)),
                Focus::Unavailable => return false,
            }
        }
        false
    }
    fn arm(ui: &IUIAutomation, id: u64, target: Target, use_live: bool) -> Result<Armed> {
        let field = unsafe { ui.GetFocusedElement() }
            .context("Windows could not read the focused field. Focus the editor and try again.")?;
        ensure!(
            same(ui, target, &field),
            "Focus changed before dictation started."
        );
        editable(&field)?;
        let live = use_live
            .then(|| crate::live::win::Session::new(&field).ok())
            .flatten();
        Ok(Armed {
            id,
            target,
            field,
            live,
        })
    }
    fn editable(field: &IUIAutomationElement) -> Result<()> {
        unsafe {
            ensure!(
                !field.CurrentIsPassword()?.as_bool(),
                "Password fields are excluded. Your transcript is ready to copy."
            );
            ensure!(
                field.CurrentIsEnabled()?.as_bool(),
                "The text field is disabled"
            );
            if let Ok(value) =
                field.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            {
                ensure!(
                    !value.CurrentIsReadOnly()?.as_bool(),
                    "The selected field is read-only"
                );
                return Ok(());
            }
            let pattern: IUIAutomationTextPattern = field
                .GetCurrentPatternAs(UIA_TextPatternId)
                .context("Choose an editable text field. Your transcript is ready to copy.")?;
            let mut attr = pattern
                .DocumentRange()?
                .GetAttributeValue(UIA_IsReadOnlyAttributeId)?;
            let writable = attr.Anonymous.Anonymous.vt == VT_BOOL
                && VariantToBoolean(&attr).is_ok_and(|v| !v.as_bool());
            let _ = VariantClear(&mut attr);
            ensure!(
                writable,
                "This app does not expose an editable field. Your transcript is ready to copy."
            );
        }
        Ok(())
    }
    fn contents(field: &IUIAutomationElement) -> Option<String> {
        unsafe {
            if field.CurrentIsPassword().ok()?.as_bool() {
                return None;
            }
            let value = if let Ok(text) =
                field.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            {
                text.DocumentRange().ok()?.GetText(16001).ok()?.to_string()
            } else {
                field
                    .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                    .ok()?
                    .CurrentValue()
                    .ok()?
                    .to_string()
            };
            (value.len() <= 16000).then_some(value)
        }
    }
    fn insert(
        ui: &IUIAutomation,
        armed: Option<Armed>,
        id: u64,
        target: Target,
        text: &str,
        learn: bool,
        cancel: &transcribe_cpp::CancelToken,
    ) -> Result<Option<Watch>> {
        let mut armed = armed.context(
            "The text field changed or could not be identified. Your transcript is ready to copy.",
        )?;
        ensure!(
            armed.id == id && armed.target == target && same(ui, target, &armed.field),
            "Focus changed. Your transcript is ready to copy."
        );
        editable(&armed.field)?;
        if let Some(live) = armed.live.as_mut() {
            live.update(target, text, || {
                !cancel.is_cancelled() && same(ui, target, &armed.field)
            })?;
            return Ok(if learn {
                learning::Span::inserted(&live.before, live.current(), text).map(|span| Watch {
                    target,
                    field: armed.field,
                    span,
                    pending: text.into(),
                    changed: Instant::now(),
                    expires: Instant::now() + Duration::from_secs(120),
                })
            } else {
                None
            });
        }
        let before = if learn { contents(&armed.field) } else { None };
        platform::insert(target, text, || {
            !cancel.is_cancelled() && same(ui, target, &armed.field)
        })?;
        let Some(before) = before else {
            return Ok(None);
        };
        // Providers may publish the value slightly after SendInput returns.
        for _ in 0..6 {
            std::thread::sleep(Duration::from_millis(80));
            if !same(ui, target, &armed.field) {
                return Ok(None);
            }
            if let Some(after) = contents(&armed.field)
                && let Some(span) = learning::Span::inserted(&before, &after, text)
            {
                return Ok(Some(Watch {
                    target,
                    field: armed.field,
                    span,
                    pending: text.into(),
                    changed: Instant::now(),
                    expires: Instant::now() + Duration::from_secs(120),
                }));
            }
        }
        Ok(None)
    }
    pub fn run(rx: Receiver<Action>, emit: impl Fn(Event)) {
        // COM objects stay on this MTA worker. A slow provider never blocks the UI/audio thread.
        let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() };
        let _com = initialized.then_some(Com);
        let ui = initialized.then(automation).and_then(Result::ok);
        let mut armed: Option<Armed> = None;
        let mut insertion_failure: Option<String> = None;
        let mut watch: Option<Watch> = None;
        let mut polled = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Action::Cancel) => {
                    armed = None;
                    insertion_failure = None;
                    watch = None;
                }
                Ok(Action::StopLearning) => watch = None,
                Ok(Action::Arm(id, target, use_live)) => {
                    watch = None;
                    let result = ui
                        .as_ref()
                        .context("Windows text-field access is unavailable")
                        .and_then(|ui| arm(ui, id, target, use_live));
                    insertion_failure = result.as_ref().err().map(|error| format!("{error:#}"));
                    armed = result.ok();
                    if use_live {
                        emit(Event::LiveSupport(
                            id,
                            armed.as_ref().is_some_and(|a| a.live.is_some()),
                        ));
                    }
                }
                Ok(Action::Preview {
                    id,
                    target,
                    text,
                    cancel,
                }) => {
                    let result = (|| -> Result<()> {
                        let ui = ui
                            .as_ref()
                            .context("Windows text-field access is unavailable")?;
                        let a = armed
                            .as_mut()
                            .context("Focus changed. Live typing paused.")?;
                        ensure!(
                            a.id == id && a.target == target,
                            "The dictation target changed"
                        );
                        editable(&a.field)?;
                        // A preview can already be queued when the app receives
                        // the unsupported-live notice. Preserve final insertion.
                        let Some(live) = a.live.as_mut() else {
                            return Ok(());
                        };
                        live.update(target, &text, || {
                            !cancel.is_cancelled() && same(ui, target, &a.field)
                        })
                    })()
                    .map_err(|e| format!("{e:#}"));
                    if let Err(error) = &result {
                        insertion_failure = Some(error.clone());
                        armed = None;
                    }
                    emit(Event::Previewed(id, result));
                }
                Ok(Action::Insert {
                    id,
                    target,
                    text,
                    learn,
                    cancel,
                }) => {
                    watch = None;
                    let result = ui
                        .as_ref()
                        .context("Windows text-field access is unavailable")
                        .and_then(|ui| {
                            if armed.is_none()
                                && let Some(error) = insertion_failure.take()
                            {
                                anyhow::bail!("{error}");
                            }
                            Ok(ui)
                        })
                        .and_then(|ui| insert(ui, armed.take(), id, target, &text, learn, &cancel));
                    let result = result
                        .map(|next| {
                            let learning = next.is_some();
                            watch = next;
                            learning
                        })
                        .map_err(|e| format!("{e:#}"));
                    emit(Event::Inserted(id, result));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if polled.elapsed() < Duration::from_millis(300) {
                continue;
            }
            polled = Instant::now();
            let Some(ui) = ui.as_ref() else {
                continue;
            };
            if armed
                .as_ref()
                .is_some_and(|a| focus(ui, a.target, &a.field) == Focus::Changed)
            {
                armed = None;
                insertion_failure = Some("Focus changed. Your transcript is ready to copy.".into());
            }
            let Some(w) = watch.as_mut() else {
                continue;
            };
            if Instant::now() >= w.expires || !same(ui, w.target, &w.field) {
                watch = None;
                continue;
            }
            let current = contents(&w.field).and_then(|s| w.span.current(&s).map(str::to_owned));
            let Some(current) = current else {
                watch = None;
                continue;
            };
            if current != w.pending {
                w.pending = current;
                w.changed = Instant::now();
            }
            if w.pending != w.span.original && w.changed.elapsed() >= Duration::from_secs(2) {
                if let Some(mut entry) = learning::correction(&w.span.original, &w.pending) {
                    let Some(app) = platform::app_name(w.target) else {
                        // A failed process lookup must not turn an app-specific
                        // correction into a global replacement.
                        watch = None;
                        continue;
                    };
                    entry.app = Some(app);
                    let before = w.span.original.clone();
                    w.span.original.clone_from(&w.pending);
                    emit(Event::Learned(entry, before));
                } else {
                    // Composition or a larger rewrite ends this observation.
                    watch = None;
                }
            }
        }
    }

    #[cfg(test)]
    mod live_tests {
        use super::*;

        #[test]
        #[ignore = "Interactive: focus an empty disposable TextPattern field; this test replaces its dictated text repeatedly"]
        fn native_live_revisions_and_final_insertion_keep_up_with_own_edits() {
            live_revisions(
                &[
                    "Send the notes to Jon.",
                    "Send the notes to John.",
                    "Send the notes to John. café 😀",
                    "Send the notes to John. café 😃 Ready.",
                    "Send the notes to John. café 😃",
                ],
                "Send the notes to John. café 😃 Finished.",
            );
        }

        #[test]
        #[ignore = "Interactive Discord test: revises only synthetic plain text in an empty composer; never sends a message"]
        fn discord_live_plain_text_revisions_and_empty_editor_recovery() {
            assert_eq!(
                std::env::var("ARTICULATE_EDITOR_TEST_APP").as_deref(),
                Ok("discord.exe")
            );
            live_revisions(
                &[
                    "Send the notes to Jon.",
                    "Send the notes to John.",
                    "Send the notes to John. café résumé",
                    "Send the notes to John. café résumé Ready.",
                    "Send the notes to John. café résumé",
                    "",
                    "Send the notes to John. café résumé",
                ],
                "Send the notes to John. café résumé Finished.",
            );
        }

        fn live_revisions(revisions: &[&str], final_text: &str) {
            assert_eq!(std::env::var("TRANSCRIBE_UI_TEST").as_deref(), Ok("1"));
            println!(
                "Focus an empty disposable text field within five seconds. Leave the caret untouched until the result."
            );
            std::thread::sleep(Duration::from_secs(5));
            unsafe {
                CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
            }
            let _com = Com;
            let ui = automation().unwrap();
            let target = platform::target().expect("A disposable app must have focus");
            if let Ok(expected_app) = std::env::var("ARTICULATE_EDITOR_TEST_APP") {
                assert_eq!(
                    platform::app_name(target).as_deref(),
                    Some(expected_app.as_str()),
                    "The authorized test app must remain focused"
                );
            }
            let field = unsafe { ui.GetFocusedElement().unwrap() };
            // A failed prior run can leave one of these exact synthetic values.
            // Resume without ever clearing an arbitrary draft or sending Enter.
            let old_value = contents(&field).unwrap();
            if [
                "Send the notes to Jon.",
                "Send the notes to John. café \n\u{fffc}\n:grinning:\n\u{feff}\n\u{feff}",
                "Send the notes to John. café résumé Finished.",
            ]
            .contains(&old_value.as_str())
            {
                if old_value.contains('\u{fffc}') {
                    println!("Synthetic rich value: {:?}", unsafe {
                        field
                            .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                            .and_then(|value| value.CurrentValue())
                    });
                }
                let pattern: IUIAutomationTextPattern =
                    unsafe { field.GetCurrentPatternAs(UIA_TextPatternId).unwrap() };
                unsafe {
                    pattern.DocumentRange().unwrap().Select().unwrap();
                }
                for _ in 0..20 {
                    if unsafe {
                        pattern
                            .GetSelection()
                            .and_then(|ranges| ranges.GetElement(0))
                            .and_then(|range| range.GetText(100))
                            .is_ok_and(|value| value == old_value.as_str())
                    } {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                platform::replace_selection(target, "", || {
                    same(&ui, target, &field)
                        && contents(&field).as_deref() == Some(old_value.as_str())
                        && unsafe {
                            pattern
                                .GetSelection()
                                .and_then(|ranges| ranges.GetElement(0))
                                .and_then(|range| range.GetText(100))
                                .is_ok_and(|value| value == old_value.as_str())
                        }
                })
                .unwrap();
                std::thread::sleep(Duration::from_millis(100));
            }
            assert!(
                matches!(contents(&field).as_deref(), Some("" | "\u{feff}\n")),
                "The disposable text field must be empty"
            );
            println!(
                "Empty editor: {:?}",
                crate::live::win::Session::diagnostic_snapshot(&field)
            );
            let (tx, rx) = crate::integration::start(|| {});
            tx.send(Action::Arm(41, target, true)).unwrap();
            assert!(
                matches!(
                    rx.recv_timeout(Duration::from_secs(10)).unwrap(),
                    Event::LiveSupport(41, true)
                ),
                "The chosen field must expose a live TextPattern"
            );
            for &text in revisions {
                tx.send(Action::Preview {
                    id: 41,
                    target,
                    text: text.into(),
                    cancel: transcribe_cpp::CancelToken::new(),
                })
                .unwrap();
                match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
                    Event::Previewed(41, Ok(())) => {}
                    Event::Previewed(_, result) => panic!(
                        "Live revision paused: {result:?}; snapshot={:?}",
                        crate::live::win::Session::diagnostic_snapshot(&field)
                    ),
                    _ => panic!("Expected a live revision acknowledgement"),
                }
                if text.is_empty() {
                    assert!(matches!(
                        contents(&field).as_deref(),
                        Some("" | "\u{feff}\n")
                    ));
                } else {
                    assert_eq!(
                        contents(&field).as_deref(),
                        Some(text),
                        "Only the dictated span should be revised"
                    );
                }
            }
            tx.send(Action::Insert {
                id: 41,
                target,
                text: final_text.into(),
                learn: false,
                cancel: transcribe_cpp::CancelToken::new(),
            })
            .unwrap();
            match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
                Event::Inserted(41, Ok(false)) => {}
                Event::Inserted(_, result) => panic!("Final insertion failed: {result:?}"),
                _ => panic!("Expected final insertion acknowledgement"),
            }
            assert_eq!(contents(&field).as_deref(), Some(final_text));
            println!(
                "All live revisions, Unicode changes, deletion and final insertion succeeded without pausing."
            );
        }
    }
}
