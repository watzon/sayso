//! [`HotkeySource`] for X11 and Wayland.
//!
//! Two kinds of backend work together, and the session decides which ones run:
//!
//! | Job | X11 session | Wayland session | No display |
//! |---|---|---|---|
//! | Chords (toggle, paste last, cycle style, chord push-to-talk) | `XGrabKey` | GlobalShortcuts portal, else evdev, else GNOME custom shortcuts | evdev |
//! | Listen-only stream (solo push-to-talk, Esc while recording, key recorder) | XInput2 raw keys, else evdev | evdev | evdev |
//!
//! - A chord backend takes the key, so the focused app does not get it. When
//!   chords fall back to the evdev stream, the key also reaches the focused app.
//! - The evdev stream needs read access to `/dev/input` (the `input` group).
//!   That is the Linux form of Input Monitoring. On X11, XInput2 replaces it.
//! - All decisions on raw keys happen in [`logic::KeyLogic`], which has no I/O.
//!
//! Linux has no Fn key that software can see, so hotkeys with Fn fail with a reason.

mod conflicts;
mod evdev;
mod gnome;
mod keymap;
mod listen;
#[cfg(test)]
mod live_tests;
mod logic;
mod portal;
mod x11;

use crate::session::{Desktop, Session, SessionKind};
use crossbeam_channel::{Receiver, Sender, unbounded};
use keymap::Chord;
use listen::Listen;
use parking_lot::Mutex;
use sayso_core::hotkey::Hotkey;
use sayso_platform::{HotkeyBindings, HotkeyConflict, HotkeyEvent, HotkeyRegistration, HotkeySource, PermissionState};
use std::sync::Arc;

/// What a chord does when a chord backend sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordAction {
    /// One event on key press (toggle, paste last, cycle style, a style hotkey).
    Press(HotkeyEvent),
    /// Push-to-talk: down on press, up on release.
    PushToTalk,
}

/// Where chords go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChordBackend {
    X11Grab,
    Portal,
    /// The listen-only stream detects them. They also reach the focused app.
    Listen,
    /// GNOME runs `sayso --toggle` and the other commands (GNOME before 48).
    GnomeCustom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListenBackend {
    Evdev,
    Xi2,
}

#[derive(Default)]
struct Backends {
    chords: Option<ChordBackend>,
    x11: Option<x11::X11Keys>,
    /// The X server could not be reached, so do not try again.
    x11_failed: bool,
    portal: Option<portal::Portal>,
    /// The bindings sent to the portal last, so an unchanged set does not
    /// open a new portal session.
    portal_bound: Option<Vec<portal::Binding>>,
    evdev: Option<evdev::EvdevReader>,
    listening: Option<ListenBackend>,
}

pub struct LinuxHotkeys {
    session: &'static Session,
    tx: Sender<HotkeyEvent>,
    rx: Receiver<HotkeyEvent>,
    listen: Arc<Listen>,
    backends: Mutex<Backends>,
    conflicts: conflicts::Conflicts,
}

impl LinuxHotkeys {
    pub fn new(session: &'static Session) -> Self {
        let (tx, rx) = unbounded();
        Self {
            session,
            listen: Arc::new(Listen::new(tx.clone())),
            tx,
            rx,
            backends: Mutex::default(),
            conflicts: conflicts::Conflicts::new(session.desktop.clone()),
        }
    }

    /// A sender into the same event stream, for hotkeys that arrive another
    /// way (the `sayso --toggle` command).
    pub fn sender(&self) -> Sender<HotkeyEvent> {
        self.tx.clone()
    }

    /// Start the listen-only stream when nothing runs it yet.
    fn ensure_listening(&self, b: &mut Backends) -> Result<ListenBackend, String> {
        if let Some(running) = b.listening {
            return Ok(running);
        }
        // XInput2 observes every master keyboard and pointer without input-group
        // access. Prefer it even when evdev opens a subset: that subset cannot
        // tell us whether an unreadable device is the user's keyboard or mouse.
        if self.session.kind == SessionKind::X11 {
            match self.x11(b).and_then(|x11| x11.listen_raw(true)) {
                Ok(()) => {
                    self.listen.logic.lock().set_pointer_available(true);
                    b.listening = Some(ListenBackend::Xi2);
                    log::info!("the listen-only key stream uses XInput2");
                    return Ok(ListenBackend::Xi2);
                }
                Err(e) => log::warn!("XInput2 unavailable: {e}; trying evdev"),
            }
        }
        let evdev_error = match evdev::EvdevReader::start(self.listen.clone()) {
            Ok(reader) => {
                b.evdev = Some(reader);
                b.listening = Some(ListenBackend::Evdev);
                log::info!("the listen-only key stream uses evdev");
                return Ok(ListenBackend::Evdev);
            }
            Err(e) => e,
        };
        Err(evdev_error)
    }

    /// The X11 connection, made on first use.
    fn x11<'a>(&self, b: &'a mut Backends) -> Result<&'a x11::X11Keys, String> {
        if b.x11.is_none() {
            if b.x11_failed {
                return Err("the X server is not reachable".into());
            }
            let display = self.session.x11_display.as_ref().and_then(|d| d.to_str());
            match x11::X11Keys::connect(display, self.listen.clone()) {
                Ok(keys) => b.x11 = Some(keys),
                Err(e) => {
                    b.x11_failed = true;
                    return Err(e);
                }
            }
        }
        b.x11.as_ref().ok_or_else(|| "the X server is not reachable".into())
    }

    /// Choose the chord backend once, on the first registration.
    fn chord_backend(&self, b: &mut Backends) -> ChordBackend {
        if let Some(chosen) = b.chords {
            return chosen;
        }
        let chosen = match self.session.kind {
            SessionKind::X11 => match self.x11(b) {
                Ok(_) => ChordBackend::X11Grab,
                Err(e) => {
                    log::warn!("{e}; chords use the evdev key stream");
                    ChordBackend::Listen
                }
            },
            SessionKind::Wayland => match portal::Portal::start(self.listen.clone()) {
                Ok(p) => {
                    b.portal = Some(p);
                    ChordBackend::Portal
                }
                Err(e) if !evdev::devices_readable() && self.session.desktop == Desktop::Gnome && gnome::available() => {
                    log::warn!("{e}; chords become GNOME custom shortcuts");
                    ChordBackend::GnomeCustom
                }
                Err(e) => {
                    log::warn!("{e}; chords use the evdev key stream and also reach the focused app");
                    ChordBackend::Listen
                }
            },
            SessionKind::None => ChordBackend::Listen,
        };
        log::info!("chords use {chosen:?}");
        // Shortcuts that an earlier start added would now fire twice.
        if chosen != ChordBackend::GnomeCustom && self.session.desktop == Desktop::Gnome {
            gnome::clear();
        }
        b.chords = Some(chosen);
        chosen
    }
}

/// The chords and the push-to-talk binding after validation.
#[derive(Debug, Default, PartialEq)]
struct Plan {
    chords: Vec<(Hotkey, Chord, ChordAction)>,
    /// A modifier-only push-to-talk binding. Only the listen-only stream sees it.
    solo_ptt: Option<Hotkey>,
    modifier_toggle: Option<Hotkey>,
    failed: Vec<(Hotkey, String)>,
}

fn plan(bindings: &HotkeyBindings) -> Plan {
    let mut plan = Plan::default();
    let add_chord = |plan: &mut Plan, hotkey: Hotkey, action: ChordAction| match keymap::chord(&hotkey) {
        Ok(chord) if plan.chords.iter().any(|(_, c, _)| *c == chord) => {
            plan.failed.push((hotkey, "another Sayso action already uses this shortcut".into()))
        }
        Ok(chord) => plan.chords.push((hotkey, chord, action)),
        Err(reason) => plan.failed.push((hotkey, reason)),
    };
    let toggle = bindings.toggle.map(|hotkey| (hotkey, HotkeyEvent::Toggle));
    for (hotkey, event) in toggle.into_iter().chain(bindings.press_chords()) {
        if hotkey.modifier_keys().is_some() && event == HotkeyEvent::Toggle {
            match keymap::check_push_to_talk(&hotkey) {
                Ok(()) => plan.modifier_toggle = Some(hotkey),
                Err(reason) => plan.failed.push((hotkey, reason)),
            }
        } else {
            add_chord(&mut plan, hotkey, ChordAction::Press(event));
        }
    }
    if let Some(ptt) = bindings.push_to_talk {
        match (keymap::check_push_to_talk(&ptt), ptt) {
            (Err(reason), _) => plan.failed.push((ptt, reason)),
            (Ok(()), Hotkey::Solo(_) | Hotkey::ModifierOnly(_)) => plan.solo_ptt = Some(ptt),
            (Ok(()), Hotkey::Chord { .. }) => add_chord(&mut plan, ptt, ChordAction::PushToTalk),
        }
    }
    plan
}

/// The id of a style in a portal id or a GNOME path: lower case letters,
/// digits, and `-`. Any other character becomes `-`.
fn style_slug(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c.to_ascii_lowercase() } else { '-' }).collect()
}

/// The raw id of the style that `Style(i)` points at.
fn style_id(styles: &[(String, Hotkey)], index: u16) -> Option<&str> {
    styles.get(usize::from(index)).map(|(id, _)| id.as_str())
}

fn portal_binding(chord: &Chord, action: ChordAction, styles: &[(String, Hotkey)]) -> Option<portal::Binding> {
    let (id, description) = match action {
        ChordAction::Press(HotkeyEvent::Toggle) => ("toggle".to_string(), "Start or stop a dictation".to_string()),
        ChordAction::Press(HotkeyEvent::PasteLast) => ("paste-last".into(), "Paste the last dictation".into()),
        ChordAction::Press(HotkeyEvent::CycleStyle) => ("cycle-style".into(), "Switch to the next style".into()),
        ChordAction::Press(HotkeyEvent::ToggleIncognito) => ("incognito".into(), "Turn incognito on or off".into()),
        ChordAction::Press(HotkeyEvent::Style(i)) => {
            let raw = style_id(styles, i)?;
            (format!("style-{}", style_slug(raw)), format!("Start one dictation in the {raw} style"))
        }
        ChordAction::PushToTalk => ("push-to-talk".into(), "Record while you hold the keys".into()),
        ChordAction::Press(_) => return None,
    };
    Some(portal::Binding { id, description, trigger: keymap::portal_trigger(chord)?, action })
}

impl HotkeySource for LinuxHotkeys {
    fn register(&self, bindings: &HotkeyBindings) -> HotkeyRegistration {
        let Plan { chords, solo_ptt, modifier_toggle, mut failed } = plan(bindings);
        let mut b = self.backends.lock();
        let backend = if chords.is_empty() && b.chords.is_none() { ChordBackend::Listen } else { self.chord_backend(&mut b) };

        // Hotkeys that only the listen-only stream can see.
        let mut listen_ptt = solo_ptt;
        let mut listen_chords = Vec::new();
        let mut listen_hotkeys: Vec<Hotkey> = solo_ptt.into_iter().chain(modifier_toggle).collect();
        match backend {
            ChordBackend::X11Grab => match self.x11(&mut b) {
                Ok(x11) => failed.extend(x11.set_grabs(&chords)),
                Err(e) => failed.extend(chords.iter().map(|(h, _, _)| (*h, e.clone()))),
            },
            ChordBackend::Portal => {
                let mut wanted: Vec<portal::Binding> = Vec::new();
                for (hotkey, chord, action) in &chords {
                    let Some(binding) = portal_binding(chord, *action, &bindings.styles) else { continue };
                    // Two style ids can give one portal id. The portal keeps one shortcut for it.
                    if wanted.iter().any(|w| w.id == binding.id) {
                        failed.push((*hotkey, format!("another style already uses the portal shortcut {}", binding.id)));
                    } else {
                        wanted.push(binding);
                    }
                }
                if b.portal_bound.as_ref() != Some(&wanted) {
                    if let Some(p) = &b.portal {
                        p.bind(wanted.clone());
                    }
                    b.portal_bound = Some(wanted);
                }
            }
            ChordBackend::GnomeCustom => {
                let mut shortcuts: Vec<gnome::Shortcut> = Vec::new();
                for (hotkey, _, action) in &chords {
                    let (id, name, flag) = match action {
                        ChordAction::Press(HotkeyEvent::Toggle) => ("toggle".to_string(), "Start or stop dictation".to_string(), "toggle".to_string()),
                        ChordAction::Press(HotkeyEvent::PasteLast) => ("paste-last".into(), "Paste the last text".into(), "paste-last".into()),
                        ChordAction::Press(HotkeyEvent::CycleStyle) => ("cycle-style".into(), "Next style".into(), "cycle-style".into()),
                        ChordAction::Press(HotkeyEvent::ToggleIncognito) => ("incognito".into(), "Incognito".into(), "incognito".into()),
                        ChordAction::Press(HotkeyEvent::Style(i)) => {
                            let Some(raw) = style_id(&bindings.styles, *i) else { continue };
                            let id = format!("style-{}", style_slug(raw));
                            if shortcuts.iter().any(|s| s.id == id) {
                                failed.push((*hotkey, format!("another style already uses the GNOME shortcut {id}")));
                                continue;
                            }
                            (id, format!("Style: {raw}"), format!("style={raw}"))
                        }
                        ChordAction::Press(_) => continue,
                        ChordAction::PushToTalk => {
                            failed.push((*hotkey, "this version of GNOME cannot tell Sayso when you release a key. Use a toggle key, or join the input group".into()));
                            continue;
                        }
                    };
                    match gnome::accelerator(hotkey) {
                        Some(binding) => shortcuts.push(gnome::Shortcut { id, name, flag, binding }),
                        None => failed.push((*hotkey, "GNOME cannot bind a modifier alone".into())),
                    }
                }
                if let Err(e) = gnome::apply(&shortcuts) {
                    log::warn!("cannot add the GNOME shortcuts: {e}");
                    let added: Vec<String> = shortcuts.iter().map(|s| s.binding.clone()).collect();
                    for (hotkey, _, _) in &chords {
                        if gnome::accelerator(hotkey).is_some_and(|a| added.contains(&a)) {
                            failed.push((*hotkey, e.clone()));
                        }
                    }
                }
            }
            ChordBackend::Listen => {
                for (hotkey, chord, action) in &chords {
                    match action {
                        ChordAction::PushToTalk => listen_ptt = Some(*hotkey),
                        ChordAction::Press(event) => listen_chords.push((*chord, *event)),
                    }
                    listen_hotkeys.push(*hotkey);
                }
            }
        }

        let (released, needed) = {
            let mut logic = self.listen.logic.lock();
            logic.configure_esc(bindings.single_escape, bindings.double_escape_window);
            logic.set_chords(listen_chords);
            logic.set_toggle_modifiers(modifier_toggle.and_then(Hotkey::modifier_keys));
            (logic.set_ptt(listen_ptt), logic.is_needed())
        };
        if let Some(event) = released {
            self.listen.send(event);
        }
        let mut needs_input_monitoring = false;
        if needed
            && !listen_hotkeys.is_empty()
            && let Err(reason) = self.ensure_listening(&mut b)
        {
            log::warn!("the listen-only key stream cannot start: {reason}");
            needs_input_monitoring = true;
            failed.extend(listen_hotkeys.into_iter().map(|h| (h, reason.clone())));
        }
        HotkeyRegistration { failed, needs_input_monitoring }
    }

    fn set_recording(&self, recording: bool) {
        let needed = {
            let mut logic = self.listen.logic.lock();
            logic.set_recording(recording);
            logic.is_needed()
        };
        if recording
            && needed
            && let Err(err) = self.ensure_listening(&mut self.backends.lock())
        {
            log::warn!("Esc cancel is unavailable: {err}");
        }
    }

    fn events(&self) -> Receiver<HotkeyEvent> {
        self.rx.clone()
    }

    fn conflicts(&self, hotkey: &Hotkey) -> Vec<HotkeyConflict> {
        self.conflicts.conflicts(hotkey)
    }

    fn secure_input_holder(&self) -> Option<String> {
        None
    }

    fn capture_next(&self) -> Receiver<Hotkey> {
        let rx = self.listen.open_capture();
        if let Err(err) = self.ensure_listening(&mut self.backends.lock()) {
            log::warn!("the key recorder cannot listen: {err}");
            self.listen.cancel_capture();
        }
        rx
    }
}

/// Whether Sayso can watch the keyboard without grabbing it: push-to-talk,
/// double Esc, and the key recorder. The Linux form of Input Monitoring.
///
/// Granted when the user can read `/dev/input`, or in an X11 session (XInput2).
/// Denied in other sessions, where only membership in the `input` group helps.
pub fn input_monitoring_state(session: &Session) -> PermissionState {
    if evdev::devices_readable() || session.kind == SessionKind::X11 {
        PermissionState::Granted
    } else {
        PermissionState::Denied
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn bindings(toggle: Option<&str>, ptt: Option<&str>) -> HotkeyBindings {
        HotkeyBindings {
            toggle: toggle.map(|s| s.parse().unwrap()),
            push_to_talk: ptt.map(|s| s.parse().unwrap()),
            paste_last: None,
            cycle_style: None,
            incognito: None,
            styles: Vec::new(),
            enter_sends: false,
            take_escape: false,
            single_escape: false,
            double_escape_window: Duration::from_millis(400),
        }
    }

    #[test]
    fn plan_splits_chords_and_solo_modifiers() {
        let p = plan(&bindings(Some("ctrl+opt+space"), Some("right_control")));
        assert_eq!(p.chords.len(), 1);
        assert_eq!(p.chords[0].2, ChordAction::Press(HotkeyEvent::Toggle));
        assert_eq!(p.solo_ptt, Some("right_control".parse().unwrap()));
        assert!(p.failed.is_empty());

        let p = plan(&bindings(None, Some("ctrl+shift+d")));
        assert_eq!(p.chords[0].2, ChordAction::PushToTalk);
        assert_eq!(p.solo_ptt, None);
    }

    #[test]
    fn plan_accepts_solo_toggles_and_rejects_fn_and_duplicates() {
        let p = plan(&bindings(Some("right_option"), Some("fn")));
        assert_eq!(p.failed.len(), 1);
        assert_eq!(p.modifier_toggle, Some("right_option".parse().unwrap()));
        assert!(p.chords.is_empty() && p.solo_ptt.is_none());

        let mut b = bindings(Some("ctrl+opt+space"), Some("ctrl+opt+space"));
        b.paste_last = Some("fn+v".parse().unwrap());
        let p = plan(&b);
        assert_eq!(p.chords.len(), 1);
        assert_eq!(p.failed.len(), 2);
        assert!(p.failed[1].1.contains("another Sayso action"));
    }

    #[test]
    fn portal_bindings_have_stable_ids_and_spec_triggers() {
        let p = plan(&bindings(Some("ctrl+opt+space"), Some("cmd+shift+d")));
        let bound: Vec<_> = p.chords.iter().filter_map(|(_, c, a)| portal_binding(c, *a, &[])).collect();
        assert_eq!(bound[0].id, "toggle");
        assert_eq!(bound[0].trigger, "CTRL+ALT+space");
        assert_eq!(bound[1].id, "push-to-talk");
        assert_eq!(bound[1].trigger, "SHIFT+LOGO+d");
    }

    fn with_styles(toggle: Option<&str>, styles: &[(&str, &str)]) -> HotkeyBindings {
        let mut b = bindings(toggle, None);
        b.styles = styles.iter().map(|(id, key)| (id.to_string(), key.parse().unwrap())).collect();
        b
    }

    #[test]
    fn plan_sends_style_events_and_refuses_a_shared_chord() {
        let p = plan(&with_styles(Some("ctrl+opt+space"), &[("email", "ctrl+opt+e")]));
        assert_eq!(p.chords.len(), 2);
        assert_eq!(p.chords[1].2, ChordAction::Press(HotkeyEvent::Style(0)));
        assert!(p.failed.is_empty());

        let p = plan(&with_styles(Some("ctrl+opt+space"), &[("email", "ctrl+opt+space")]));
        assert_eq!(p.chords.len(), 1);
        assert_eq!(p.failed.len(), 1);
        assert!(p.failed[0].1.contains("another Sayso action"));
    }

    #[test]
    fn style_portal_ids_are_stable_and_sanitized() {
        let b = with_styles(None, &[("email", "ctrl+opt+e"), ("My Style!", "ctrl+opt+m")]);
        let p = plan(&b);
        let bound: Vec<_> = p.chords.iter().filter_map(|(_, c, a)| portal_binding(c, *a, &b.styles)).collect();
        assert_eq!(bound[0].id, "style-email");
        assert_eq!(bound[0].description, "Start one dictation in the email style");
        assert_eq!(bound[1].id, "style-my-style-");
        assert_eq!(bound[1].description, "Start one dictation in the My Style! style");
        // The id does not depend on the other styles.
        let b = with_styles(None, &[("My Style!", "ctrl+opt+m")]);
        let (_, chord, action) = &plan(&b).chords[0];
        assert_eq!(portal_binding(chord, *action, &b.styles).unwrap().id, "style-my-style-");
    }
}
