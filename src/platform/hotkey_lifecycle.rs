//! Tracks physical gestures independently of foreground focus and app mode.
//! Polling only ends a gesture or arms a fresh registration; only WM_HOTKEY
//! may start one. A chord already held at registration must be fully released.
#[derive(Default)]
pub(super) struct Lifecycle {
    armed: bool,
    pressed: bool,
}

impl Lifecycle {
    pub(super) fn register(&mut self, any_bound_key_down: bool) -> bool {
        let released = self.pressed;
        self.pressed = false;
        self.armed = !any_bound_key_down;
        released
    }

    pub(super) fn press(&mut self) -> bool {
        if !self.armed || self.pressed {
            return false;
        }
        self.pressed = true;
        true
    }

    pub(super) fn poll(&mut self, any_bound_key_down: bool, complete_chord_down: bool) -> bool {
        if !self.armed && !any_bound_key_down {
            self.armed = true;
        }
        let released = self.pressed && !complete_chord_down;
        if released {
            self.pressed = false;
        }
        released
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_at_registration_never_starts_without_a_new_press() {
        let mut state = Lifecycle::default();
        assert!(!state.register(true));
        assert!(!state.press());
        assert!(!state.poll(true, false));
        assert!(!state.press());
        assert!(!state.poll(false, false));
        assert!(state.press());
    }

    #[test]
    fn repeats_do_not_toggle_and_release_is_emitted_once() {
        let mut state = Lifecycle::default();
        state.register(false);
        assert!(state.press());
        assert!(!state.press());
        assert!(!state.poll(true, true));
        assert!(state.poll(true, false)); // Main key or required modifier released.
        assert!(!state.poll(false, false));
        assert!(state.press());
    }

    #[test]
    fn quick_tap_still_finishes_and_polling_never_starts() {
        let mut state = Lifecycle::default();
        state.register(false);
        assert!(!state.poll(true, true));
        assert!(state.press());
        assert!(state.poll(false, false));
        assert!(!state.poll(true, true));
    }

    #[test]
    fn registration_change_finishes_old_gesture_and_requires_release() {
        let mut state = Lifecycle::default();
        state.register(false);
        assert!(state.press());
        assert!(state.register(true));
        assert!(!state.press());
        assert!(!state.poll(false, false));
        assert!(state.press());
    }
}
