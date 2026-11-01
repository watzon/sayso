//! Dictation first (`docs/extensions.md`, "Scheduling"): a job waits between
//! two engine calls while a dictation runs.
//!
//! Jobs and live sessions ask a [`Gate`] before their engine calls.
//! [`DictationGate`] is the gate of Sayso. An app without dictation (the
//! local mode of the SDK) uses it too and never calls `begin`, so live
//! sessions still come before jobs.
//!
//! The app calls [`DictationGate::begin`] when a dictation starts its capture
//! and [`DictationGate::end`] when the dictation no longer needs the engine.
//! A dictation of Sayso (`sayso-app`: `dictation.rs` and the state machine in
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
//! So Sayso does not call `end` at each of those places. It calls `begin`
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
//!
//! Live sessions come before jobs too. A live session waits only for
//! dictation ([`DictationGate::wait_idle`]). While it transcribes a part, it
//! holds a [`LivePart`], and a job waits for that part before its next
//! engine call ([`DictationGate::wait_for_job`]). The part ends when the
//! guard drops, also when the engine call fails or panics.

use parking_lot::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Dictation first, then live sessions, then jobs (`docs/extensions.md`,
/// "Scheduling"). The final pass of a part and the diarization of a live
/// session wait while a dictation runs. The preview stream and the VAD do not
/// wait. While a part or a diarization is in the engine, jobs wait.
pub trait Gate: Send + Sync {
    /// Wait until no dictation runs: for live sessions. Returns false, at
    /// once, when `stop` becomes true (the work was cancelled).
    fn wait_idle(&self, stop: &dyn Fn() -> bool) -> bool;

    /// Wait until no dictation runs and no live session gives the engine a
    /// part: for jobs. Returns false, at once, when `stop` becomes true.
    fn wait_for_job(&self, stop: &dyn Fn() -> bool) -> bool {
        self.wait_idle(stop)
    }

    /// A live session gives the engine a part now. Jobs wait until the value drops.
    fn engine_call(&self) -> Box<dyn Send + '_> {
        Box::new(())
    }

    /// Wake the jobs that wait, so a cancelled job stops at once.
    fn wake(&self) {}
}

/// No dictation to wait for, and jobs do not wait for live sessions.
pub struct NoGate;

impl Gate for NoGate {
    fn wait_idle(&self, stop: &dyn Fn() -> bool) -> bool {
        !stop()
    }
}

/// A part of a live session that the engine transcribes now. Jobs wait
/// until it drops.
pub struct LivePart<'a> {
    gate: &'a DictationGate,
}

impl Drop for LivePart<'_> {
    fn drop(&mut self) {
        let mut state = self.gate.state.lock();
        state.live_parts = state.live_parts.saturating_sub(1);
        drop(state);
        self.gate.changed.notify_all();
    }
}

/// The longest that a dictation holds the jobs: the longest recording (10
/// minutes, `Timing::max_duration_ms`), its final pass, and its style.
pub const STALE_AFTER: Duration = Duration::from_secs(20 * 60);

/// How often a waiting job looks again, also without a wake-up.
const LOOK_AGAIN: Duration = Duration::from_millis(250);

pub struct DictationGate {
    state: Mutex<GateState>,
    changed: Condvar,
    stale_after: Duration,
}

#[derive(Default)]
struct GateState {
    /// When the dictation that runs began.
    began: Option<Instant>,
    /// The parts that live sessions transcribe now.
    live_parts: usize,
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
        DictationGate { state: Mutex::new(GateState::default()), changed: Condvar::new(), stale_after }
    }

    /// A dictation starts its capture. A dictation that already runs keeps
    /// its start time.
    pub fn begin(&self) {
        let mut state = self.state.lock();
        if !self.holds(&mut state.began) {
            state.began = Some(Instant::now());
        }
    }

    /// The dictation no longer needs the engine. Without a dictation, nothing happens.
    pub fn end(&self) {
        self.state.lock().began = None;
        self.changed.notify_all();
    }

    /// True while a dictation holds the jobs.
    pub fn dictating(&self) -> bool {
        self.holds(&mut self.state.lock().began)
    }

    /// A live session starts to transcribe a part. Jobs wait until the guard drops.
    pub fn live_part(&self) -> LivePart<'_> {
        self.state.lock().live_parts += 1;
        LivePart { gate: self }
    }

    /// Wait until no dictation runs: for live sessions. Returns false, at
    /// once, when `stop` becomes true (the work was cancelled); see
    /// [`DictationGate::wake`].
    pub fn wait_idle(&self, stop: &dyn Fn() -> bool) -> bool {
        self.wait(stop, false)
    }

    /// Wait until no dictation runs and no live session transcribes a part:
    /// for jobs. Returns false, at once, when `stop` becomes true.
    pub fn wait_for_job(&self, stop: &dyn Fn() -> bool) -> bool {
        self.wait(stop, true)
    }

    fn wait(&self, stop: &dyn Fn() -> bool, for_job: bool) -> bool {
        let mut state = self.state.lock();
        loop {
            if stop() {
                return false;
            }
            let held = self.holds(&mut state.began) || (for_job && state.live_parts > 0);
            if !held {
                return true;
            }
            self.changed.wait_for(&mut state, LOOK_AGAIN);
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

/// The gate of Sayso holds live sessions while a dictation runs, and holds
/// jobs while a dictation runs or a live session uses the engine.
impl Gate for DictationGate {
    fn wait_idle(&self, stop: &dyn Fn() -> bool) -> bool {
        DictationGate::wait_idle(self, stop)
    }

    fn wait_for_job(&self, stop: &dyn Fn() -> bool) -> bool {
        DictationGate::wait_for_job(self, stop)
    }

    fn engine_call(&self) -> Box<dyn Send + '_> {
        Box::new(self.live_part())
    }

    fn wake(&self) {
        DictationGate::wake(self);
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
    fn a_job_waits_for_a_part_of_a_live_session_but_a_live_session_does_not() {
        let gate = Arc::new(DictationGate::new());
        let part = gate.live_part();
        assert!(gate.wait_idle(&|| false), "a live session waits only for dictation");
        let passed = Arc::new(AtomicBool::new(false));
        let waiter = {
            let (gate, passed) = (gate.clone(), passed.clone());
            std::thread::spawn(move || {
                let idle = gate.wait_for_job(&|| false);
                passed.store(true, Ordering::SeqCst);
                idle
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        assert!(!passed.load(Ordering::SeqCst), "the job waits for the part");
        let second = gate.live_part();
        drop(part);
        std::thread::sleep(Duration::from_millis(50));
        assert!(!passed.load(Ordering::SeqCst), "and for each part");
        drop(second);
        assert!(waiter.join().unwrap());
        // A dictation still holds the job without a part.
        gate.begin();
        let stop = AtomicBool::new(false);
        let gate2 = gate.clone();
        let check = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            gate2.end();
        });
        assert!(gate.wait_for_job(&|| stop.load(Ordering::SeqCst)));
        check.join().unwrap();
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
