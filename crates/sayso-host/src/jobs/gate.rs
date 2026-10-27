//! Dictation first (`docs/extensions.md`, "Scheduling"): a job waits between
//! two engine calls while a dictation runs.
//!
//! The app calls [`DictationGate::begin`] when a dictation starts its capture
//! and [`DictationGate::end`] when the dictation no longer needs the engine.
//! A dictation of `sayso-app` (`dictation.rs` and the state machine in
//! `sayso_core::dictation`) ends in one of these ways:
//!
//! - the text is inserted (`Event::Inserted`), or the insert fails (`InsertFailed`);
//! - the final pass gives no text (`Transcribed` with empty text, or
//!   `ReadyToInsert` with empty text), or it fails (`TranscribeFailed`);
//! - the user cancels while recording or while processing (`Cancel`). Undo
//!   (`Event::Undo`) then runs the final pass again, so it begins again;
//! - the microphone does not start (`CaptureFailed`);
//! - the recording is shorter than the shortest dictation, and is discarded;
//! - Sayso quits during a dictation.
//!
//! So the app does not call `end` at each of those places. It calls `begin`
//! when its state machine enters `Recording` or `Processing`, and `end` when
//! it leaves both, in one place after each event (`AppModel::dispatch`).
//!
//! The rules make a missed or doubled call harmless:
//!
//! - `begin` and `end` set a state; they do not count. A second `begin` does
//!   nothing more, and one `end` ends the dictation. Sayso has one dictation
//!   at a time, so there is nothing to nest.
//! - A dictation that began more than [`STALE_AFTER`] ago no longer holds
//!   jobs, in case an `end` is lost on a path that this list does not know.

use parking_lot::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// The longest that a dictation holds the jobs: the longest recording (10
/// minutes, `Timing::max_duration_ms`), its final pass, and its style.
pub const STALE_AFTER: Duration = Duration::from_secs(20 * 60);

/// How often a waiting job looks again, also without a wake-up.
const LOOK_AGAIN: Duration = Duration::from_millis(250);

pub struct DictationGate {
    /// When the dictation that runs began.
    began: Mutex<Option<Instant>>,
    changed: Condvar,
    stale_after: Duration,
}

impl Default for DictationGate {
    fn default() -> Self {
        Self::new()
    }
}

impl DictationGate {
    pub fn new() -> DictationGate {
        Self::with_stale_after(STALE_AFTER)
    }

    /// A gate whose dictation holds jobs for at most `stale_after`.
    pub fn with_stale_after(stale_after: Duration) -> DictationGate {
        DictationGate { began: Mutex::new(None), changed: Condvar::new(), stale_after }
    }

    /// A dictation starts its capture. A dictation that already runs keeps
    /// its start time.
    pub fn begin(&self) {
        let mut began = self.began.lock();
        if !self.holds(&mut began) {
            *began = Some(Instant::now());
        }
    }

    /// The dictation no longer needs the engine. Without a dictation, nothing happens.
    pub fn end(&self) {
        *self.began.lock() = None;
        self.changed.notify_all();
    }

    /// True while a dictation holds the jobs.
    pub fn dictating(&self) -> bool {
        self.holds(&mut self.began.lock())
    }

    /// Wait until no dictation runs. Returns false, at once, when `stop`
    /// becomes true (the job was cancelled); see [`DictationGate::wake`].
    pub fn wait_idle(&self, stop: &dyn Fn() -> bool) -> bool {
        let mut began = self.began.lock();
        loop {
            if stop() {
                return false;
            }
            if !self.holds(&mut began) {
                return true;
            }
            self.changed.wait_for(&mut began, LOOK_AGAIN);
        }
    }

    /// Wake the jobs that wait, so a cancelled job stops at once.
    pub fn wake(&self) {
        self.changed.notify_all();
    }

    /// True while the dictation in `began` holds the jobs. A stale one is
    /// forgotten, with one warning.
    fn holds(&self, began: &mut Option<Instant>) -> bool {
        match *began {
            None => false,
            Some(at) if at.elapsed() < self.stale_after => true,
            Some(_) => {
                log::warn!("host: a dictation began more than {} s ago and did not end; jobs go on", self.stale_after.as_secs());
                *began = None;
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn begin_and_end_set_a_state_and_do_not_count() {
        let gate = DictationGate::new();
        assert!(!gate.dictating());
        gate.begin();
        gate.begin();
        assert!(gate.dictating());
        gate.end();
        assert!(!gate.dictating(), "one end ends the dictation");
        gate.end();
        assert!(!gate.dictating(), "an end without a dictation does nothing");
        assert!(gate.wait_idle(&|| false));
    }

    #[test]
    fn a_job_waits_until_the_dictation_ends() {
        let gate = Arc::new(DictationGate::new());
        gate.begin();
        let passed = Arc::new(AtomicBool::new(false));
        let waiter = {
            let (gate, passed) = (gate.clone(), passed.clone());
            std::thread::spawn(move || {
                let idle = gate.wait_idle(&|| false);
                passed.store(true, Ordering::SeqCst);
                idle
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        assert!(!passed.load(Ordering::SeqCst));
        gate.end();
        assert!(waiter.join().unwrap());
    }

    #[test]
    fn a_cancelled_job_stops_waiting() {
        let gate = Arc::new(DictationGate::new());
        gate.begin();
        let stop = Arc::new(AtomicBool::new(false));
        let waiter = {
            let (gate, stop) = (gate.clone(), stop.clone());
            std::thread::spawn(move || gate.wait_idle(&|| stop.load(Ordering::SeqCst)))
        };
        stop.store(true, Ordering::SeqCst);
        gate.wake();
        assert!(!waiter.join().unwrap());
    }

    #[test]
    fn a_lost_end_holds_jobs_for_a_bounded_time() {
        let gate = DictationGate::with_stale_after(Duration::from_millis(50));
        gate.begin();
        assert!(gate.dictating());
        let started = Instant::now();
        assert!(gate.wait_idle(&|| false));
        assert!(started.elapsed() >= Duration::from_millis(40));
        assert!(!gate.dictating());
        // The next dictation holds again, also when the lost end never comes.
        gate.begin();
        assert!(gate.dictating());
    }
}
