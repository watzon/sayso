//! The shared end of the listen-only key stream. The evdev and XInput2
//! threads feed key events into it, and it delivers hotkey events and
//! captured hotkeys.

use super::logic::{KeyAction, KeyLogic};
use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use sayso_core::hotkey::Hotkey;
use sayso_platform::HotkeyEvent;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// An abandoned key recorder must not keep the next key press.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);
/// The chord backends may see the recorded key press after the listen-only
/// stream does. They stay muted this long after a capture.
const CAPTURE_MUTE: Duration = Duration::from_millis(500);

pub struct Listen {
    pub logic: Mutex<KeyLogic>,
    events: Sender<HotkeyEvent>,
    /// The open key recorder and its generation number.
    capture: Mutex<Option<(u64, Sender<Hotkey>)>>,
    generation: AtomicU64,
    captured_at: Mutex<Option<Instant>>,
}

impl Listen {
    pub fn new(events: Sender<HotkeyEvent>) -> Self {
        Self { logic: Mutex::new(KeyLogic::default()), events, capture: Mutex::new(None), generation: AtomicU64::new(0), captured_at: Mutex::new(None) }
    }

    /// Deliver one hotkey event. A closed receiver is not an error: the app quits.
    pub fn send(&self, event: HotkeyEvent) {
        let _ = self.events.send(event);
    }

    /// True while the key recorder is open, and shortly after it took a key.
    /// The chord backends drop their events then, so a recorded shortcut does
    /// not also run its action.
    pub fn chords_muted(&self) -> bool {
        self.logic.lock().is_capturing() || self.captured_at.lock().is_some_and(|t| t.elapsed() < CAPTURE_MUTE)
    }

    /// Feed one key event from a backend thread.
    pub fn key(&self, code: u16, action: KeyAction) {
        let outcome = self.logic.lock().handle(code, action, Instant::now());
        for event in outcome.events {
            self.send(event);
        }
        if let Some(hotkey) = outcome.captured
            && let Some((_, tx)) = self.capture.lock().take()
        {
            *self.captured_at.lock() = Some(Instant::now());
            let _ = tx.send(hotkey);
        }
    }

    /// The backend lost its keyboards or its connection. Forget the keys that
    /// are down and release a held push-to-talk.
    pub fn reset_keys(&self) {
        let released = self.logic.lock().reset_keys();
        if let Some(event) = released {
            self.send(event);
        }
    }

    /// Open the key recorder. The next completed hotkey arrives once on the
    /// returned channel. It closes without a value after a timeout or when
    /// [`Listen::cancel_capture`] runs.
    pub fn open_capture(self: &Arc<Self>) -> Receiver<Hotkey> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mine = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.capture.lock() = Some((mine, tx));
        self.logic.lock().set_capturing(true);
        let this = Arc::downgrade(self);
        let spawned = std::thread::Builder::new().name("sayso-capture-timeout".into()).spawn(move || {
            std::thread::sleep(CAPTURE_TIMEOUT);
            if let Some(this) = this.upgrade() {
                this.cancel_capture_if(mine);
            }
        });
        if let Err(err) = spawned {
            log::warn!("cannot start the key recorder timeout: {err}");
        }
        rx
    }

    /// Close the key recorder without a result.
    pub fn cancel_capture(&self) {
        self.capture.lock().take();
        self.logic.lock().set_capturing(false);
    }

    fn cancel_capture_if(&self, generation: u64) {
        let mut slot = self.capture.lock();
        if slot.as_ref().is_some_and(|(current, _)| *current == generation) {
            slot.take();
            self.logic.lock().set_capturing(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkeys::keymap::{KEY_LEFTALT, KEY_SPACE};

    #[test]
    fn a_capture_arrives_once_and_closes_the_recorder() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let listen = Arc::new(Listen::new(tx));
        let captured = listen.open_capture();
        assert!(listen.chords_muted());
        listen.key(KEY_LEFTALT, KeyAction::Down);
        listen.key(KEY_SPACE, KeyAction::Down);
        assert_eq!(captured.try_recv(), Ok("opt+space".parse().unwrap()));
        assert!(listen.chords_muted(), "muted briefly after the capture");
        assert!(!listen.logic.lock().is_capturing());
        assert!(captured.try_recv().is_err());
    }

    #[test]
    fn cancel_closes_the_channel_without_a_value() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let listen = Arc::new(Listen::new(tx));
        let captured = listen.open_capture();
        listen.cancel_capture();
        assert_eq!(captured.recv(), Err(crossbeam_channel::RecvError));
    }
}
