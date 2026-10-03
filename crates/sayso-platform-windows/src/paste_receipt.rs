// Copied from sayso-platform-macos; keep the two in step until they move to a shared crate.
//! When did the target app read the pasted text?
//!
//! The paste publishes the text as a promise. macOS then tells us each time
//! an app reads it. A read after Cmd+V is the receipt: an app took the text.
//! This module holds the times and decides what they mean. It has no macOS
//! code, so the rules are tested directly.

use std::time::{Duration, Instant};

/// How long to wait for the first read after Cmd+V. A busy app reads within
/// tens of milliseconds. With no read in this time, nothing took the text.
pub const NO_READ_TIMEOUT: Duration = Duration::from_millis(1500);
/// The longest wait after the first read, for an app that keeps reading.
pub const MAX_SETTLE: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// No answer yet.
    Wait,
    /// An app read the text, and its reads have stopped.
    Taken,
    /// No app read the text after Cmd+V.
    NotTaken,
}

/// The reads of one paste.
#[derive(Debug, Default)]
pub struct Receipts {
    /// When Cmd+V was sent. A read before this time is not the target app: it
    /// is a clipboard manager that reacts to the clipboard change itself.
    sent_at: Option<Instant>,
    reads: Vec<Instant>,
    /// Another app wrote to the clipboard. Our promise is gone.
    replaced: bool,
}

impl Receipts {
    pub fn mark_sent(&mut self, at: Instant) {
        self.sent_at = Some(at);
    }

    pub fn record_read(&mut self, at: Instant) {
        self.reads.push(at);
    }

    pub fn mark_replaced(&mut self) {
        self.replaced = true;
    }

    /// The reads that count: at or after Cmd+V.
    fn reads_after_send(&self) -> impl Iterator<Item = Instant> + '_ {
        let sent_at = self.sent_at;
        self.reads.iter().copied().filter(move |read| sent_at.is_some_and(|sent| *read >= sent))
    }

    /// What the reads mean at `now`. `quiet` is how long the reads must have
    /// stopped: some apps read the clipboard several times for one paste.
    pub fn verdict(&self, now: Instant, quiet: Duration) -> Verdict {
        let Some(sent_at) = self.sent_at else { return Verdict::Wait };
        let (first, last) = (self.reads_after_send().min(), self.reads_after_send().max());
        match (first, last) {
            (Some(first), Some(last)) => {
                let settled = now.duration_since(last) >= quiet || now.duration_since(first) >= MAX_SETTLE;
                // After a replace no more reads can come.
                if settled || self.replaced { Verdict::Taken } else { Verdict::Wait }
            }
            _ if self.replaced => Verdict::NotTaken,
            _ if now.duration_since(sent_at) >= NO_READ_TIMEOUT => Verdict::NotTaken,
            _ => Verdict::Wait,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUIET: Duration = Duration::from_millis(200);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn nothing_is_decided_before_the_key_press() {
        let mut r = Receipts::default();
        r.record_read(Instant::now());
        assert_eq!(r.verdict(Instant::now() + ms(5000), QUIET), Verdict::Wait);
    }

    #[test]
    fn a_read_after_the_key_press_is_taken_when_the_reads_stop() {
        let t = Instant::now();
        let mut r = Receipts::default();
        r.mark_sent(t);
        r.record_read(t + ms(20));
        assert_eq!(r.verdict(t + ms(100), QUIET), Verdict::Wait, "the quiet time has not passed");
        assert_eq!(r.verdict(t + ms(220), QUIET), Verdict::Taken);
    }

    #[test]
    fn each_new_read_starts_the_quiet_time_again() {
        let t = Instant::now();
        let mut r = Receipts::default();
        r.mark_sent(t);
        r.record_read(t + ms(20));
        r.record_read(t + ms(150));
        assert_eq!(r.verdict(t + ms(300), QUIET), Verdict::Wait);
        assert_eq!(r.verdict(t + ms(350), QUIET), Verdict::Taken);
    }

    #[test]
    fn a_read_before_the_key_press_does_not_count() {
        // A clipboard manager reads when the clipboard changes, before Cmd+V.
        let t = Instant::now();
        let mut r = Receipts::default();
        r.record_read(t);
        r.mark_sent(t + ms(10));
        assert_eq!(r.verdict(t + ms(500), QUIET), Verdict::Wait);
        assert_eq!(r.verdict(t + ms(10) + NO_READ_TIMEOUT, QUIET), Verdict::NotTaken);
    }

    #[test]
    fn no_read_in_time_is_not_taken() {
        let t = Instant::now();
        let mut r = Receipts::default();
        r.mark_sent(t);
        assert_eq!(r.verdict(t + NO_READ_TIMEOUT - ms(1), QUIET), Verdict::Wait);
        assert_eq!(r.verdict(t + NO_READ_TIMEOUT, QUIET), Verdict::NotTaken);
    }

    #[test]
    fn an_app_that_never_stops_reading_is_taken_at_the_limit() {
        let t = Instant::now();
        let mut r = Receipts::default();
        r.mark_sent(t);
        let mut at = t;
        while at < t + MAX_SETTLE {
            r.record_read(at);
            at += ms(100);
        }
        assert_eq!(r.verdict(t + MAX_SETTLE, QUIET), Verdict::Taken);
    }

    #[test]
    fn a_replaced_clipboard_ends_the_wait() {
        let t = Instant::now();
        let mut unread = Receipts::default();
        unread.mark_sent(t);
        unread.mark_replaced();
        assert_eq!(unread.verdict(t + ms(1), QUIET), Verdict::NotTaken, "the app can only read the other content now");

        let mut read = Receipts::default();
        read.mark_sent(t);
        read.record_read(t + ms(5));
        read.mark_replaced();
        assert_eq!(read.verdict(t + ms(6), QUIET), Verdict::Taken);
    }
}
