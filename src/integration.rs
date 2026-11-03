use crate::{dictionary::Entry, platform::Target};
use std::sync::mpsc::{self, Receiver, Sender};

pub enum Action {
    LineBreak(crate::platform::LineBreak),
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
    EditPrevious {
        id: u64,
        target: Target,
        undo: bool,
        cancel: transcribe_cpp::CancelToken,
    },
    Cancel,
    StopLearning,
}
pub enum Event {
    LiveSupport(u64, bool),
    Previewed(u64, Result<(), String>),
    Inserted(u64, Result<bool, String>),
    Edited(u64, Result<(), String>),
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

    /// The focused element, read through UI Automation like the worker does.
    fn focused_field() -> Option<(
        windows::Win32::UI::Accessibility::IUIAutomation,
        windows::Win32::UI::Accessibility::IUIAutomationElement,
    )> {
        use windows::Win32::{System::Com::*, UI::Accessibility::*};
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let ui: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
            let field = ui.GetFocusedElement().ok()?;
            Some((ui, field))
        }
    }

    fn field_text(field: &windows::Win32::UI::Accessibility::IUIAutomationElement) -> String {
        use windows::Win32::UI::Accessibility::*;
        unsafe {
            if let Ok(text) =
                field.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
                && let Ok(value) = text.DocumentRange().and_then(|r| r.GetText(-1))
            {
                return value.to_string();
            }
            field
                .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                .and_then(|value| value.CurrentValue())
                .map(|value| value.to_string())
                .unwrap_or_else(|error| format!("<unreadable: {error}>"))
        }
    }

    fn describe(field: &windows::Win32::UI::Accessibility::IUIAutomationElement) -> String {
        unsafe {
            format!(
                "control={:?} name={:?} class={:?}",
                field.CurrentControlType().map(|c| c.0).unwrap_or_default(),
                field
                    .CurrentName()
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
                field
                    .CurrentClassName()
                    .map(|n| n.to_string())
                    .unwrap_or_default()
            )
        }
    }

    fn send_keys(keys: &[(u16, bool)]) {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
        let input: Vec<INPUT> = keys
            .iter()
            .map(|&(key, up)| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: key,
                        wScan: 0,
                        dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            })
            .collect();
        unsafe {
            SendInput(
                input.len() as u32,
                input.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            );
        }
    }

    /// End to end against a real window in another process: the dictation
    /// shortcut is tapped as a toggle press, then final and live insertion run
    /// and the field is read back. Every stage is reported before asserting.
    #[test]
    #[ignore = "Interactive: drives the disposable window in ARTICULATE_E2E_HWND and types into it"]
    fn shortcut_then_insertion_into_a_disposable_window() {
        use std::time::Duration;
        use windows_sys::Win32::{
            System::Threading::{AttachThreadInput, GetCurrentThreadId},
            UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
        };
        let handle: isize = std::env::var("ARTICULATE_E2E_HWND")
            .expect("Set ARTICULATE_E2E_HWND to a disposable window")
            .parse()
            .unwrap();
        let window = handle as windows_sys::Win32::Foundation::HWND;
        unsafe {
            let foreground = GetForegroundWindow();
            let current = GetCurrentThreadId();
            let other = GetWindowThreadProcessId(foreground, std::ptr::null_mut());
            AttachThreadInput(current, other, 1);
            ShowWindow(window, SW_RESTORE);
            BringWindowToTop(window);
            SetForegroundWindow(window);
            AttachThreadInput(current, other, 0);
        }
        std::thread::sleep(Duration::from_millis(800));
        // Put focus in the window's first text field, as a click would.
        if let Some((ui, _)) = focused_field() {
            use windows::Win32::UI::Accessibility::*;
            unsafe {
                let edit = ui
                    .ElementFromHandle(windows::Win32::Foundation::HWND(handle as _))
                    .and_then(|root| {
                        let all =
                            root.FindAll(TreeScope_Descendants, &ui.CreateTrueCondition()?)?;
                        for index in 0..all.Length()? {
                            let element = all.GetElement(index)?;
                            // Skip the browser address bar; the page field comes after it.
                            if element.CurrentControlType()? == UIA_EditControlTypeId
                                && element.CurrentClassName()? != "OmniboxViewViews"
                            {
                                return Ok(Some(element));
                            }
                        }
                        Ok(None)
                    });
                match edit {
                    Ok(Some(edit)) => {
                        let _ = edit.SetFocus();
                    }
                    Ok(None) => println!("No text field found"),
                    Err(error) => println!("Could not search for a text field: {error}"),
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let mut report = Vec::new();
        let before = crate::platform::target().expect("The disposable window must have focus");
        let (ui, field) = focused_field().expect("No focused element");
        report.push(format!("focused before: {}", describe(&field)));

        // 1. A toggle tap of an Alt chord. F13 avoids a running copy's Alt + Space.
        let (events, hotkeys) = mpsc::channel();
        let shortcut = crate::platform::Hotkey {
            ctrl: false,
            alt: true,
            shift: false,
            win: false,
            key: "F13".into(),
        };
        let updates = crate::platform::hotkey(shortcut, events);
        loop {
            match hotkeys.recv_timeout(Duration::from_secs(5)) {
                Ok(crate::platform::HotkeyEvent::Registered(_)) => break,
                Ok(crate::platform::HotkeyEvent::Error { message, .. }) => {
                    panic!("Shortcut unavailable: {message}")
                }
                Ok(_) => {}
                Err(error) => panic!("No registration: {error}"),
            }
        }
        send_keys(&[(VK_MENU, false), (VK_F13, false)]);
        std::thread::sleep(Duration::from_millis(80));
        send_keys(&[(VK_F13, true)]);
        std::thread::sleep(Duration::from_millis(40));
        send_keys(&[(VK_MENU, true)]);
        let pressed = loop {
            match hotkeys.recv_timeout(Duration::from_secs(3)) {
                Ok(crate::platform::HotkeyEvent::Pressed(target, _)) => break Some(target),
                Ok(_) => {}
                Err(_) => break None,
            }
        };
        report.push(format!(
            "shortcut pressed: {:?}, target matches: {}",
            pressed.is_some(),
            pressed == Some(before)
        ));
        std::thread::sleep(Duration::from_millis(600));
        let after = crate::platform::target();
        let still_focused = unsafe {
            ui.GetFocusedElement()
                .and_then(|now| {
                    report.push(format!("focused after release: {}", describe(&now)));
                    ui.CompareElements(&field, &now)
                })
                .is_ok_and(|same| same.as_bool())
        };
        report.push(format!(
            "after Alt release: same window {}, same field {still_focused}",
            after == Some(before)
        ));
        drop(updates);

        // 2. Final insertion, as hold mode uses.
        let (tx, rx) = start(|| {});
        let wait = |rx: &Receiver<Event>, want: &str| -> String {
            loop {
                match rx.recv_timeout(Duration::from_secs(10)) {
                    Ok(Event::Inserted(_, result)) if want == "inserted" => {
                        break format!("{result:?}");
                    }
                    Ok(Event::LiveSupport(_, live)) if want == "live" => break format!("{live}"),
                    Ok(Event::Previewed(_, result)) if want == "preview" => {
                        break format!("{result:?}");
                    }
                    Ok(_) => {}
                    Err(error) => break format!("<no event: {error}>"),
                }
            }
        };
        // Empty fields report only a marker: U+FFFC for a text area, a line
        // break for a rich-text paragraph. Neither is part of the text.
        let visible = |text: String| text.replace(['\u{fffc}', '\n'], "");
        let start_text = visible(field_text(&field));
        tx.send(Action::Arm(1, before, false)).unwrap();
        tx.send(Action::Insert {
            id: 1,
            target: before,
            text: "alpha beta".into(),
            learn: false,
            cancel: transcribe_cpp::CancelToken::new(),
        })
        .unwrap();
        let final_result = wait(&rx, "inserted");
        report.push(format!("final insert: {final_result}"));
        std::thread::sleep(Duration::from_millis(300));
        let after_final = field_text(&field);
        report.push(format!("field after final: {after_final:?}"));

        // 3. Live typing, as toggle mode uses.
        tx.send(Action::Arm(2, before, true)).unwrap();
        report.push(format!("live supported: {}", wait(&rx, "live")));
        for (step, text) in [" gamma", " gamma delta"].iter().enumerate() {
            tx.send(Action::Preview {
                id: 2,
                target: before,
                text: (*text).into(),
                cancel: transcribe_cpp::CancelToken::new(),
            })
            .unwrap();
            report.push(format!("preview {step}: {}", wait(&rx, "preview")));
        }
        tx.send(Action::Insert {
            id: 2,
            target: before,
            text: " gamma delta epsilon".into(),
            learn: false,
            cancel: transcribe_cpp::CancelToken::new(),
        })
        .unwrap();
        let live_result = wait(&rx, "inserted");
        report.push(format!("live final insert: {live_result}"));
        std::thread::sleep(Duration::from_millis(300));
        let after_live = field_text(&field);
        report.push(format!("field after live: {after_live:?}"));
        println!("E2E REPORT\n{}", report.join("\n"));
        assert!(
            pressed == Some(before),
            "shortcut did not fire for the window"
        );
        assert!(still_focused, "Alt release moved focus out of the field");
        assert!(
            final_result.starts_with("Ok"),
            "final insertion reported {final_result}"
        );
        assert!(
            live_result.starts_with("Ok"),
            "live insertion reported {live_result}"
        );
        assert_eq!(visible(after_final), format!("{start_text}alpha beta"));
        assert_eq!(
            visible(after_live),
            format!("{start_text}alpha beta gamma delta epsilon")
        );
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
        line_break: platform::LineBreak,
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
    fn arm(ui: &IUIAutomation, id: u64, target: Target, _use_live: bool) -> Result<Armed> {
        let field = unsafe { ui.GetFocusedElement() }
            .context("Windows could not read the focused field. Focus the editor and try again.")?;
        ensure!(
            same(ui, target, &field),
            "Focus changed before dictation started."
        );
        editable(&field)?;
        let live = crate::live::win::Session::new(&field).ok();
        Ok(Armed {
            id,
            target,
            field,
            live,
            line_break: platform::LineBreak::Enter,
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
        armed: &mut Armed,
        id: u64,
        target: Target,
        text: &str,
        learn: bool,
        cancel: &transcribe_cpp::CancelToken,
    ) -> Result<Option<Watch>> {
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
                    field: armed.field.clone(),
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
        platform::insert_with_line_break(target, text, armed.line_break, || {
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
                    field: armed.field.clone(),
                    span,
                    pending: text.into(),
                    changed: Instant::now(),
                    expires: Instant::now() + Duration::from_secs(120),
                }));
            }
        }
        Ok(None)
    }
    fn receive_action(
        rx: &Receiver<Action>,
        observing: bool,
        since_poll: Duration,
    ) -> Result<Action, mpsc::RecvTimeoutError> {
        if observing {
            rx.recv_timeout(Duration::from_millis(300).saturating_sub(since_poll))
        } else {
            rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        }
    }

    pub fn run(rx: Receiver<Action>, emit: impl Fn(Event)) {
        // COM objects stay on this MTA worker. A slow provider never blocks the UI/audio thread.
        let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() };
        let _com = initialized.then_some(Com);
        let ui = initialized.then(automation).and_then(Result::ok);
        let mut armed: Option<Armed> = None;
        let mut previous: Option<(Armed, crate::macros::InsertionHistory)> = None;
        let mut line_break = platform::LineBreak::Enter;
        let mut insertion_failure: Option<String> = None;
        let mut watch: Option<Watch> = None;
        let mut polled = Instant::now();
        loop {
            match receive_action(&rx, armed.is_some() || watch.is_some(), polled.elapsed()) {
                Ok(Action::Cancel) => {
                    armed = None;
                    insertion_failure = None;
                    watch = None;
                }
                Ok(Action::EditPrevious {
                    id,
                    target,
                    undo,
                    cancel,
                }) => {
                    watch = None;
                    armed = None;
                    let result = (|| -> Result<()> {
                        let ui = ui
                            .as_ref()
                            .context("Windows text-field access is unavailable")?;
                        let (a, history) = previous
                            .as_mut()
                            .context("Nothing to edit in the previous field.")?;
                        ensure!(
                            a.target == target && same(ui, target, &a.field),
                            "The target field changed. Previous dictation was left unchanged."
                        );
                        editable(&a.field)?;
                        let live = a
                            .live
                            .as_mut()
                            .context("This field does not support verified spoken edits.")?;
                        let replacement = history.replacement(undo)?;
                        live.update(target, &replacement, || {
                            !cancel.is_cancelled() && same(ui, target, &a.field)
                        })?;
                        history.commit(replacement);
                        Ok(())
                    })()
                    .map_err(|e| format!("{e:#}"));
                    emit(Event::Edited(id, result));
                }
                Ok(Action::StopLearning) => watch = None,
                Ok(Action::LineBreak(value)) => line_break = value,
                Ok(Action::Arm(id, target, use_live)) => {
                    watch = None;
                    let result = ui
                        .as_ref()
                        .context("Windows text-field access is unavailable")
                        .and_then(|ui| arm(ui, id, target, use_live))
                        .map(|mut armed| {
                            armed.line_break = line_break;
                            if let Some(live) = &mut armed.live {
                                live.line_break = line_break;
                            }
                            armed
                        });
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
                        .and_then(|ui| {
                            let a = armed
                                .as_mut()
                                .context("The text field changed or could not be identified.")?;
                            insert(ui, a, id, target, &text, learn, &cancel)
                        });
                    if result.is_ok() {
                        previous = armed.take().map(|a| {
                            let before = a
                                .live
                                .as_ref()
                                .map(|live| live.original_selection().to_owned());
                            (
                                a,
                                crate::macros::InsertionHistory::new(
                                    text.clone(),
                                    before.unwrap_or_default(),
                                ),
                            )
                        });
                    } else {
                        armed = None;
                        previous = None;
                    }
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
        fn idle_commands_disconnect_and_observation_deadlines_wake_the_worker() {
            let (tx, rx) = mpsc::channel();
            let waiting = std::thread::spawn(move || receive_action(&rx, false, Duration::ZERO));
            tx.send(Action::Cancel).unwrap();
            assert!(matches!(waiting.join().unwrap(), Ok(Action::Cancel)));

            let (tx, rx) = mpsc::channel::<Action>();
            assert!(matches!(
                receive_action(&rx, true, Duration::from_millis(300)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            drop(tx);
            assert!(matches!(
                receive_action(&rx, false, Duration::ZERO),
                Err(mpsc::RecvTimeoutError::Disconnected)
            ));
        }

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
