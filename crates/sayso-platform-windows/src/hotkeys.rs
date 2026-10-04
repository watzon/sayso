//! [`HotkeySource`] for Windows.
//!
//! - Chords (toggle, paste last, cycle style) go through `RegisterHotKey`
//!   with the `global-hotkey` crate. No permission is needed.
//! - Push-to-talk, Esc, and the key recorder go through our own listen-only
//!   keyboard hook (see [`crate::hook`]). It needs no permission either.
//!
//! `global-hotkey` makes a hidden window on the thread of the first
//! `register` call, and Windows posts `WM_HOTKEY` to that thread. So that
//! thread must run a message loop and must make every later call. The GPUI
//! main thread does both.

use crate::hook::HookController;
use crate::keymap;
use crossbeam_channel::{Receiver, Sender};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use parking_lot::Mutex;
use sayso_core::hotkey::Hotkey;
use sayso_platform::{HotkeyBindings, HotkeyConflict, HotkeyEvent, HotkeyRegistration, HotkeySource};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::ThreadId;

#[derive(Default)]
struct Chords {
    manager: Option<GlobalHotKeyManager>,
    /// The thread that made the manager and its window.
    thread: Option<ThreadId>,
    /// What is registered now, as `global-hotkey` and as Sayso sees it.
    registered: Vec<(HotKey, Hotkey)>,
}

pub struct WinHotkeys {
    chords: Mutex<Chords>,
    /// `global-hotkey` event id to the action it triggers.
    actions: Arc<Mutex<HashMap<u32, HotkeyEvent>>>,
    rx: Receiver<HotkeyEvent>,
    hook: HookController,
}

impl WinHotkeys {
    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let capturing = Arc::new(AtomicBool::new(false));
        let actions: Arc<Mutex<HashMap<u32, HotkeyEvent>>> = Arc::default();
        spawn_chord_forwarder(actions.clone(), tx.clone(), capturing.clone());
        let hook = HookController::new(tx, capturing);
        Self { chords: Mutex::new(Chords::default()), actions, rx, hook }
    }

    /// The chords Sayso holds now.
    pub fn registered(&self) -> Vec<Hotkey> {
        self.chords.lock().registered.iter().map(|(_, hotkey)| *hotkey).collect()
    }
}

impl Default for WinHotkeys {
    fn default() -> Self {
        Self::new()
    }
}

/// `global-hotkey` has one process-wide receiver. This thread turns its
/// presses into [`HotkeyEvent`]s. It ends when the receiving side is gone.
fn spawn_chord_forwarder(
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

/// Whether a hotkey needs the Fn key, which Windows never sees.
fn uses_fn(hotkey: &Hotkey) -> bool {
    match hotkey {
        Hotkey::Solo(solo) => keymap::solo_vk(*solo).is_none(),
        Hotkey::Chord { modifiers, .. } => modifiers.function,
    }
}

impl HotkeySource for WinHotkeys {
    fn register(&self, bindings: &HotkeyBindings) -> HotkeyRegistration {
        let mut failed: Vec<(Hotkey, String)> = Vec::new();

        // Chords through RegisterHotKey.
        let chords = [
            (bindings.toggle, HotkeyEvent::Toggle),
            (bindings.paste_last, HotkeyEvent::PasteLast),
            (bindings.cycle_style, HotkeyEvent::CycleStyle),
            (bindings.incognito, HotkeyEvent::ToggleIncognito),
        ];
        {
            let mut state = self.chords.lock();
            let here = std::thread::current().id();
            if state.thread.is_some_and(|t| t != here) {
                // The old chords stay as they are.
                let reason = "hotkeys must be registered on the thread that registered them first".to_string();
                failed.extend(chords.iter().filter_map(|(h, _)| h.map(|h| (h, reason.clone()))));
            } else {
                self.actions.lock().clear();
                if state.manager.is_none() {
                    match GlobalHotKeyManager::new() {
                        Ok(manager) => {
                            state.manager = Some(manager);
                            state.thread = Some(here);
                        }
                        Err(err) => log::error!("cannot create the hotkey manager: {err}"),
                    }
                }
                let old: Vec<HotKey> = std::mem::take(&mut state.registered).into_iter().map(|(h, _)| h).collect();
                if let Some(manager) = &state.manager {
                    let _ = manager.unregister_all(&old);
                }
                for (hotkey, action) in chords {
                    let Some(hotkey) = hotkey else { continue };
                    match register_chord(&mut state, &hotkey) {
                        Ok(registered) => {
                            self.actions.lock().insert(registered.id(), action);
                            state.registered.push((registered, hotkey));
                        }
                        Err(reason) => failed.push((hotkey, reason)),
                    }
                }
            }
        }

        // Push-to-talk and Esc through the hook.
        let mut ptt = bindings.push_to_talk;
        if let Some(hotkey) = ptt.filter(uses_fn) {
            failed.push((hotkey, keymap::FN_UNSUPPORTED.into()));
            ptt = None;
        }
        if let Err(reason) = self.hook.configure(ptt, bindings.single_escape, bindings.double_escape_window)
            && let Some(ptt) = ptt
        {
            failed.push((ptt, reason));
        }

        // Windows needs no grant for the keyboard hook.
        HotkeyRegistration { failed, needs_input_monitoring: false }
    }

    fn set_recording(&self, recording: bool) {
        if let Err(err) = self.hook.set_recording(recording) {
            log::warn!("Esc cancel is unavailable: {err}");
        }
    }

    fn events(&self) -> Receiver<HotkeyEvent> {
        self.rx.clone()
    }

    fn conflicts(&self, hotkey: &Hotkey) -> Vec<HotkeyConflict> {
        crate::conflicts::conflicts(hotkey, &self.registered())
    }

    /// Windows has no Secure Event Input. Password fields take synthetic keys.
    fn secure_input_holder(&self) -> Option<String> {
        None
    }

    fn capture_next(&self) -> Receiver<Hotkey> {
        self.hook.capture()
    }
}

fn register_chord(state: &mut Chords, hotkey: &Hotkey) -> Result<HotKey, String> {
    let mapped = keymap::to_global_hotkey(hotkey)?;
    if state.registered.iter().any(|(h, _)| *h == mapped) {
        return Err("another Sayso action already uses this shortcut".into());
    }
    let manager = state.manager.as_ref().ok_or("the hotkey manager is not available")?;
    manager.register(mapped).map_err(|e| match e {
        global_hotkey::Error::AlreadyRegistered(_) => "another app already uses this shortcut".to_string(),
        other => other.to_string(),
    })?;
    Ok(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn bindings(toggle: &str, ptt: Option<&str>) -> HotkeyBindings {
        HotkeyBindings {
            toggle: Some(toggle.parse().unwrap()),
            push_to_talk: ptt.map(|p| p.parse().unwrap()),
            paste_last: None,
            cycle_style: None,
            incognito: None,
            single_escape: false,
            double_escape_window: Duration::from_millis(400),
        }
    }

    #[test]
    fn fn_hotkeys_are_reported_as_failed() {
        assert!(uses_fn(&"fn".parse().unwrap()));
        assert!(uses_fn(&"fn+space".parse().unwrap()));
        assert!(!uses_fn(&"right_option".parse().unwrap()));
        assert!(!uses_fn(&Hotkey::toggle_default()));
    }

    #[test]
    fn registers_unusual_chords_and_releases_them() {
        // Run the whole test on one thread, which is what the manager needs.
        let hotkeys = WinHotkeys::new();
        let chord = "ctrl+opt+shift+f18";
        let registration = hotkeys.register(&bindings(chord, Some("fn")));
        assert_eq!(registration.failed, vec![("fn".parse().unwrap(), keymap::FN_UNSUPPORTED.to_string())]);
        assert!(!registration.needs_input_monitoring);
        assert_eq!(hotkeys.registered(), vec![chord.parse().unwrap()]);
        // Sayso's own chord is not reported as another app's.
        assert!(hotkeys.conflicts(&chord.parse().unwrap()).is_empty());

        // A second registration of the same chord by Sayso fails cleanly.
        let same = HotkeyBindings { paste_last: Some(chord.parse().unwrap()), ..bindings(chord, None) };
        let registration = hotkeys.register(&same);
        assert_eq!(registration.failed.len(), 1);
        assert!(registration.failed[0].1.contains("another Sayso action"));

        let none = HotkeyBindings { toggle: None, ..bindings(chord, None) };
        assert!(hotkeys.register(&none).failed.is_empty());
        assert!(hotkeys.registered().is_empty());
    }

    #[test]
    #[ignore = "sends a hotkey chord to a window of its own"]
    fn a_registered_chord_fires_while_the_thread_pumps_messages() {
        use crate::input::{Stroke, send};
        use crate::keymap::{VK_F1, VK_LCONTROL, VK_LMENU, VK_LSHIFT};
        use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage};

        let window = crate::test_window::TestWindow::open();
        assert!(window.wait_in_front(), "the test window could not come to the front; no keys were sent");
        let hotkeys = WinHotkeys::new();
        assert!(hotkeys.register(&bindings("ctrl+opt+shift+f18", None)).failed.is_empty());
        let events = hotkeys.events();

        let f18 = VK_F1 + 17;
        let keys = [VK_LCONTROL, VK_LMENU, VK_LSHIFT, f18];
        let mut strokes: Vec<Stroke> = keys.iter().map(|vk| Stroke::Key { vk: *vk, down: true }).collect();
        strokes.extend(keys.iter().rev().map(|vk| Stroke::Key { vk: *vk, down: false }));
        assert!(window.is_in_front());
        send(&strokes).unwrap();

        // WM_HOTKEY goes to the hidden window on this thread: pump until it arrives.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let event = loop {
            let mut msg = MSG::default();
            // SAFETY: a plain message pump on this thread.
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if let Ok(event) = events.try_recv() {
                break Some(event);
            }
            if std::time::Instant::now() > deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        hotkeys.register(&HotkeyBindings { toggle: None, ..bindings("ctrl+opt+shift+f18", None) });
        assert_eq!(event, Some(HotkeyEvent::Toggle));
    }

    #[test]
    fn a_solo_modifier_cannot_be_the_toggle() {
        let hotkeys = WinHotkeys::new();
        let registration = hotkeys.register(&bindings("right_option", None));
        assert_eq!(registration.failed.len(), 1);
        assert!(hotkeys.registered().is_empty());
    }

    #[test]
    fn register_on_another_thread_fails_instead_of_breaking() {
        let hk = WinHotkeys::new();
        hk.register(&HotkeyBindings { toggle: None, ..bindings("ctrl+opt+shift+f17", None) });
        assert_eq!(hk.chords.lock().thread, Some(std::thread::current().id()));
        // Pretend the manager was made on another thread.
        let other = std::thread::spawn(|| std::thread::current().id()).join().unwrap();
        hk.chords.lock().thread = Some(other);
        let registration = hk.register(&bindings("ctrl+opt+shift+f17", None));
        assert!(registration.failed[0].1.contains("thread"));
        assert!(hk.registered().is_empty());
        hk.chords.lock().thread = Some(std::thread::current().id());
    }
}
