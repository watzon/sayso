//! Live input levels, shared between the audio thread and the overlay.
//!
//! The overlay reads this every frame, so it uses a plain mutex and never
//! notifies the model.

use parking_lot::Mutex;
use std::collections::VecDeque;

/// About 1.3 s of history at 20 ms frames.
const KEEP: usize = 64;

#[derive(Default)]
pub struct LiveAudio {
    levels: Mutex<VecDeque<f32>>,
}

impl LiveAudio {
    pub fn push(&self, level: f32) {
        let mut l = self.levels.lock();
        if l.len() >= KEEP {
            l.pop_front();
        }
        l.push_back(level.clamp(0.0, 1.0));
    }

    pub fn clear(&self) {
        self.levels.lock().clear();
    }

    /// The last `n` levels, oldest first, padded with silence.
    pub fn recent(&self, n: usize) -> Vec<f32> {
        let l = self.levels.lock();
        let mut out = vec![0.0; n.saturating_sub(l.len())];
        out.extend(l.iter().skip(l.len().saturating_sub(n)).copied());
        out
    }

    /// A smoothed level for the whole stroke.
    pub fn smoothed(&self, n: usize) -> Vec<f32> {
        let raw = self.recent(n + 2);
        (0..n).map(|i| (raw[i] + raw[i + 1] * 2.0 + raw[i + 2]) / 4.0).collect()
    }
}
