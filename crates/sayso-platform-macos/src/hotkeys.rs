//! [`HotkeySource`] for macOS.
//!
//! - Chords (toggle, paste last, cycle style) go through Carbon with the
//!   `global-hotkey` crate. No permission is needed.
//! - Push-to-talk and Esc go through our own listen-only event tap
//!   (see [`crate::tap`]). That needs Input Monitoring.
//!
//! Carbon hotkeys only fire while the main thread runs an event loop. GPUI
//! does. A bare `CFRunLoop` does not (spike result).

use crate::{ffi, keymap, secure_input, tap::TapController};
use crossbeam_channel::{Receiver, Sender};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use objc2::MainThreadMarker;
use parking_lot::Mutex;
use sayso_core::hotkey::Hotkey;
use sayso_platform::{HotkeyBindings, HotkeyConflict, HotkeyEvent, HotkeyRegistration, HotkeySource};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct Carbon {
    manager: Option<GlobalHotKeyManager>,
    registered: Vec<HotKey>,
}

pub struct MacHotkeys {
    carbon: Mutex<Carbon>,
    /// `global-hotkey` event id to the action it triggers.
    actions: Arc<Mutex<HashMap<u32, HotkeyEvent>>>,
    rx: Receiver<HotkeyEvent>,
    tap: TapController,
}

impl MacHotkeys {
    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let capturing = Arc::new(AtomicBool::new(false));
        let actions: Arc<Mutex<HashMap<u32, HotkeyEvent>>> = Arc::default();
        spawn_carbon_forwarder(actions.clone(), tx.clone(), capturing.clone());
        let tap = TapController::new(tx, capturing);
        Self { carbon: Mutex::new(Carbon::default()), actions, rx, tap }
    }
}

impl Default for MacHotkeys {
    fn default() -> Self {
        Self::new()
    }
}

/// `global-hotkey` has one process-wide receiver. This thread turns its
/// presses into [`HotkeyEvent`]s. It ends when the receiving side is gone.
fn spawn_carbon_forwarder(
    actions: Arc<Mutex<HashMap<u32, HotkeyEvent>>>,
    tx: Sender<HotkeyEvent>,
    capturing: Arc<AtomicBool>,
) {
    let spawned = std::thread::Builder::new().name("sayso-hotkey-forward".into()).spawn(move || {
        while let Ok(event) = GlobalHotKeyEvent::receiver().recv() {
            if event.state() != HotKeyState::Pressed || capturing.load(Ordering::SeqCst) {
                continue;
            }
            let action = actions.lock().get(&event.id()).copied();
            if let Some(action) = action
                && tx.send(action).is_err()
            {
                break;
            }
        }
    });
    if let Err(err) = spawned {
        log::error!("cannot start the hotkey forwarder: {err}");
    }
}

impl HotkeySource for MacHotkeys {
    fn register(&self, bindings: &HotkeyBindings) -> HotkeyRegistration {
        let mut failed: Vec<(Hotkey, String)> = Vec::new();

        // Chords through Carbon.
        let chords = [
            (bindings.toggle, HotkeyEvent::Toggle),
            (bindings.paste_last, HotkeyEvent::PasteLast),
            (bindings.cycle_style, HotkeyEvent::CycleStyle),
            (bindings.incognito, HotkeyEvent::ToggleIncognito),
        ];
        {
            let mut carbon = self.carbon.lock();
            self.actions.lock().clear();
            if MainThreadMarker::new().is_none() {
                let reason = "hotkeys must be registered on the main thread".to_string();
                failed.extend(chords.iter().filter_map(|(h, _)| h.map(|h| (h, reason.clone()))));
            } else {
                if carbon.manager.is_none() {
                    match GlobalHotKeyManager::new() {
                        Ok(manager) => carbon.manager = Some(manager),
                        Err(err) => log::error!("cannot create the hotkey manager: {err}"),
                    }
                }
                let old = std::mem::take(&mut carbon.registered);
                if let Some(manager) = &carbon.manager {
                    let _ = manager.unregister_all(&old);
                }
                for (hotkey, action) in chords {
                    let Some(hotkey) = hotkey else { continue };
                    match register_chord(&mut carbon, &hotkey) {
                        Ok(registered) => {
                            self.actions.lock().insert(registered.id(), action);
                            carbon.registered.push(registered);
                        }
                        Err(reason) => failed.push((hotkey, reason)),
                    }
                }
            }
        }

        // Push-to-talk and Esc through the tap.
        let ptt = bindings.push_to_talk;
        if let Err(reason) = self.tap.configure(ptt, bindings.single_escape, bindings.double_escape_window)
            && let Some(ptt) = ptt
        {
            failed.push((ptt, reason));
        }
        // SAFETY: plain permission query.
        let needs_input_monitoring = ptt.is_some() && !unsafe { ffi::CGPreflightListenEventAccess() };

        HotkeyRegistration { failed, needs_input_monitoring }
    }

    fn set_recording(&self, recording: bool) {
        if let Err(err) = self.tap.set_recording(recording) {
            log::warn!("Esc cancel is unavailable: {err}");
        }
    }

    fn events(&self) -> Receiver<HotkeyEvent> {
        self.rx.clone()
    }

    fn conflicts(&self, hotkey: &Hotkey) -> Vec<HotkeyConflict> {
        crate::conflicts::conflicts(hotkey)
    }

    fn secure_input_holder(&self) -> Option<String> {
        secure_input::holder()
    }

    fn capture_next(&self) -> Receiver<Hotkey> {
        self.tap.capture()
    }
}

fn register_chord(carbon: &mut Carbon, hotkey: &Hotkey) -> Result<HotKey, String> {
    let mapped = keymap::to_global_hotkey(hotkey)?;
    if carbon.registered.contains(&mapped) {
        return Err("another Sayso action already uses this shortcut".into());
    }
    let manager = carbon.manager.as_ref().ok_or("the hotkey manager is not available")?;
    manager.register(mapped).map_err(|e| e.to_string())?;
    Ok(mapped)
}
