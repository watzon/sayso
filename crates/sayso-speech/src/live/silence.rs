//! The silence limit of a live session (`SessionSpec::silence_limit`).
//!
//! Each source says how long nobody has spoken in it. When every source has
//! been silent for the limit, the watch runs its action one time. The clock is
//! the audio that a source heard, so a source that gets no audio does not count
//! as silent, and the tests need no real time.

use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;

/// Runs when the limit passes. False: not now, ask again at the next audio.
type Action = Box<dyn Fn() -> bool + Send + Sync>;

pub(crate) struct SilenceWatch {
    limit_ms: u64,
    action: Action,
    /// The silence of each source so far, in ms. `None` after the action ran.
    silent_ms: Mutex<Option<Vec<u64>>>,
}

impl SilenceWatch {
    pub fn new(limit: Duration, sources: usize, action: Action) -> Arc<SilenceWatch> {
        Arc::new(SilenceWatch {
            limit_ms: limit.as_millis() as u64,
            action,
            silent_ms: Mutex::new(Some(vec![0; sources])),
        })
    }

    /// Source `index` has heard `silent_ms` of silence since its last speech.
    pub fn heard(&self, index: usize, silent_ms: u64) {
        let mut state = self.silent_ms.lock();
        let Some(sources) = state.as_mut() else {
            return;
        };
        sources[index] = silent_ms;
        if sources.iter().all(|&ms| ms >= self.limit_ms) && (self.action)() {
            *state = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn watch(limit_ms: u64, sources: usize, answer: bool) -> (Arc<SilenceWatch>, Arc<AtomicU32>) {
        let runs = Arc::new(AtomicU32::new(0));
        let counter = runs.clone();
        let action = Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            answer
        });
        (
            SilenceWatch::new(Duration::from_millis(limit_ms), sources, action),
            runs,
        )
    }

    #[test]
    fn it_runs_one_time_at_the_limit() {
        let (watch, runs) = watch(100, 1, true);
        watch.heard(0, 99);
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        watch.heard(0, 100);
        watch.heard(0, 200);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn it_waits_for_every_source() {
        let (watch, runs) = watch(100, 2, true);
        watch.heard(0, 500);
        assert_eq!(
            runs.load(Ordering::SeqCst),
            0,
            "the other source has heard nothing yet"
        );
        watch.heard(1, 50);
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        watch.heard(1, 100);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn it_asks_again_when_the_action_says_not_now() {
        let (watch, runs) = watch(100, 1, false);
        watch.heard(0, 100);
        watch.heard(0, 120);
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }
}
