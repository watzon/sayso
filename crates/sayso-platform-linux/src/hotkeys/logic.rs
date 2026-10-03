//! The decision logic behind the listen-only key stream. No I/O, so it is
//! unit tested with plain values.
//!
//! The evdev and XInput2 threads turn each key event into an evdev code and
//! a [`KeyAction`], and call [`KeyLogic::handle`]. The logic tracks the keys
//! that are down, and answers with hotkey events and, while the key recorder
//! is open, with the captured hotkey.

use super::keymap::{self, Chord, KEY_ESC};
use crate::esc::EscDetector;
use sayso_core::hotkey::{Hotkey, Key, Modifiers, SoloModifier};
use sayso_platform::HotkeyEvent;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Down,
    Up,
    /// Auto-repeat of a key that is down.
    Repeat,
}

/// What the backend should deliver for one key event.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub events: Vec<HotkeyEvent>,
    /// Set when the key recorder finished with this event.
    pub captured: Option<Hotkey>,
}

/// The push-to-talk binding as the logic sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ptt {
    Solo(u16),
    Chord(Chord),
}

#[derive(Debug)]
pub struct KeyLogic {
    /// evdev codes of the keys that are down now.
    pressed: Vec<u16>,
    ptt: Option<Ptt>,
    ptt_down: bool,
    /// Chords that this stream detects, for sessions without a chord backend.
    chords: Vec<(Chord, HotkeyEvent)>,
    recording: bool,
    esc: EscDetector,
    /// The key recorder is open.
    capturing: bool,
    /// A solo modifier went down alone and nothing else happened since.
    solo_candidate: Option<SoloModifier>,
}

impl Default for KeyLogic {
    fn default() -> Self {
        Self::new(EscDetector::new(false, Duration::from_millis(400)))
    }
}

impl KeyLogic {
    pub fn new(esc: EscDetector) -> Self {
        Self {
            pressed: Vec::new(),
            ptt: None,
            ptt_down: false,
            chords: Vec::new(),
            recording: false,
            esc,
            capturing: false,
            solo_candidate: None,
        }
    }

    /// True when the stream has any reason to run.
    pub fn is_needed(&self) -> bool {
        self.ptt.is_some() || !self.chords.is_empty() || self.recording || self.capturing
    }

    /// Set the push-to-talk binding. Returns a release event when a held
    /// push-to-talk is dropped by the change. A hotkey that Linux cannot use
    /// (Fn) sets no binding.
    pub fn set_ptt(&mut self, hotkey: Option<Hotkey>) -> Option<HotkeyEvent> {
        let released = self.release_held();
        self.ptt = hotkey.and_then(|h| match h {
            Hotkey::Solo(m) => keymap::solo_code(m).map(Ptt::Solo),
            Hotkey::Chord { .. } => keymap::chord(&h).ok().map(Ptt::Chord),
        });
        released
    }

    /// Set the chords that this stream turns into events (toggle, paste last,
    /// cycle style). The chord backend handles them when one exists.
    pub fn set_chords(&mut self, chords: Vec<(Chord, HotkeyEvent)>) {
        self.chords = chords;
    }

    pub fn configure_esc(&mut self, single: bool, window: Duration) {
        self.esc.configure(single, window);
    }

    pub fn set_recording(&mut self, recording: bool) {
        self.recording = recording;
        self.esc.reset();
    }

    pub fn is_capturing(&self) -> bool {
        self.capturing
    }

    pub fn set_capturing(&mut self, capturing: bool) {
        self.capturing = capturing;
        self.solo_candidate = None;
    }

    /// Release a held push-to-talk, for example when the stream stops.
    pub fn release_held(&mut self) -> Option<HotkeyEvent> {
        std::mem::take(&mut self.ptt_down).then_some(HotkeyEvent::PushToTalkUp)
    }

    /// Forget all keys that are down. Call it when a keyboard goes away or the
    /// stream restarts, because their key-up events will not arrive.
    pub fn reset_keys(&mut self) -> Option<HotkeyEvent> {
        self.pressed.clear();
        self.solo_candidate = None;
        self.release_held()
    }

    pub fn handle(&mut self, code: u16, action: KeyAction, now: Instant) -> Outcome {
        // A second key-down without a key-up is auto-repeat (XInput2 does not mark it).
        let action = match action {
            KeyAction::Down if self.pressed.contains(&code) => KeyAction::Repeat,
            other => other,
        };
        match action {
            KeyAction::Down => self.pressed.push(code),
            KeyAction::Up => self.pressed.retain(|c| *c != code),
            KeyAction::Repeat => {}
        }
        if self.capturing {
            return Outcome { events: Vec::new(), captured: self.capture(code, action) };
        }
        let mut out = Outcome::default();
        let mods = self.modifiers();
        match action {
            KeyAction::Repeat => {}
            KeyAction::Down => {
                match self.ptt {
                    Some(Ptt::Solo(solo)) if code == solo => self.set_ptt_down(true, &mut out.events),
                    Some(Ptt::Chord(c)) if code == c.code && mods == c.modifiers => {
                        self.set_ptt_down(true, &mut out.events)
                    }
                    // An extra modifier went down while the chord was held.
                    Some(Ptt::Chord(c)) if keymap::is_modifier(code) && mods != c.modifiers => {
                        self.set_ptt_down(false, &mut out.events)
                    }
                    _ => {}
                }
                for (chord, event) in &self.chords {
                    if code == chord.code && mods == chord.modifiers {
                        out.events.push(*event);
                    }
                }
                let plain = !(mods.control || mods.option || mods.command);
                if code == KEY_ESC && self.recording && plain && self.esc.press(now) {
                    out.events.push(HotkeyEvent::Cancel);
                }
            }
            KeyAction::Up => match self.ptt {
                Some(Ptt::Solo(solo)) if code == solo => self.set_ptt_down(false, &mut out.events),
                // The key, or one of the chord's modifiers, went up.
                Some(Ptt::Chord(c)) if code == c.code || (keymap::is_modifier(code) && mods != c.modifiers) => {
                    self.set_ptt_down(false, &mut out.events)
                }
                _ => {}
            },
        }
        out
    }

    fn modifiers(&self) -> Modifiers {
        keymap::modifiers_of(self.pressed.iter().copied())
    }

    fn set_ptt_down(&mut self, down: bool, events: &mut Vec<HotkeyEvent>) {
        if self.ptt_down == down {
            return;
        }
        self.ptt_down = down;
        events.push(if down { HotkeyEvent::PushToTalkDown } else { HotkeyEvent::PushToTalkUp });
    }

    /// Key recorder: a chord completes on key down, a solo modifier completes
    /// when it is released without any other key in between.
    fn capture(&mut self, code: u16, action: KeyAction) -> Option<Hotkey> {
        match action {
            // Modifiers auto-repeat too, so a repeat must not end a solo candidate.
            KeyAction::Repeat => None,
            KeyAction::Down if keymap::is_modifier(code) => {
                let alone = self.pressed.iter().all(|c| *c == code || !keymap::is_modifier(*c));
                self.solo_candidate = match keymap::solo_from_code(code) {
                    Some(solo) if alone && self.solo_candidate.is_none() => Some(solo),
                    _ => None,
                };
                None
            }
            KeyAction::Down => {
                self.solo_candidate = None;
                let key = keymap::key_from_code(code)?;
                let modifiers = self.modifiers();
                // A bare key is only a valid hotkey when it is an F key.
                if modifiers.is_empty() && !matches!(key, Key::F(_)) {
                    return None;
                }
                self.finish_capture(Hotkey::Chord { modifiers, key })
            }
            KeyAction::Up => match self.solo_candidate {
                Some(solo) if keymap::solo_code(solo) == Some(code) => self.finish_capture(Hotkey::Solo(solo)),
                _ => None,
            },
        }
    }

    fn finish_capture(&mut self, hotkey: Hotkey) -> Option<Hotkey> {
        self.capturing = false;
        self.solo_candidate = None;
        Some(hotkey)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkeys::keymap::*;
    use KeyAction::{Down, Repeat, Up};

    const KEY_D: u16 = 32;
    const KEY_A: u16 = 30;
    const KEY_F5: u16 = 63;

    fn logic() -> KeyLogic {
        KeyLogic::default()
    }

    fn with_ptt(s: &str) -> KeyLogic {
        let mut l = logic();
        l.set_ptt(Some(s.parse().unwrap()));
        l
    }

    /// Feed a sequence and collect every event.
    fn feed(l: &mut KeyLogic, keys: &[(u16, KeyAction)]) -> Vec<HotkeyEvent> {
        let now = Instant::now();
        keys.iter().flat_map(|(c, a)| l.handle(*c, *a, now).events).collect()
    }

    #[test]
    fn solo_push_to_talk_press_and_release() {
        let mut l = with_ptt("right_control");
        let now = Instant::now();
        assert_eq!(l.handle(KEY_RIGHTCTRL, Down, now).events, vec![HotkeyEvent::PushToTalkDown]);
        assert!(l.handle(KEY_RIGHTCTRL, Repeat, now).events.is_empty());
        // A second key-down without a key-up is a repeat too.
        assert!(l.handle(KEY_RIGHTCTRL, Down, now).events.is_empty());
        assert_eq!(l.handle(KEY_RIGHTCTRL, Up, now).events, vec![HotkeyEvent::PushToTalkUp]);
    }

    #[test]
    fn the_other_side_does_not_trigger_a_solo_modifier() {
        let mut l = with_ptt("right_option");
        assert!(feed(&mut l, &[(KEY_LEFTALT, Down), (KEY_LEFTALT, Up)]).is_empty());
        let mut l = with_ptt("left_option");
        assert!(feed(&mut l, &[(KEY_RIGHTALT, Down), (KEY_RIGHTALT, Up)]).is_empty());
    }

    #[test]
    fn solo_push_to_talk_works_with_other_modifiers_held() {
        let mut l = with_ptt("right_option");
        let events = feed(&mut l, &[(KEY_LEFTCTRL, Down), (KEY_RIGHTALT, Down), (KEY_RIGHTALT, Up), (KEY_LEFTCTRL, Up)]);
        assert_eq!(events, vec![HotkeyEvent::PushToTalkDown, HotkeyEvent::PushToTalkUp]);
    }

    #[test]
    fn fn_push_to_talk_sets_no_binding() {
        let l = with_ptt("fn");
        assert!(!l.is_needed());
    }

    #[test]
    fn chord_push_to_talk_follows_key_and_modifiers() {
        let mut l = with_ptt("ctrl+shift+d");
        let now = Instant::now();
        l.handle(KEY_LEFTCTRL, Down, now);
        // Only Ctrl is down: no match.
        assert!(l.handle(KEY_D, Down, now).events.is_empty());
        l.handle(KEY_D, Up, now);
        l.handle(KEY_RIGHTSHIFT, Down, now);
        assert_eq!(l.handle(KEY_D, Down, now).events, vec![HotkeyEvent::PushToTalkDown]);
        assert!(l.handle(KEY_D, Repeat, now).events.is_empty());
        assert_eq!(l.handle(KEY_D, Up, now).events, vec![HotkeyEvent::PushToTalkUp]);
    }

    #[test]
    fn chord_push_to_talk_releases_when_a_modifier_goes_first() {
        let mut l = with_ptt("ctrl+shift+d");
        let events = feed(
            &mut l,
            &[(KEY_LEFTCTRL, Down), (KEY_LEFTSHIFT, Down), (KEY_D, Down), (KEY_LEFTSHIFT, Up), (KEY_D, Up)],
        );
        assert_eq!(events, vec![HotkeyEvent::PushToTalkDown, HotkeyEvent::PushToTalkUp]);
    }

    #[test]
    fn chord_push_to_talk_needs_exact_modifiers() {
        let mut l = with_ptt("ctrl+d");
        let events = feed(&mut l, &[(KEY_LEFTCTRL, Down), (KEY_LEFTALT, Down), (KEY_D, Down)]);
        assert!(events.is_empty());
    }

    #[test]
    fn detected_chords_fire_once_per_press() {
        let mut l = logic();
        let toggle = chord(&"ctrl+opt+space".parse().unwrap()).unwrap();
        l.set_chords(vec![(toggle, HotkeyEvent::Toggle)]);
        assert!(l.is_needed());
        let events = feed(
            &mut l,
            &[
                (KEY_LEFTCTRL, Down),
                (KEY_LEFTALT, Down),
                (KEY_SPACE, Down),
                (KEY_SPACE, Repeat),
                (KEY_SPACE, Up),
                (KEY_SPACE, Down),
            ],
        );
        assert_eq!(events, vec![HotkeyEvent::Toggle, HotkeyEvent::Toggle]);
        // Without Alt the chord does not match.
        let events = feed(&mut l, &[(KEY_SPACE, Up), (KEY_LEFTALT, Up), (KEY_SPACE, Down)]);
        assert!(events.is_empty());
    }

    #[test]
    fn esc_is_ignored_when_not_recording() {
        let mut l = logic();
        assert!(feed(&mut l, &[(KEY_ESC, Down), (KEY_ESC, Up), (KEY_ESC, Down)]).is_empty());
    }

    #[test]
    fn double_esc_cancels_while_recording() {
        let mut l = logic();
        l.set_recording(true);
        let t0 = Instant::now();
        assert!(l.handle(KEY_ESC, Down, t0).events.is_empty());
        l.handle(KEY_ESC, Up, t0);
        let out = l.handle(KEY_ESC, Down, t0 + Duration::from_millis(180));
        assert_eq!(out.events, vec![HotkeyEvent::Cancel]);
    }

    #[test]
    fn esc_repeat_and_modified_esc_do_not_count() {
        let mut l = logic();
        l.set_recording(true);
        let t0 = Instant::now();
        l.handle(KEY_ESC, Down, t0);
        assert!(l.handle(KEY_ESC, Repeat, t0 + Duration::from_millis(50)).events.is_empty());
        assert!(l.handle(KEY_ESC, Down, t0 + Duration::from_millis(60)).events.is_empty());
        l.handle(KEY_ESC, Up, t0);
        l.handle(KEY_LEFTMETA, Down, t0);
        assert!(l.handle(KEY_ESC, Down, t0 + Duration::from_millis(70)).events.is_empty());
    }

    #[test]
    fn shift_esc_is_still_plain() {
        let mut l = logic();
        l.configure_esc(true, Duration::from_millis(400));
        l.set_recording(true);
        let events = feed(&mut l, &[(KEY_LEFTSHIFT, Down), (KEY_ESC, Down)]);
        assert_eq!(events, vec![HotkeyEvent::Cancel]);
    }

    #[test]
    fn single_esc_setting() {
        let mut l = logic();
        l.configure_esc(true, Duration::from_millis(400));
        l.set_recording(true);
        assert_eq!(l.handle(KEY_ESC, Down, Instant::now()).events, vec![HotkeyEvent::Cancel]);
    }

    #[test]
    fn stopping_recording_forgets_a_half_finished_double_esc() {
        let mut l = logic();
        let t0 = Instant::now();
        l.set_recording(true);
        l.handle(KEY_ESC, Down, t0);
        l.handle(KEY_ESC, Up, t0);
        l.set_recording(false);
        l.set_recording(true);
        assert!(l.handle(KEY_ESC, Down, t0 + Duration::from_millis(100)).events.is_empty());
    }

    #[test]
    fn changing_the_binding_releases_a_held_ptt() {
        let mut l = with_ptt("right_option");
        l.handle(KEY_RIGHTALT, Down, Instant::now());
        assert_eq!(l.set_ptt(None), Some(HotkeyEvent::PushToTalkUp));
        assert!(!l.is_needed());
    }

    #[test]
    fn reset_keys_forgets_modifiers_and_releases_once() {
        let mut l = with_ptt("right_option");
        l.set_recording(true);
        feed(&mut l, &[(KEY_LEFTCTRL, Down), (KEY_RIGHTALT, Down)]);
        assert_eq!(l.reset_keys(), Some(HotkeyEvent::PushToTalkUp));
        assert_eq!(l.reset_keys(), None);
        // Ctrl is forgotten, so Esc is plain again.
        l.configure_esc(true, Duration::from_millis(400));
        assert_eq!(feed(&mut l, &[(KEY_ESC, Down)]), vec![HotkeyEvent::Cancel]);
    }

    #[test]
    fn capture_returns_a_chord_on_key_down() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        assert_eq!(l.handle(KEY_LEFTALT, Down, now).captured, None);
        let out = l.handle(KEY_SPACE, Down, now);
        assert_eq!(out.captured, Some("opt+space".parse().unwrap()));
        assert!(!l.is_capturing(), "capture ends after one result");
        assert_eq!(l.handle(KEY_SPACE, Down, now).captured, None);
    }

    #[test]
    fn capture_ignores_bare_letters_and_esc_but_takes_f_keys() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        assert_eq!(l.handle(KEY_A, Down, now).captured, None);
        assert_eq!(l.handle(KEY_ESC, Down, now).captured, None);
        let out = l.handle(KEY_F5, Down, now);
        assert_eq!(out.captured, Some(Hotkey::Chord { modifiers: Modifiers::default(), key: Key::F(5) }));
    }

    #[test]
    fn capture_returns_a_solo_modifier_after_release() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        assert_eq!(l.handle(KEY_RIGHTCTRL, Down, now).captured, None);
        // Modifiers auto-repeat while held.
        assert_eq!(l.handle(KEY_RIGHTCTRL, Repeat, now).captured, None);
        let out = l.handle(KEY_RIGHTCTRL, Up, now);
        assert_eq!(out.captured, Some(Hotkey::Solo(SoloModifier::RightControl)));
    }

    #[test]
    fn capture_does_not_take_a_modifier_that_became_part_of_a_chord() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        l.handle(KEY_RIGHTALT, Down, now);
        let chord = l.handle(KEY_SPACE, Down, now);
        assert!(matches!(chord.captured, Some(Hotkey::Chord { .. })));
        l.set_capturing(true);
        assert_eq!(l.handle(KEY_RIGHTALT, Up, now).captured, None);
    }

    #[test]
    fn capture_ignores_a_solo_pressed_while_another_modifier_is_held() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        l.handle(KEY_LEFTMETA, Down, now);
        l.handle(KEY_RIGHTALT, Down, now);
        assert_eq!(l.handle(KEY_RIGHTALT, Up, now).captured, None);
        // A second modifier after the solo one also spoils it.
        l.handle(KEY_LEFTMETA, Up, now);
        l.handle(KEY_RIGHTSHIFT, Down, now);
        l.handle(KEY_LEFTCTRL, Down, now);
        assert_eq!(l.handle(KEY_RIGHTSHIFT, Up, now).captured, None);
    }

    #[test]
    fn capture_ignores_left_ctrl_alone() {
        let mut l = logic();
        l.set_capturing(true);
        assert!(feed(&mut l, &[(KEY_LEFTCTRL, Down), (KEY_LEFTCTRL, Up)]).is_empty());
        assert!(l.is_capturing());
    }

    #[test]
    fn capture_suppresses_ptt_and_cancel_events() {
        let mut l = with_ptt("right_option");
        l.configure_esc(true, Duration::from_millis(400));
        l.set_recording(true);
        l.set_capturing(true);
        assert!(feed(&mut l, &[(KEY_RIGHTALT, Down), (KEY_ESC, Down)]).is_empty());
    }

    #[test]
    fn modifiers_held_before_capture_still_count() {
        let mut l = logic();
        let now = Instant::now();
        l.handle(KEY_LEFTCTRL, Down, now);
        l.set_capturing(true);
        let out = l.handle(KEY_D, Down, now);
        assert_eq!(out.captured, Some("ctrl+d".parse().unwrap()));
    }
}
