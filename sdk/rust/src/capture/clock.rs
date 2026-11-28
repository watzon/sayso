//! The capture clock: a monotonic clock that each capture of this process
//! reads, so the microphone and the system audio of a session can be compared
//! (`docs/sdk.md`, "System audio", "Times").

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The time since the first read in this process.
pub fn now() -> Duration {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed()
}
