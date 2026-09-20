use std::time::{Duration, Instant};

const TAP_LIMIT: Duration = Duration::from_millis(220);
const DOUBLE_PRESS: Duration = Duration::from_millis(300);

/// Owns only a recording started by a Hold shortcut. A quick first tap waits
/// briefly for a second press; a normal hold finishes immediately on release.
#[derive(Default)]
pub(super) struct Gesture {
    pressed_at: Option<Instant>,
    released_at: Option<Instant>,
    pub(super) hands_free: bool,
}

impl Gesture {
    pub(super) fn start(&mut self, now: Instant) {
        *self = Self {
            pressed_at: Some(now),
            ..Self::default()
        };
    }

    /// Returns true when another press should finish the owned recording.
    pub(super) fn press_again(&mut self, now: Instant) -> bool {
        if self.hands_free {
            return true;
        }
        if let Some(released) = self.released_at.take() {
            if now.saturating_duration_since(released) <= DOUBLE_PRESS {
                self.hands_free = true;
                self.pressed_at = None;
                return false;
            }
            return true;
        }
        false
    }

    pub(super) fn release(&mut self, now: Instant) -> bool {
        if self.hands_free {
            return false;
        }
        let Some(pressed) = self.pressed_at.take() else {
            return false;
        };
        if now.saturating_duration_since(pressed) <= TAP_LIMIT {
            self.released_at = Some(now);
            false
        } else {
            true
        }
    }

    pub(super) fn expired(&self, now: Instant) -> bool {
        self.released_at
            .is_some_and(|at| now.saturating_duration_since(at) > DOUBLE_PRESS)
    }

    pub(super) fn awaiting_second_press(&self) -> bool {
        self.released_at.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normal_hold_finishes_on_release() {
        let now = Instant::now();
        let mut gesture = Gesture::default();
        gesture.start(now);
        assert!(gesture.release(now + Duration::from_millis(500)));
        assert!(!gesture.release(now + Duration::from_millis(600)));
    }
    #[test]
    fn double_press_latches_until_next_press() {
        let now = Instant::now();
        let mut gesture = Gesture::default();
        gesture.start(now);
        assert!(!gesture.release(now + Duration::from_millis(80)));
        assert!(!gesture.press_again(now + Duration::from_millis(200)));
        assert!(gesture.hands_free);
        assert!(!gesture.release(now + Duration::from_millis(260)));
        assert!(!gesture.expired(now + Duration::from_secs(5)));
        assert!(gesture.press_again(now + Duration::from_secs(5)));
    }
    #[test]
    fn single_tap_expires_and_late_press_cannot_latch() {
        let now = Instant::now();
        let mut gesture = Gesture::default();
        gesture.start(now);
        assert!(!gesture.release(now + Duration::from_millis(80)));
        assert!(!gesture.expired(now + Duration::from_millis(200)));
        assert!(gesture.expired(now + Duration::from_millis(400)));
        assert!(gesture.press_again(now + Duration::from_millis(400)));
        assert!(!gesture.hands_free);
    }
    #[test]
    fn repeats_and_unowned_releases_do_nothing() {
        let now = Instant::now();
        let mut gesture = Gesture::default();
        assert!(!gesture.release(now));
        gesture.start(now);
        assert!(!gesture.press_again(now + Duration::from_millis(50)));
        assert!(!gesture.hands_free);
    }
}
