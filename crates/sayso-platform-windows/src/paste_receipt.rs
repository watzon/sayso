// Copied from sayso-platform-macos; keep the two in step until they move to a shared crate.
//! When did the target app read the pasted text?
//!
//! The paste publishes the text as a promise. macOS then tells us each time
//! an app reads it. A read after Cmd+V is the receipt: an app took the text.
//! This module holds the times and decides what they mean. It has no macOS
//! code, so the rules are tested directly. The [`Finisher`] runs the rest of
//! a paste after the insert returns.

use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
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

/// The rest of a paste after its receipt: wait until the reads stop, then put
/// the old clipboard back or leave the text. It runs after the insert returns.
pub type Finish = Box<dyn FnOnce() + Send>;

/// Runs the finish of each paste on a thread of its own, one at a time.
///
/// The next insert waits for the finish of the last paste, so that its
/// snapshot never takes the dictated text for the user's clipboard, and two
/// pastes never use the clipboard at the same time.
#[derive(Debug, Default)]
pub struct Finisher {
    last: Mutex<Option<JoinHandle<()>>>,
}

impl Finisher {
    pub const fn new() -> Self {
        Self { last: Mutex::new(None) }
    }

    /// Wait for the finish of the last paste, then run `insert`. A finish that
    /// `insert` gives back runs after this call returns.
    pub fn insert<R, E>(&self, insert: impl FnOnce() -> Result<(R, Option<Finish>), E>) -> Result<R, E> {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(pending) = last.take() {
            let _ = pending.join();
        }
        let (result, finish) = insert()?;
        if let Some(finish) = finish {
            // Shared, so that the finish still runs here when no thread starts.
            let slot = Arc::new(Mutex::new(Some(finish)));
            let theirs = Arc::clone(&slot);
            match std::thread::Builder::new().name("sayso-clipboard-restore".into()).spawn(move || run_once(&theirs)) {
                Ok(handle) => *last = Some(handle),
                Err(e) => {
                    log::warn!("cannot start the clipboard restore, so it runs now: {e}");
                    run_once(&slot);
                }
            }
        }
        Ok(result)
    }
}

fn run_once(slot: &Mutex<Option<Finish>>) {
    let finish = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(finish) = finish {
        finish();
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

    #[test]
    fn the_insert_returns_before_its_finish_and_the_next_insert_waits_for_it() {
        let finisher = Finisher::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let theirs = Arc::clone(&events);
        let started = Instant::now();
        let first: Result<u8, ()> = finisher.insert(|| {
            let finish: Finish = Box::new(move || {
                std::thread::sleep(ms(150));
                theirs.lock().unwrap().push("first finished");
            });
            Ok((1, Some(finish)))
        });
        assert_eq!(first, Ok(1));
        assert!(started.elapsed() < ms(150), "returned after {:?}", started.elapsed());
        events.lock().unwrap().push("first returned");
        let second: Result<u8, ()> = finisher.insert(|| {
            events.lock().unwrap().push("second started");
            Ok((2, None))
        });
        assert_eq!(second, Ok(2));
        assert_eq!(*events.lock().unwrap(), ["first returned", "first finished", "second started"]);
    }

    #[test]
    fn a_failed_insert_leaves_nothing_to_wait_for() {
        let finisher = Finisher::new();
        let result: Result<(), &str> = finisher.insert(|| Err("no key"));
        assert_eq!(result, Err("no key"));
        assert!(finisher.last.lock().unwrap().is_none());
    }
}
