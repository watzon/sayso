// Copied from sayso-platform-macos; keep the two in step until they move to a shared crate.
//! Cancel detection for the Esc key: double Esc inside a time window, or a
//! single Esc when the user asks for it.
//!
//! Pure state machine. The caller passes the time of each press, so the tests
//! need no sleeping.

use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct EscDetector {
    single: bool,
    window: Duration,
    last: Option<Instant>,
}

impl EscDetector {
    pub fn new(single: bool, window: Duration) -> Self {
        Self { single, window, last: None }
    }

    pub fn configure(&mut self, single: bool, window: Duration) {
        self.single = single;
        self.window = window;
        self.last = None;
    }

    /// Forget a half-finished double press. Call when recording starts or stops.
    pub fn reset(&mut self) {
        self.last = None;
    }

    /// Feed one Esc key-down. Returns true when the press completes a cancel.
    /// Auto-repeat must be filtered out by the caller.
    pub fn press(&mut self, now: Instant) -> bool {
        if self.single {
            self.last = None;
            return true;
        }
        let double = self.last.is_some_and(|prev| now.saturating_duration_since(prev) <= self.window);
        self.last = if double { None } else { Some(now) };
        double
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Duration = Duration::from_millis(400);

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn two_presses_inside_the_window_cancel() {
        let t0 = Instant::now();
        let mut d = EscDetector::new(false, WINDOW);
        assert!(!d.press(t0));
        assert!(d.press(at(t0, 180)));
    }

    #[test]
    fn slow_second_press_starts_a_new_attempt() {
        let t0 = Instant::now();
        let mut d = EscDetector::new(false, WINDOW);
        assert!(!d.press(t0));
        assert!(!d.press(at(t0, 770)));
        // The slow press became the first press of a new pair.
        assert!(d.press(at(t0, 900)));
    }

    #[test]
    fn the_window_edge_counts() {
        let t0 = Instant::now();
        let mut d = EscDetector::new(false, WINDOW);
        assert!(!d.press(t0));
        assert!(d.press(at(t0, 400)));
    }

    #[test]
    fn a_cancel_consumes_both_presses() {
        let t0 = Instant::now();
        let mut d = EscDetector::new(false, WINDOW);
        assert!(!d.press(t0));
        assert!(d.press(at(t0, 100)));
        // A third press is the start of a new pair, not another cancel.
        assert!(!d.press(at(t0, 200)));
    }

    #[test]
    fn single_mode_cancels_on_every_press() {
        let t0 = Instant::now();
        let mut d = EscDetector::new(true, WINDOW);
        assert!(d.press(t0));
        assert!(d.press(at(t0, 5000)));
    }

    #[test]
    fn reset_forgets_the_first_press() {
        let t0 = Instant::now();
        let mut d = EscDetector::new(false, WINDOW);
        assert!(!d.press(t0));
        d.reset();
        assert!(!d.press(at(t0, 100)));
    }

    #[test]
    fn configure_changes_mode_and_window() {
        let t0 = Instant::now();
        let mut d = EscDetector::new(false, WINDOW);
        d.configure(false, Duration::from_millis(50));
        assert!(!d.press(t0));
        assert!(!d.press(at(t0, 100)));
        d.configure(true, WINDOW);
        assert!(d.press(at(t0, 200)));
    }
}
