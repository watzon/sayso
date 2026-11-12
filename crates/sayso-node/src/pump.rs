//! The thread that gives the events of the SDK to JavaScript, for `Speech` and
//! `EmbeddedSpeech`. It does not hold the event loop: the TypeScript layer does that while
//! work is active ("Events").
//!
//! A promise of the addon settles through its own threadsafe function, and Node gives no
//! order between two of them. So the result of `stopSession` can reach JavaScript before
//! the last `segment` event of the session. [`Pump::flush`] fixes the order: the thread
//! gives every event that waits, then [`FLUSHED`], through the one function of the events.
//! JavaScript waits for that text before it gives the result.

use crate::Outcome;
use crate::outcome_text;
use crossbeam_channel::{Receiver, Sender, bounded, select, unbounded};
use napi::Status;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};

/// The callback of the events, as JSON text of an outcome.
pub(crate) type EventFn = ThreadsafeFunction<String, (), String, Status, false, true>;

/// What the callback gets after the events that came before a flush. It is not an
/// outcome (`{"ok":…}` or `{"err":…}`), so JavaScript cannot take it for an event.
pub(crate) const FLUSHED: &str = r#"{"flushed":true}"#;

pub(crate) struct Pump {
    stop: Sender<()>,
    flush: Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

impl Pump {
    /// Start the thread. `value` makes the outcome of one event.
    pub fn start<E: Send + 'static>(name: &str, events: Receiver<E>, callback: EventFn, value: fn(E) -> Outcome) -> std::io::Result<Pump> {
        let (stop, stopped) = bounded::<()>(1);
        let (flush, flushes) = unbounded::<()>();
        // For the tests of the order only: a slow thread of the events, as on a busy
        // computer. Missing or not a number: no wait.
        let delay = std::env::var("SAYSO_NODE_EVENT_DELAY_MS").ok().and_then(|ms| ms.parse().ok()).map(std::time::Duration::from_millis);
        let thread = std::thread::Builder::new().name(name.into()).spawn(move || {
            let deliver = |event: E| {
                if let Some(delay) = delay {
                    std::thread::sleep(delay);
                }
                callback.call(outcome_text(value(event)), ThreadsafeFunctionCallMode::NonBlocking);
            };
            let flushed = || {
                callback.call(FLUSHED.to_string(), ThreadsafeFunctionCallMode::NonBlocking);
            };
            loop {
                select! {
                    recv(events) -> event => match event {
                        Ok(event) => deliver(event),
                        Err(_) => break,
                    },
                    recv(flushes) -> _ => {
                        events.try_iter().for_each(deliver);
                        flushed();
                    }
                    recv(stopped) -> _ => {
                        // The events that came before the close still reach the app.
                        events.try_iter().for_each(deliver);
                        break;
                    }
                }
            }
            // A flush that waits still gets its answer. One that comes later finds no
            // thread, and the TypeScript layer ends it at the close.
            flushes.try_iter().for_each(|()| flushed());
        })?;
        Ok(Pump { stop, flush, thread })
    }

    /// Ask for [`FLUSHED`] after the events that wait now. False when the thread ended:
    /// then no text comes.
    pub fn flush(&self) -> bool {
        !self.thread.is_finished() && self.flush.send(()).is_ok()
    }

    /// Give the events that wait, and end the thread.
    pub fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.thread.join();
    }
}
