use anyhow::{Result, bail};
use std::sync::mpsc::Sender;

mod hotkey_lifecycle;

/// How the app interprets a shortcut gesture. Explicit choices are preserved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HotkeyMode {
    Toggle,
    #[default]
    Hold,
}

/// A portable shortcut preference. Key names are independent of Windows codes.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub key: String,
}

pub const HOTKEY_KEYS: &[&str] = &[
    "Space",
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "L",
    "M",
    "N",
    "O",
    "P",
    "Q",
    "R",
    "S",
    "T",
    "U",
    "V",
    "W",
    "X",
    "Y",
    "Z",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "F13",
    "F14",
    "F15",
    "F16",
    "F17",
    "F18",
    "F19",
    "F20",
    "F21",
    "F22",
    "F23",
    "F24",
    "Tab",
    "Enter",
    "Escape",
    "Backspace",
    "Delete",
    "Insert",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
];

impl Default for Hotkey {
    fn default() -> Self {
        Self {
            ctrl: true,
            alt: true,
            shift: false,
            win: false,
            key: "Space".into(),
        }
    }
}

impl Hotkey {
    pub fn validate(&self) -> Result<(), String> {
        if !HOTKEY_KEYS.contains(&self.key.as_str()) {
            return Err("Choose a supported shortcut key.".into());
        }
        if !(self.ctrl || self.alt || self.win) {
            return Err("Include Ctrl, Alt, or Win so ordinary typing stays available.".into());
        }
        Ok(())
    }

    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.win {
            parts.push("Win");
        }
        parts.push(&self.key);
        parts.join(" + ")
    }
}

#[derive(Debug)]
pub enum HotkeyEvent {
    Registered(Hotkey),
    Pressed(Target, std::time::Instant),
    /// The accepted chord was released. Never captures a new foreground target.
    Released(std::time::Instant),
    Error {
        requested: Hotkey,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    window: isize,
    focus: isize,
    process: u32,
}

impl Target {
    pub fn is_external(self) -> bool {
        self.window != 0 && self.process != 0
    }
}

#[cfg(test)]
mod target_tests {
    use super::*;

    #[test]
    fn shortcut_settings_round_trip_and_old_settings_use_default() {
        assert_eq!(HotkeyMode::default(), HotkeyMode::Hold);
        assert_eq!(
            serde_json::from_str::<HotkeyMode>("\"Hold\"").unwrap(),
            HotkeyMode::Hold
        );
        assert_eq!(
            serde_json::from_str::<Hotkey>("{}").unwrap(),
            Hotkey::default()
        );
        let custom = Hotkey {
            ctrl: false,
            alt: true,
            shift: true,
            win: true,
            key: "F8".into(),
        };
        let restored: Hotkey =
            serde_json::from_str(&serde_json::to_string(&custom).unwrap()).unwrap();
        assert_eq!(restored, custom);
        assert_eq!(restored.label(), "Alt + Shift + Win + F8");
    }

    #[test]
    fn ordinary_typing_and_unknown_keys_cannot_be_registered() {
        let mut custom = Hotkey {
            ctrl: false,
            alt: false,
            key: "A".into(),
            ..Hotkey::default()
        };
        assert!(custom.validate().is_err());
        custom.shift = true;
        assert!(custom.validate().is_err());
        custom.ctrl = true;
        assert!(custom.validate().is_ok());
        custom.key = "F25".into();
        assert!(custom.validate().is_err());
    }

    #[test]
    fn app_focused_shortcut_has_no_external_insertion_target() {
        assert!(!test_target().is_external());
        assert!(
            Target {
                window: 1,
                focus: 0,
                process: 2,
            }
            .is_external()
        );
    }
}

#[cfg(test)]
pub fn test_target() -> Target {
    Target {
        window: 0,
        focus: 0,
        process: 0,
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::Win32::{
        Foundation::*,
        System::{
            DataExchange::*,
            Memory::*,
            Threading::{
                GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
                QueryFullProcessImageNameW,
            },
        },
        UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    };

    pub fn target() -> Option<Target> {
        unsafe {
            let window = GetForegroundWindow();
            if window.is_null() {
                return None;
            }
            let mut process = 0;
            let thread = GetWindowThreadProcessId(window, &mut process);
            if process == GetCurrentProcessId() {
                return None;
            }
            let mut info: GUITHREADINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
            if GetGUIThreadInfo(thread, &mut info) == 0 {
                return None;
            }
            Some(Target {
                window: window as isize,
                focus: info.hwndFocus as isize,
                process,
            })
        }
    }
    pub fn app_name(target: Target) -> Option<String> {
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, target.process);
            if process.is_null() {
                return None;
            }
            let mut path = [0u16; 32768];
            let mut size = path.len() as u32;
            let ok = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut size);
            CloseHandle(process);
            if ok == 0 {
                return None;
            }
            let full = String::from_utf16(&path[..size as usize]).ok()?;
            std::path::Path::new(&full)
                .file_name()?
                .to_str()
                .map(str::to_lowercase)
        }
    }

    fn key_code(key: &str) -> u32 {
        match key {
            "Space" => VK_SPACE as u32,
            "Tab" => VK_TAB as u32,
            "Enter" => VK_RETURN as u32,
            "Escape" => VK_ESCAPE as u32,
            "Backspace" => VK_BACK as u32,
            "Delete" => VK_DELETE as u32,
            "Insert" => VK_INSERT as u32,
            "Home" => VK_HOME as u32,
            "End" => VK_END as u32,
            "PageUp" => VK_PRIOR as u32,
            "PageDown" => VK_NEXT as u32,
            "ArrowUp" => VK_UP as u32,
            "ArrowDown" => VK_DOWN as u32,
            "ArrowLeft" => VK_LEFT as u32,
            "ArrowRight" => VK_RIGHT as u32,
            _ if key.len() == 1 => key.as_bytes()[0] as u32,
            _ => VK_F1 as u32 + key[1..].parse::<u32>().expect("validated function key") - 1,
        }
    }

    fn modifiers(shortcut: &Hotkey) -> u32 {
        MOD_NOREPEAT
            | if shortcut.ctrl { MOD_CONTROL } else { 0 }
            | if shortcut.alt { MOD_ALT } else { 0 }
            | if shortcut.shift { MOD_SHIFT } else { 0 }
            | if shortcut.win { MOD_WIN } else { 0 }
    }

    fn chord_state(shortcut: &Hotkey) -> (bool, bool) {
        let down = |key: u32| unsafe { GetAsyncKeyState(key as i32) < 0 };
        let mut any = down(key_code(&shortcut.key));
        let mut complete = any;
        for (required, held) in [
            (shortcut.ctrl, down(VK_CONTROL as u32)),
            (shortcut.alt, down(VK_MENU as u32)),
            (shortcut.shift, down(VK_SHIFT as u32)),
            (shortcut.win, down(VK_LWIN as u32) || down(VK_RWIN as u32)),
        ] {
            if required {
                any |= held;
                complete &= held;
            }
        }
        (any, complete)
    }

    trait HotkeyRegistry {
        fn register(&mut self, id: i32, shortcut: &Hotkey) -> std::io::Result<()>;
        fn unregister(&mut self, id: i32);
    }

    struct WindowsRegistry;

    impl HotkeyRegistry for WindowsRegistry {
        fn register(&mut self, id: i32, shortcut: &Hotkey) -> std::io::Result<()> {
            unsafe {
                if RegisterHotKey(
                    std::ptr::null_mut(),
                    id,
                    modifiers(shortcut),
                    key_code(&shortcut.key),
                ) == 0
                {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            }
        }
        fn unregister(&mut self, id: i32) {
            unsafe {
                UnregisterHotKey(std::ptr::null_mut(), id);
            }
        }
    }

    #[derive(Default)]
    struct HotkeyRegistration {
        active: Option<(i32, Hotkey)>,
    }

    impl HotkeyRegistration {
        fn replace(
            &mut self,
            requested: Hotkey,
            registry: &mut impl HotkeyRegistry,
        ) -> Result<(), String> {
            requested.validate()?;
            if self
                .active
                .as_ref()
                .is_some_and(|(_, current)| current == &requested)
            {
                return Ok(());
            }
            let next_id = if self.active.as_ref().is_some_and(|(id, _)| *id == 1) {
                2
            } else {
                1
            };
            // Claim the new chord first. A conflict must never discard the
            // working shortcut or claim success in the saved preferences.
            registry.register(next_id, &requested).map_err(|error| {
                let fallback = self.active.as_ref().map_or_else(
                    || "Use the Record button.".to_owned(),
                    |(_, current)| format!("{} still works.", current.label()),
                );
                format!("{} is unavailable ({error}). {fallback}", requested.label())
            })?;
            if let Some((id, _)) = self.active.take() {
                registry.unregister(id);
            }
            self.active = Some((next_id, requested));
            Ok(())
        }

        fn accepts(&self, id: usize, chord: isize) -> bool {
            self.active.as_ref().is_some_and(|(active_id, shortcut)| {
                *active_id as usize == id
                    && (chord as u32 & 0xffff) == (modifiers(shortcut) & !MOD_NOREPEAT)
                    && ((chord as u32 >> 16) & 0xffff) == key_code(&shortcut.key)
            })
        }

        fn clear(&mut self, registry: &mut impl HotkeyRegistry) {
            if let Some((id, _)) = self.active.take() {
                registry.unregister(id);
            }
        }
    }

    /// Each service owns a separate Windows thread and message queue. Multiple
    /// services can use the same thread-local IDs; conflicting chords are still
    /// rejected globally and replacement keeps the previous chord registered.
    pub fn hotkey(initial: Hotkey, tx: Sender<HotkeyEvent>) -> Sender<Hotkey> {
        let (updates, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || unsafe {
            let mut registration = HotkeyRegistration::default();
            let mut registry = WindowsRegistry;
            let mut lifecycle = hotkey_lifecycle::Lifecycle::default();
            let apply = |requested: Hotkey,
                         registration: &mut HotkeyRegistration,
                         registry: &mut WindowsRegistry,
                         lifecycle: &mut hotkey_lifecycle::Lifecycle| {
                let changed = registration
                    .active
                    .as_ref()
                    .is_none_or(|(_, key)| *key != requested);
                let event = match registration.replace(requested.clone(), registry) {
                    Ok(()) => {
                        if changed
                            && lifecycle.register(chord_state(&requested).0)
                            && tx
                                .send(HotkeyEvent::Released(std::time::Instant::now()))
                                .is_err()
                        {
                            return false;
                        }
                        HotkeyEvent::Registered(requested)
                    }
                    Err(message) => HotkeyEvent::Error { requested, message },
                };
                tx.send(event).is_ok()
            };
            if !apply(initial, &mut registration, &mut registry, &mut lifecycle) {
                registration.clear(&mut registry);
                return;
            }
            'worker: loop {
                // Sleep on the command channel between message polls. This
                // also notices controller shutdown without an orphaned thread.
                match rx.recv_timeout(std::time::Duration::from_millis(20)) {
                    Ok(requested) => {
                        if !apply(requested, &mut registration, &mut registry, &mut lifecycle) {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    if msg.message == WM_QUIT {
                        break 'worker;
                    }
                    if msg.message == WM_HOTKEY
                        && registration.accepts(msg.wParam, msg.lParam)
                        && lifecycle.press()
                    {
                        let target = target().unwrap_or(Target {
                            window: 0,
                            focus: 0,
                            process: 0,
                        });
                        if tx
                            .send(HotkeyEvent::Pressed(target, std::time::Instant::now()))
                            .is_err()
                        {
                            break 'worker;
                        }
                    }
                }
                // Drain queued presses before rearming a registration which
                // began while keys were held. Release only ends the existing
                // gesture; it must never retarget a newly focused app.
                if let Some((_, shortcut)) = &registration.active {
                    let (any, complete) = chord_state(shortcut);
                    if lifecycle.poll(any, complete)
                        && tx
                            .send(HotkeyEvent::Released(std::time::Instant::now()))
                            .is_err()
                    {
                        break;
                    }
                }
            }
            registration.clear(&mut registry);
        });
        updates
    }

    pub fn insert(expected: Target, text: &str, same_field: impl Fn() -> bool) -> Result<()> {
        send_text(expected, text, false, same_field)
    }

    pub fn replace_selection(
        expected: Target,
        text: &str,
        same_field: impl Fn() -> bool,
    ) -> Result<()> {
        send_text(expected, text, true, same_field)
    }

    /// Extends a verified selection by one native left-arrow step. The caller
    /// must verify the resulting UIA range before replacing any selected text.
    pub fn select_previous_character(expected: Target, guard: impl Fn() -> bool) -> Result<()> {
        unsafe {
            for _ in 0..50 {
                if modifiers_released() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if !modifiers_released() {
                bail!("Release the modifier keys. Live typing paused.");
            }
            // UIA can take time to answer. Check native focus and modifiers
            // after it returns, immediately before queuing any keyboard input.
            if !guard() || target() != Some(expected) || !modifiers_released() {
                bail!("Focus changed. Live typing paused.");
            }
            send_selection(|input| {
                SendInput(
                    input.len() as u32,
                    input.as_ptr(),
                    std::mem::size_of::<INPUT>() as i32,
                )
            })
        }
    }

    unsafe fn modifiers_released() -> bool {
        [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
            .iter()
            .all(|key| unsafe { GetAsyncKeyState(*key as i32) >= 0 })
    }

    fn selection_key(key: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: key,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn send_selection(mut send: impl FnMut(&[INPUT]) -> u32) -> Result<()> {
        let input = [
            selection_key(VK_SHIFT, 0),
            selection_key(VK_LEFT, KEYEVENTF_EXTENDEDKEY),
            selection_key(VK_LEFT, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP),
            selection_key(VK_SHIFT, KEYEVENTF_KEYUP),
        ];
        let sent = send(&input);
        if sent == input.len() as u32 {
            return Ok(());
        }
        if sent > 0 {
            // A partial batch must not leave Shift or Left held. Cleanup sends
            // only releases, never another selection step or any text editing.
            let releases = [
                selection_key(VK_LEFT, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP),
                selection_key(VK_SHIFT, KEYEVENTF_KEYUP),
            ];
            for _ in 0..3 {
                if send(&releases) == releases.len() as u32 {
                    break;
                }
            }
        }
        bail!("Windows blocked selection. Live typing paused.")
    }

    fn send_text(
        expected: Target,
        text: &str,
        delete_empty: bool,
        same_field: impl Fn() -> bool,
    ) -> Result<()> {
        unsafe {
            // Let the shortcut modifiers go before emitting Unicode packets.
            for _ in 0..50 {
                if [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
                    .iter()
                    .all(|k| GetAsyncKeyState(*k as i32) >= 0)
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
                .iter()
                .any(|k| GetAsyncKeyState(*k as i32) < 0)
            {
                bail!("Release the modifier keys. Your transcript is ready to copy.");
            }
            let mut input = Vec::new();
            if text.is_empty() && delete_empty {
                for flags in [0, KEYEVENTF_KEYUP] {
                    input.push(INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: VK_BACK,
                                wScan: 0,
                                dwFlags: flags,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    });
                }
            }
            for code in text.encode_utf16() {
                for flags in [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP] {
                    input.push(INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: 0,
                                wScan: code,
                                dwFlags: flags,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    });
                }
            }
            if input.is_empty() {
                return Ok(());
            }
            // UIA providers may take hundreds of milliseconds to answer. The
            // native focus and modifier observations must follow that read,
            // immediately before emitting the prepared input batch.
            check_insertion_target(expected, same_field, target, || modifiers_released())?;
            let sent = SendInput(
                input.len() as u32,
                input.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            );
            if sent != input.len() as u32 {
                bail!(
                    "Windows blocked some or all of the insertion. Check the destination before copying to avoid duplicates."
                );
            }
            Ok(())
        }
    }

    fn check_insertion_target(
        expected: Target,
        same_field: impl FnOnce() -> bool,
        current_target: impl FnOnce() -> Option<Target>,
        modifiers_up: impl FnOnce() -> bool,
    ) -> Result<()> {
        if !same_field() || current_target() != Some(expected) {
            bail!("Focus changed. Your transcript is ready to copy.");
        }
        if !modifiers_up() {
            bail!("Release the modifier keys. Your transcript is ready to copy.");
        }
        Ok(())
    }

    pub fn copy(text: &str) -> Result<()> {
        unsafe {
            if OpenClipboard(GetForegroundWindow()) == 0 {
                bail!("Clipboard is busy. Please try again.");
            }
            struct Close;
            impl Drop for Close {
                fn drop(&mut self) {
                    unsafe {
                        CloseClipboard();
                    }
                }
            }
            let _close = Close;
            if EmptyClipboard() == 0 {
                bail!("Could not clear the clipboard");
            }
            // Exclusion flags must be written before the text. Abort if the
            // platform cannot set them, rather than leaking text to history.
            for name in ["CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"] {
                let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
                let format = RegisterClipboardFormatW(wide.as_ptr());
                anyhow::ensure!(format != 0, "Could not set clipboard privacy flags");
                put(format, &0u32.to_ne_bytes())?;
            }
            let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
            let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
            put(13, bytes)
        }
    }

    unsafe fn put(format: u32, bytes: &[u8]) -> Result<()> {
        unsafe {
            let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
            anyhow::ensure!(!memory.is_null(), "Could not allocate clipboard memory");
            let ptr = GlobalLock(memory);
            if ptr.is_null() {
                GlobalFree(memory);
                bail!("Could not access clipboard memory");
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
            GlobalUnlock(memory);
            if SetClipboardData(format, memory).is_null() {
                GlobalFree(memory);
                bail!("Could not update clipboard");
            }
            Ok(())
        }
    }
    #[cfg(test)]
    mod hotkey_tests {
        use super::*;

        #[test]
        fn insertion_checks_native_state_after_slow_field_validation() {
            let expected = Target {
                window: 1,
                focus: 2,
                process: 3,
            };
            let changed = Target {
                window: 4,
                focus: 5,
                process: 6,
            };
            let current = std::cell::Cell::new(Some(expected));
            assert!(
                check_insertion_target(
                    expected,
                    || {
                        current.set(Some(changed));
                        true
                    },
                    || current.get(),
                    || true,
                )
                .is_err()
            );
            let modifiers_up = std::cell::Cell::new(true);
            assert!(
                check_insertion_target(
                    expected,
                    || {
                        modifiers_up.set(false);
                        true
                    },
                    || Some(expected),
                    || modifiers_up.get(),
                )
                .is_err()
            );
            assert!(check_insertion_target(expected, || true, || Some(expected), || true).is_ok());
        }

        #[test]
        fn selection_releases_modifiers_after_every_partial_batch() {
            for accepted in 0..=4 {
                let mut batches = Vec::new();
                let result = send_selection(|input| {
                    let keys: Vec<_> = input
                        .iter()
                        .map(|event| unsafe {
                            assert_eq!(event.r#type, INPUT_KEYBOARD);
                            let key = event.Anonymous.ki;
                            (key.wVk, key.dwFlags)
                        })
                        .collect();
                    batches.push(keys);
                    if batches.len() == 1 {
                        accepted
                    } else {
                        input.len() as u32
                    }
                });
                assert_eq!(result.is_ok(), accepted == 4);
                assert_eq!(
                    batches[0],
                    vec![
                        (VK_SHIFT, 0),
                        (VK_LEFT, KEYEVENTF_EXTENDEDKEY),
                        (VK_LEFT, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP),
                        (VK_SHIFT, KEYEVENTF_KEYUP)
                    ]
                );
                if (1..4).contains(&accepted) {
                    assert_eq!(batches.len(), 2);
                    assert_eq!(
                        batches[1],
                        vec![
                            (VK_LEFT, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP),
                            (VK_SHIFT, KEYEVENTF_KEYUP)
                        ]
                    );
                } else {
                    assert_eq!(batches.len(), 1);
                }
            }
        }

        #[test]
        fn blocked_selection_cleanup_is_bounded_and_never_repeats_keydown() {
            let mut batches = 0;
            assert!(
                send_selection(|input| {
                    batches += 1;
                    if batches == 1 {
                        return 1;
                    }
                    assert!(
                        input
                            .iter()
                            .all(|key| unsafe { key.Anonymous.ki.dwFlags & KEYEVENTF_KEYUP != 0 })
                    );
                    0
                })
                .is_err()
            );
            assert_eq!(batches, 4);
        }

        #[derive(Default)]
        struct Registry {
            fail: bool,
            calls: Vec<(bool, i32)>,
        }
        impl HotkeyRegistry for Registry {
            fn register(&mut self, id: i32, _: &Hotkey) -> std::io::Result<()> {
                self.calls.push((true, id));
                if self.fail {
                    Err(std::io::Error::from_raw_os_error(1409))
                } else {
                    Ok(())
                }
            }
            fn unregister(&mut self, id: i32) {
                self.calls.push((false, id));
            }
        }

        #[test]
        fn conflict_retains_working_shortcut_and_retry_replaces_it_atomically() {
            let mut registration = HotkeyRegistration::default();
            let mut registry = Registry::default();
            registration
                .replace(Hotkey::default(), &mut registry)
                .unwrap();
            let requested = Hotkey {
                key: "F9".into(),
                ..Hotkey::default()
            };
            registry.fail = true;
            let error = registration
                .replace(requested.clone(), &mut registry)
                .unwrap_err();
            assert!(error.contains("Ctrl + Alt + Space still works"));
            assert_eq!(registration.active.as_ref().unwrap().1, Hotkey::default());
            assert_eq!(registry.calls, vec![(true, 1), (true, 2)]);
            registry.fail = false;
            registration
                .replace(requested.clone(), &mut registry)
                .unwrap();
            assert_eq!(registration.active.as_ref().unwrap().1, requested);
            assert_eq!(
                registry.calls,
                vec![(true, 1), (true, 2), (true, 2), (false, 1)]
            );
        }

        #[test]
        fn rejected_validation_and_unchanged_settings_do_not_disturb_registration() {
            let mut registration = HotkeyRegistration::default();
            let mut registry = Registry::default();
            registration
                .replace(Hotkey::default(), &mut registry)
                .unwrap();
            registration
                .replace(Hotkey::default(), &mut registry)
                .unwrap();
            assert!(
                registration
                    .replace(
                        Hotkey {
                            key: "unknown".into(),
                            ..Hotkey::default()
                        },
                        &mut registry
                    )
                    .is_err()
            );
            assert_eq!(registry.calls, vec![(true, 1)]);
        }

        #[test]
        fn queued_events_from_replaced_shortcuts_are_ignored() {
            let mut registration = HotkeyRegistration::default();
            let mut registry = Registry::default();
            registration
                .replace(Hotkey::default(), &mut registry)
                .unwrap();
            let old_chord = ((VK_SPACE as u32) << 16 | MOD_CONTROL | MOD_ALT) as isize;
            assert!(registration.accepts(1, old_chord));
            registration
                .replace(
                    Hotkey {
                        key: "F9".into(),
                        ..Hotkey::default()
                    },
                    &mut registry,
                )
                .unwrap();
            assert!(!registration.accepts(1, old_chord));
            assert!(!registration.accepts(2, old_chord));
            registration.clear(&mut registry);
            assert!(registration.active.is_none());
            assert_eq!(registry.calls.last(), Some(&(false, 2)));
        }

        #[test]
        fn every_selectable_key_has_a_windows_mapping() {
            for &key in HOTKEY_KEYS {
                let hotkey = Hotkey {
                    key: key.into(),
                    ..Hotkey::default()
                };
                hotkey.validate().unwrap();
                assert_ne!(key_code(key), 0);
            }
        }

        #[test]
        #[ignore = "Temporarily registers rare function-key shortcuts; uses no injected keypresses"]
        fn windows_conflict_preserves_existing_registration() {
            let blocked = Hotkey {
                shift: true,
                key: "F24".into(),
                ..Hotkey::default()
            };
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let holder = std::thread::spawn(move || {
                let mut registry = WindowsRegistry;
                let result = registry.register(21, &blocked);
                let succeeded = result.is_ok();
                ready_tx.send(result).unwrap();
                let _ = done_rx.recv_timeout(std::time::Duration::from_secs(5));
                if succeeded {
                    registry.unregister(21);
                }
            });
            let ready = ready_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            let outcome = (|| {
                ready?;
                let mut registration = HotkeyRegistration::default();
                let mut registry = WindowsRegistry;
                let existing = Hotkey {
                    shift: true,
                    key: "F23".into(),
                    ..Hotkey::default()
                };
                registration
                    .replace(existing.clone(), &mut registry)
                    .map_err(std::io::Error::other)?;
                let result = registration.replace(
                    Hotkey {
                        shift: true,
                        key: "F24".into(),
                        ..Hotkey::default()
                    },
                    &mut registry,
                );
                let retained = registration
                    .active
                    .as_ref()
                    .is_some_and(|(_, current)| current == &existing);
                registration.clear(&mut registry);
                if result.is_ok() || !retained {
                    return Err(std::io::Error::other(
                        "The conflicting shortcut must fail and preserve the existing registration",
                    ));
                }
                Ok::<(), std::io::Error>(())
            })();
            let _ = done_tx.send(());
            holder.join().unwrap();
            outcome.unwrap();
        }
    }
}

#[cfg(windows)]
pub use win::{
    app_name, copy, hotkey, insert, replace_selection, select_previous_character, target,
};

#[cfg(not(windows))]
pub fn app_name(_: Target) -> Option<String> {
    None
}

#[cfg(not(windows))]
pub fn hotkey(initial: Hotkey, tx: Sender<HotkeyEvent>) -> Sender<Hotkey> {
    let (updates, _) = std::sync::mpsc::channel();
    let _ = tx.send(HotkeyEvent::Error {
        requested: initial,
        message: "Global shortcuts are currently available on Windows.".into(),
    });
    updates
}
#[cfg(not(windows))]
pub fn insert(_: Target, _: &str, _: impl Fn() -> bool) -> Result<()> {
    bail!("Insertion is currently available on Windows");
}
#[cfg(not(windows))]
pub fn select_previous_character(_: Target, _: impl Fn() -> bool) -> Result<()> {
    bail!("Keyboard selection is currently available on Windows");
}
#[cfg(not(windows))]
pub fn target() -> Option<Target> {
    None
}
#[cfg(not(windows))]
pub fn copy(_: &str) -> Result<()> {
    bail!("Clipboard is currently available on Windows");
}
