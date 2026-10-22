//! The decision logic behind the listen-only event tap. No FFI, so it is unit
//! tested with plain values.
//!
//! The tap thread turns each CGEvent into a [`RawEvent`] and calls
//! [`TapLogic::handle`]. The logic answers with hotkey events and, while the
//! key recorder is open, with the captured hotkey. The active tap calls
//! [`TapLogic::active_esc`] for Esc while Sayso takes Esc.

use crate::esc::{EscDetector, EscTake};
use crate::keymap::{
    self, KC_ESCAPE, modifiers_from_cg_flags, solo_from_keycode, solo_is_down,
};
use crate::ffi::{FLAG_ALT, FLAG_COMMAND, FLAG_CONTROL};
use sayso_core::hotkey::{Hotkey, Key, ModifierCapture, ModifierSet, Modifiers, SoloModifier};
use sayso_platform::HotkeyEvent;
use std::time::Instant;

/// One keyboard or pointer event from the tap, reduced to the fields we use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawEvent {
    KeyDown { code: u16, flags: u64, repeat: bool },
    KeyUp { code: u16, flags: u64 },
    FlagsChanged { code: u16, flags: u64, secure_input: bool },
    /// A mouse button went down or the wheel scrolled.
    PointerActivity,
}

/// What the tap should do for one raw event.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub events: Vec<HotkeyEvent>,
    /// Set when the key recorder finished with this event.
    pub captured: Option<Hotkey>,
}

/// The push-to-talk binding as the tap sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ptt {
    Solo(SoloModifier),
    ModifierOnly(ModifierSet),
    Chord { keycode: u16, modifiers: Modifiers },
}

#[derive(Debug)]
pub struct TapLogic {
    ptt: Option<Ptt>,
    toggle_modifiers: Option<ModifierSet>,
    ptt_down: bool,
    recording: bool,
    esc: EscDetector,
    /// The "take Esc" setting, and the Esc that the active tap took.
    take: EscTake,
    /// The active tap runs and sees Esc first. With `take` on, it sends the
    /// cancel, so the listen-only tap leaves Esc alone.
    active_tap: bool,
    /// The key recorder is open.
    capturing: bool,
    /// Modifiers pressed without a regular key in between.
    modifier_capture: ModifierCapture,
    pressed_keys: Vec<u16>,
}

impl TapLogic {
    pub fn new(esc: EscDetector) -> Self {
        Self {
            ptt: None,
            toggle_modifiers: None,
            ptt_down: false,
            recording: false,
            esc,
            take: EscTake::default(),
            active_tap: false,
            capturing: false,
            modifier_capture: ModifierCapture::default(),
            pressed_keys: Vec::new(),
        }
    }

    /// True when the tap has any reason to run.
    pub fn is_needed(&self) -> bool {
        self.ptt.is_some() || self.toggle_modifiers.is_some() || self.recording || self.capturing
    }

    pub fn set_toggle_modifiers(&mut self, keys: Option<ModifierSet>) {
        self.toggle_modifiers = keys;
        self.modifier_capture.clear();
    }

    /// Set the push-to-talk binding. Returns a release event when a held
    /// push-to-talk is dropped by the change.
    pub fn set_ptt(&mut self, hotkey: Option<Hotkey>) -> Option<HotkeyEvent> {
        let released = self.release_held();
        self.ptt = hotkey.and_then(|h| match h {
            Hotkey::Solo(m) => Some(Ptt::Solo(m)),
            Hotkey::ModifierOnly(keys) => Some(Ptt::ModifierOnly(keys)),
            Hotkey::Chord { modifiers, key } => keymap::keycode(key).map(|keycode| Ptt::Chord { keycode, modifiers }),
        });
        released
    }

    pub fn configure_esc(&mut self, single: bool, window: std::time::Duration) {
        self.esc.configure(single, window);
    }

    pub fn set_take_escape(&mut self, on: bool) {
        self.take.set_on(on);
    }

    pub fn takes_escape(&self) -> bool {
        self.take.is_on()
    }

    /// Whether the active tap runs.
    pub fn set_active_tap(&mut self, running: bool) {
        self.active_tap = running;
    }

    pub fn set_recording(&mut self, recording: bool) {
        self.recording = recording;
        self.esc.reset();
        self.take.set_recording(recording);
    }

    /// The active tap owns Esc: it takes the key and sends the cancel.
    fn active_owns_esc(&self) -> bool {
        self.active_tap && self.take.is_on()
    }

    /// One Esc event in the active tap. Returns the cancel, if this press
    /// completes one, and true when the app must not get the event.
    pub fn active_esc(&mut self, down: bool, repeat: bool, flags: u64, now: Instant) -> (Option<HotkeyEvent>, bool) {
        if self.capturing || !self.active_owns_esc() {
            return (None, false);
        }
        let plain = is_plain(flags);
        let swallow = self.take.key(down, repeat, plain);
        let cancel = down && !repeat && self.recording && plain && self.esc.press(now);
        (cancel.then_some(HotkeyEvent::Cancel), swallow)
    }

    pub fn set_capturing(&mut self, capturing: bool) {
        self.capturing = capturing;
        self.modifier_capture.clear();
    }

    /// The system disabled the tap, so key-up events may have been missed.
    pub fn release_held(&mut self) -> Option<HotkeyEvent> {
        self.pressed_keys.clear();
        self.modifier_capture.clear();
        std::mem::take(&mut self.ptt_down).then_some(HotkeyEvent::PushToTalkUp)
    }

    /// Reconcile releases hidden by Secure Event Input with current HID state.
    /// Keep genuinely held keys: clearing everything would allow a modifier tap
    /// begun while a regular key is still held to toggle.
    pub fn reconcile_pressed_keys(&mut self, mut is_down: impl FnMut(u16) -> bool) {
        self.pressed_keys.retain(|code| is_down(*code));
    }

    pub fn handle(&mut self, event: RawEvent, now: Instant) -> Outcome {
        match event {
            RawEvent::KeyDown { code, .. } if !self.pressed_keys.contains(&code) => self.pressed_keys.push(code),
            RawEvent::KeyUp { code, .. } => self.pressed_keys.retain(|held| *held != code),
            _ => {}
        }
        if self.capturing {
            return Outcome { events: Vec::new(), captured: self.capture(event) };
        }
        let mut out = Outcome::default();
        match event {
            RawEvent::FlagsChanged { code, flags, secure_input } => {
                let held = keymap::modifier_keys(flags);
                if secure_input || !self.pressed_keys.is_empty() {
                    // The gesture must contain only modifiers, with no hidden keys.
                    self.modifier_capture.clear();
                } else if let Some(keys) = self.toggle_modifiers {
                    if let Some(modifier) = solo_from_keycode(code) {
                        if solo_is_down(modifier, flags) {
                            self.modifier_capture.press_tap(held, now);
                        } else if self.modifier_capture.release_tap(modifier, now).and_then(Hotkey::modifier_keys) == Some(keys) {
                            out.events.push(HotkeyEvent::Toggle);
                        }
                    } else {
                        self.modifier_capture.clear();
                    }
                }
                match self.ptt {
                    Some(Ptt::ModifierOnly(keys)) => {
                        self.set_ptt_down(held == keys, &mut out.events);
                    }
                    Some(Ptt::Solo(m)) if keymap::solo_spec(m).keycode == code => {
                        self.set_ptt_down(solo_is_down(m, flags), &mut out.events);
                    }
                    // The user let go of a modifier before the key.
                    Some(Ptt::Chord { modifiers, .. })
                        if self.ptt_down && modifiers_from_cg_flags(flags) != required_modifiers(modifiers) =>
                    {
                        self.set_ptt_down(false, &mut out.events);
                    }
                    _ => {}
                }
            }
            RawEvent::KeyDown { code, flags, repeat } => {
                self.modifier_capture.clear();
                if repeat {
                    return out;
                }
                if let Some(Ptt::Chord { keycode, modifiers }) = self.ptt
                    && code == keycode
                    && modifiers_from_cg_flags(flags) == required_modifiers(modifiers)
                    && (!modifiers.function || flags & crate::ffi::FLAG_SECONDARY_FN != 0)
                {
                    self.set_ptt_down(true, &mut out.events);
                }
                if code == KC_ESCAPE
                    && self.recording
                    && is_plain(flags)
                    && !self.active_owns_esc()
                    && self.esc.press(now)
                {
                    out.events.push(HotkeyEvent::Cancel);
                }
            }
            RawEvent::PointerActivity => self.modifier_capture.clear(),
            RawEvent::KeyUp { code, .. } => {
                if let Some(Ptt::Chord { keycode, .. }) = self.ptt
                    && code == keycode
                {
                    self.set_ptt_down(false, &mut out.events);
                }
            }
        }
        out
    }

    fn set_ptt_down(&mut self, down: bool, events: &mut Vec<HotkeyEvent>) {
        if self.ptt_down == down {
            return;
        }
        self.ptt_down = down;
        events.push(if down { HotkeyEvent::PushToTalkDown } else { HotkeyEvent::PushToTalkUp });
    }

    /// Chords complete on key down. Modifier-only bindings complete on the
    /// first release, so a following regular key can still complete a chord.
    fn capture(&mut self, event: RawEvent) -> Option<Hotkey> {
        let done = |this: &mut Self, hotkey| {
            this.capturing = false;
            this.modifier_capture.clear();
            Some(hotkey)
        };
        match event {
            RawEvent::KeyDown { code, flags, repeat } => {
                self.modifier_capture.clear();
                if repeat {
                    return None;
                }
                let key = keymap::key_from_keycode(code)?;
                let modifiers = modifiers_from_cg_flags(flags);
                // A bare key is only a valid hotkey when it is an F key.
                if modifiers.is_empty() && !matches!(key, Key::F(_)) {
                    return None;
                }
                done(self, Hotkey::Chord { modifiers, key })
            }
            RawEvent::FlagsChanged { code, flags, .. } => {
                let Some(solo) = solo_from_keycode(code) else {
                    // Some other modifier changed: this is a combination in progress.
                    self.modifier_capture.clear();
                    return None;
                };
                if solo_is_down(solo, flags) {
                    self.modifier_capture.press(keymap::modifier_keys(flags));
                    None
                } else {
                    self.modifier_capture.release(solo).and_then(|hk| done(self, hk))
                }
            }
            RawEvent::KeyUp { .. } => None,
            RawEvent::PointerActivity => {
                self.modifier_capture.clear();
                None
            }
        }
    }
}

/// No Command, Control, or Option is down. Only a plain Esc cancels.
fn is_plain(flags: u64) -> bool {
    flags & (FLAG_COMMAND | FLAG_CONTROL | FLAG_ALT) == 0
}

/// The modifiers a chord needs, without Fn (which is checked on its own).
fn required_modifiers(m: Modifiers) -> Modifiers {
    Modifiers { function: false, ..m }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::{FLAG_SECONDARY_FN, FLAG_SHIFT};
    use std::time::Duration;

    const RIGHT_OPT_DOWN: u64 = FLAG_ALT | 0x40;

    fn logic() -> TapLogic {
        TapLogic::new(EscDetector::new(false, Duration::from_millis(400)))
    }

    fn solo_logic(m: SoloModifier) -> TapLogic {
        let mut l = logic();
        l.set_ptt(Some(Hotkey::Solo(m)));
        l
    }

    fn key_down(code: u16, flags: u64) -> RawEvent {
        RawEvent::KeyDown { code, flags, repeat: false }
    }

    const LEFT_CMD_DOWN: u64 = FLAG_COMMAND | 0x08;
    const LEFT_OPT_DOWN: u64 = FLAG_ALT | 0x20;

    fn flags(code: u16, flags: u64) -> RawEvent {
        RawEvent::FlagsChanged { code, flags, secure_input: false }
    }

    #[test]
    fn pointer_activity_cancels_toggle_and_recorder_candidates() {
        let now = Instant::now();
        for capturing in [false, true] {
            let mut l = logic();
            l.set_toggle_modifiers("left_command".parse::<Hotkey>().unwrap().modifier_keys());
            l.set_capturing(capturing);
            l.handle(flags(55, LEFT_CMD_DOWN), now);
            assert_eq!(l.handle(RawEvent::PointerActivity, now), Outcome::default());
            assert_eq!(l.handle(flags(55, 0), now), Outcome::default());
            // Cancellation must not poison the next clean tap.
            l.handle(flags(55, LEFT_CMD_DOWN), now);
            let out = l.handle(flags(55, 0), now);
            if capturing {
                assert_eq!(out.captured, Some(Hotkey::Solo(SoloModifier::LeftCommand)));
            } else {
                assert_eq!(out.events, vec![HotkeyEvent::Toggle]);
            }
        }
    }

    #[test]
    fn reconciliation_retains_really_held_keys_and_prunes_only_released_keys() {
        let now = Instant::now();
        let mut l = logic();
        l.set_toggle_modifiers("left_command".parse::<Hotkey>().unwrap().modifier_keys());
        l.handle(key_down(48, 0), now);
        l.handle(key_down(13, 0), now);
        l.reconcile_pressed_keys(|code| code == 13);
        l.handle(flags(55, LEFT_CMD_DOWN), now);
        l.reconcile_pressed_keys(|code| code == 13);
        assert!(l.handle(flags(55, 0), now).events.is_empty());
        // Once W really is released, a clean gesture works again.
        l.reconcile_pressed_keys(|_| false);
        l.handle(flags(55, LEFT_CMD_DOWN), now);
        assert_eq!(l.handle(flags(55, 0), now).events, vec![HotkeyEvent::Toggle]);
    }

    #[test]
    fn tap_deadline_is_from_first_modifier_and_recorder_remains_unbounded() {
        let now = Instant::now();
        for capturing in [false, true] {
            let mut l = logic();
            l.set_toggle_modifiers("left_command+left_option".parse::<Hotkey>().unwrap().modifier_keys());
            l.set_capturing(capturing);
            l.handle(flags(55, LEFT_CMD_DOWN), now);
            l.handle(flags(58, LEFT_CMD_DOWN | LEFT_OPT_DOWN), now + Duration::from_millis(350));
            let out = l.handle(flags(58, LEFT_CMD_DOWN), now + Duration::from_millis(401));
            assert!(out.events.is_empty());
            assert_eq!(out.captured, capturing.then(|| "left_command+left_option".parse().unwrap()));
        }
    }

    #[test]
    fn tap_at_400ms_still_toggles() {
        let now = Instant::now();
        let mut l = logic();
        l.set_toggle_modifiers("left_command".parse::<Hotkey>().unwrap().modifier_keys());
        l.handle(flags(55, LEFT_CMD_DOWN), now);
        assert_eq!(l.handle(flags(55, 0), now + Duration::from_millis(400)).events, vec![HotkeyEvent::Toggle]);
    }

    #[test]
    fn long_hold_does_not_toggle() {
        let now = Instant::now();
        let mut l = logic();
        l.set_toggle_modifiers("left_command".parse::<Hotkey>().unwrap().modifier_keys());
        l.handle(flags(55, LEFT_CMD_DOWN), now);
        assert!(l.handle(flags(55, 0), now + Duration::from_millis(401)).events.is_empty());
    }

    #[test]
    fn hidden_keyup_from_secure_input_recovers_toggle() {
        let now = Instant::now();
        let mut l = logic();
        l.set_toggle_modifiers("left_command".parse::<Hotkey>().unwrap().modifier_keys());
        l.handle(key_down(48, 0), now);
        // Tab moves focus into a password field; its release is hidden, and
        // no flagsChanged arrives until secure input has ended.
        l.reconcile_pressed_keys(|code| { assert_eq!(code, 48); false });
        l.handle(flags(55, LEFT_CMD_DOWN), now);
        assert_eq!(l.handle(flags(55, 0), now).events, vec![HotkeyEvent::Toggle]);
    }

    #[test]
    fn secure_input_cancels_modifier_toggles_without_stopping_push_to_talk() {
        let now = Instant::now();
        for (secure_down, secure_up) in [(true, true), (false, true), (true, false)] {
            let mut l = logic();
            l.set_toggle_modifiers("left_shift".parse::<Hotkey>().unwrap().modifier_keys());
            for (held, secure_input) in [(FLAG_SHIFT | 0x02, secure_down), (0, secure_up)] {
                let out = l.handle(RawEvent::FlagsChanged { code: 56, flags: held, secure_input }, now);
                assert!(out.events.is_empty());
            }
            assert!(l.handle(flags(56, FLAG_SHIFT | 0x02), now).events.is_empty());
            assert_eq!(l.handle(flags(56, 0), now).events, vec![HotkeyEvent::Toggle]);
        }

        let mut l = solo_logic(SoloModifier::RightOption);
        for (held, expected) in [(RIGHT_OPT_DOWN, HotkeyEvent::PushToTalkDown), (0, HotkeyEvent::PushToTalkUp)] {
            let out = l.handle(RawEvent::FlagsChanged { code: 61, flags: held, secure_input: true }, now);
            assert_eq!(out.events, vec![expected]);
        }
    }

    #[test]
    fn modifier_toggle_rejects_both_key_orders() {
        let now = Instant::now();
        for first in 0..2 {
            for last in 0..2 {
                let mut l = logic();
                l.set_toggle_modifiers("left_shift".parse::<Hotkey>().unwrap().modifier_keys());
                let press = [key_down(0, if first == 1 { FLAG_SHIFT | 0x02 } else { 0 }), flags(56, FLAG_SHIFT | 0x02)];
                let release = [RawEvent::KeyUp { code: 0, flags: if last == 0 { FLAG_SHIFT | 0x02 } else { 0 } }, flags(56, 0)];
                for event in [press[first], press[1 - first], release[last], release[1 - last]] {
                    assert!(l.handle(event, now).events.is_empty());
                }
                assert!(l.handle(flags(56, FLAG_SHIFT | 0x02), now).events.is_empty());
                assert_eq!(l.handle(flags(56, 0), now).events, vec![HotkeyEvent::Toggle]);
            }
        }
    }

    #[test]
    fn modifier_toggle_ignores_normal_shortcuts() {
        let now = Instant::now();
        for binding in ["left_command", "left_shift", "left_control+left_option"] {
            let keys = binding.parse::<Hotkey>().unwrap().modifier_keys().unwrap();
            let mut l = logic();
            l.set_toggle_modifiers(Some(keys));
            let mut held = 0;
            let mut events = Vec::new();
            for modifier in keys.iter() {
                let spec = keymap::solo_spec(modifier);
                held |= spec.down_bit;
                events.extend(l.handle(flags(spec.keycode, held), now).events);
            }
            events.extend(l.handle(key_down(0, held), now).events);
            events.extend(l.handle(RawEvent::KeyUp { code: 0, flags: held }, now).events);
            for modifier in keys.iter() {
                let spec = keymap::solo_spec(modifier);
                held &= !spec.down_bit;
                events.extend(l.handle(flags(spec.keycode, held), now).events);
            }
            assert!(events.is_empty(), "{binding} fired for a normal shortcut");
        }
    }

    #[test]
    fn every_detectable_modifier_combination_records_and_triggers() {
        let available = SoloModifier::ALL.to_vec();
        let now = Instant::now();
        for mask in 0usize..(1 << available.len()) {
            let selected: Vec<_> = available.iter().enumerate().filter_map(|(i, m)| (mask & (1 << i) != 0).then_some(*m)).collect();
            if selected.is_empty() { continue; }
            let modifiers: ModifierSet = selected.iter().copied().collect();
            let binding = Hotkey::modifier_only(modifiers).unwrap();
            for reverse in [false, true] {
                let mut order = selected.clone();
                if reverse { order.reverse(); }
                for released in selected.iter().copied() {
                    for toggle in [false, true] {
                        let mut active = logic();
                        if toggle { active.set_toggle_modifiers(Some(modifiers)); }
                        else { active.set_ptt(Some(binding)); }
                        let mut capture = logic();
                        capture.set_capturing(true);
                        let mut held = 0u64;
                        for (i, modifier) in order.iter().enumerate() {
                            held |= keymap::solo_spec(*modifier).down_bit;
                            let event = flags(keymap::solo_spec(*modifier).keycode, held);
                            let out = active.handle(event, now);
                            assert_eq!(capture.handle(event, now).captured, None);
                            let expected = if !toggle && i + 1 == order.len() {
                                vec![HotkeyEvent::PushToTalkDown]
                            } else { Vec::new() };
                            assert_eq!(out.events, expected, "{binding}, reverse={reverse}, toggle={toggle}");
                        }
                        held &= !keymap::solo_spec(released).down_bit;
                        let event = flags(keymap::solo_spec(released).keycode, held);
                        let out = active.handle(event, now);
                        assert_eq!(capture.handle(event, now).captured, Some(binding));
                        assert_eq!(out.events, vec![if toggle { HotkeyEvent::Toggle } else { HotkeyEvent::PushToTalkUp }]);
                    }
                }
            }
        }
    }

    #[test]
    fn releasing_an_extra_modifier_does_not_toggle() {
        let now = Instant::now();
        let extra = keymap::solo_spec(SoloModifier::LeftOption);
        for binding in ["left_shift", "left_control+left_command"] {
            let keys = binding.parse::<Hotkey>().unwrap().modifier_keys().unwrap();
            let mut l = logic();
            l.set_toggle_modifiers(Some(keys));
            let mut held = extra.down_bit;
            assert!(l.handle(flags(extra.keycode, held), now).events.is_empty());
            for modifier in keys.iter() {
                let spec = keymap::solo_spec(modifier);
                held |= spec.down_bit;
                assert!(l.handle(flags(spec.keycode, held), now).events.is_empty());
            }
            held &= !extra.down_bit;
            assert!(l.handle(flags(extra.keycode, held), now).events.is_empty());
            let required = keymap::solo_spec(keys.iter().next().unwrap());
            assert!(l.handle(flags(required.keycode, held & !required.down_bit), now).events.is_empty());
        }
    }

    #[test]
    fn modifier_pair_toggle_cancels_on_extra_modifier_in_both_orders() {
        for reverse in [false, true] {
            let mut l = logic();
            let Hotkey::ModifierOnly(pair) = "left_command+left_option".parse().unwrap() else { unreachable!() };
            l.set_toggle_modifiers(Some(pair));
            assert!(l.is_needed());
            let now = Instant::now();
            let mut keys = [(55, LEFT_CMD_DOWN), (58, LEFT_OPT_DOWN)];
            if reverse { keys.reverse(); }
            let both = LEFT_CMD_DOWN | LEFT_OPT_DOWN;
            for _ in 0..2 {
                assert!(l.handle(flags(keys[0].0, keys[0].1), now).events.is_empty());
                assert!(l.handle(flags(keys[1].0, both), now).events.is_empty());
                assert!(l.handle(flags(keys[1].0, both), now).events.is_empty());
                // A third modifier cancels the pending toggle.
                assert!(l.handle(flags(60, both | FLAG_SHIFT | 0x04), now).events.is_empty());
                assert!(l.handle(flags(60, both), now).events.is_empty());
                assert!(l.handle(flags(keys[0].0, keys[1].1), now).events.is_empty());
                assert!(l.handle(flags(keys[1].0, 0), now).events.is_empty());
            }
            l.set_toggle_modifiers(None);
            assert!(!l.is_needed());
            assert!(l.handle(flags(keys[1].0, both), now).events.is_empty());
        }
    }

    #[test]
    fn modifier_pair_toggle_uses_exact_sides_and_stays_silent_during_capture() {
        let mut l = logic();
        let Hotkey::ModifierOnly(pair) = "left_command+left_option".parse().unwrap() else { unreachable!() };
        l.set_toggle_modifiers(Some(pair));
        let now = Instant::now();
        assert!(l.handle(flags(61, LEFT_CMD_DOWN | RIGHT_OPT_DOWN), now).events.is_empty());
        assert!(l.handle(flags(54, FLAG_COMMAND | 0x10 | LEFT_OPT_DOWN), now).events.is_empty());
        l.set_capturing(true);
        assert!(l.handle(flags(55, LEFT_CMD_DOWN), now).events.is_empty());
        assert!(l.handle(flags(58, LEFT_CMD_DOWN | LEFT_OPT_DOWN), now).events.is_empty());
        l.set_capturing(false);
        l.release_held();
        assert!(l.handle(flags(55, LEFT_CMD_DOWN), now).events.is_empty());
        assert!(l.handle(flags(58, LEFT_CMD_DOWN | LEFT_OPT_DOWN), now).events.is_empty());
        assert_eq!(l.handle(flags(58, LEFT_CMD_DOWN), now).events, vec![HotkeyEvent::Toggle]);
    }

    #[test]
    fn modifier_pair_requires_exact_sides_and_releases_on_reset() {
        let mut l = logic();
        l.set_ptt(Some("left_command+left_option".parse().unwrap()));
        let now = Instant::now();
        assert!(l.handle(flags(61, LEFT_CMD_DOWN | RIGHT_OPT_DOWN), now).events.is_empty());
        assert!(l.handle(flags(54, FLAG_COMMAND | 0x10 | LEFT_OPT_DOWN), now).events.is_empty());
        let both = LEFT_CMD_DOWN | LEFT_OPT_DOWN;
        assert_eq!(l.handle(flags(58, both), now).events, vec![HotkeyEvent::PushToTalkDown]);
        assert_eq!(l.handle(flags(60, both | FLAG_SHIFT | 0x04), now).events, vec![HotkeyEvent::PushToTalkUp]);
        assert_eq!(l.handle(flags(60, both), now).events, vec![HotkeyEvent::PushToTalkDown]);
        assert_eq!(l.release_held(), Some(HotkeyEvent::PushToTalkUp));
        assert_eq!(l.release_held(), None);
        l.handle(flags(58, both), now);
        assert_eq!(l.set_ptt(None), Some(HotkeyEvent::PushToTalkUp));
    }

    #[test]
    fn capture_modifier_pair_still_allows_a_regular_chord() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        l.handle(flags(55, LEFT_CMD_DOWN), now);
        l.handle(flags(58, LEFT_CMD_DOWN | LEFT_OPT_DOWN), now);
        let out = l.handle(key_down(49, LEFT_CMD_DOWN | LEFT_OPT_DOWN), now);
        assert_eq!(out.captured, Some("cmd+opt+space".parse().unwrap()));
    }

    #[test]
    fn capture_does_not_save_partial_pairs_after_other_keys() {
        for extra in [flags(57, LEFT_CMD_DOWN | LEFT_OPT_DOWN), key_down(123, LEFT_CMD_DOWN | LEFT_OPT_DOWN)] {
            let mut l = logic();
            l.set_capturing(true);
            let now = Instant::now();
            l.handle(flags(55, LEFT_CMD_DOWN), now);
            l.handle(flags(58, LEFT_CMD_DOWN | LEFT_OPT_DOWN), now);
            assert_eq!(l.handle(extra, now).captured, None);
            assert_eq!(l.handle(flags(58, LEFT_CMD_DOWN), now).captured, None);
            assert_eq!(l.handle(flags(55, 0), now).captured, None);
        }
    }

    #[test]
    fn right_option_press_and_release() {
        let mut l = solo_logic(SoloModifier::RightOption);
        let now = Instant::now();
        let down = l.handle(flags(61, RIGHT_OPT_DOWN), now);
        assert_eq!(down.events, vec![HotkeyEvent::PushToTalkDown]);
        // A repeated flagsChanged with the same state does nothing.
        assert!(l.handle(flags(61, RIGHT_OPT_DOWN), now).events.is_empty());
        let up = l.handle(flags(61, 0), now);
        assert_eq!(up.events, vec![HotkeyEvent::PushToTalkUp]);
    }

    #[test]
    fn left_option_does_not_trigger_right_option() {
        let mut l = solo_logic(SoloModifier::RightOption);
        let out = l.handle(flags(58, FLAG_ALT | 0x20), Instant::now());
        assert!(out.events.is_empty());
    }

    #[test]
    fn right_option_release_while_left_option_held() {
        let mut l = solo_logic(SoloModifier::RightOption);
        let now = Instant::now();
        l.handle(flags(61, RIGHT_OPT_DOWN | 0x20), now);
        // Right released, left still down: generic Alt flag stays set.
        let up = l.handle(flags(61, FLAG_ALT | 0x20), now);
        assert_eq!(up.events, vec![HotkeyEvent::PushToTalkUp]);
    }

    #[test]
    fn fn_press_and_release() {
        let mut l = solo_logic(SoloModifier::Fn);
        let now = Instant::now();
        assert_eq!(
            l.handle(flags(63, FLAG_SECONDARY_FN), now).events,
            vec![HotkeyEvent::PushToTalkDown]
        );
        assert_eq!(
            l.handle(flags(63, 0), now).events,
            vec![HotkeyEvent::PushToTalkUp]
        );
    }

    #[test]
    fn arrow_key_with_fn_flag_is_not_fn() {
        let mut l = solo_logic(SoloModifier::Fn);
        // Arrow keys arrive as keyDown with the Fn flag, not as flagsChanged for code 63.
        let out = l.handle(key_down(123, FLAG_SECONDARY_FN), Instant::now());
        assert!(out.events.is_empty());
    }

    #[test]
    fn chord_ptt_follows_key_and_modifiers() {
        let mut l = logic();
        l.set_ptt(Some("ctrl+shift+d".parse().unwrap()));
        let now = Instant::now();
        let mods = FLAG_CONTROL | FLAG_SHIFT;
        assert!(l.handle(key_down(2, FLAG_CONTROL), now).events.is_empty());
        assert_eq!(l.handle(key_down(2, mods), now).events, vec![HotkeyEvent::PushToTalkDown]);
        // Auto-repeat is ignored.
        assert!(l.handle(RawEvent::KeyDown { code: 2, flags: mods, repeat: true }, now).events.is_empty());
        assert_eq!(l.handle(RawEvent::KeyUp { code: 2, flags: mods }, now).events, vec![HotkeyEvent::PushToTalkUp]);
    }

    #[test]
    fn chord_ptt_releases_when_a_modifier_goes_first() {
        let mut l = logic();
        l.set_ptt(Some("ctrl+shift+d".parse().unwrap()));
        let now = Instant::now();
        l.handle(key_down(2, FLAG_CONTROL | FLAG_SHIFT), now);
        let out = l.handle(flags(56, FLAG_CONTROL), now);
        assert_eq!(out.events, vec![HotkeyEvent::PushToTalkUp]);
        // The later key-up must not release twice.
        assert!(l.handle(RawEvent::KeyUp { code: 2, flags: 0 }, now).events.is_empty());
    }

    #[test]
    fn esc_is_ignored_when_not_recording() {
        let mut l = logic();
        let t0 = Instant::now();
        assert!(l.handle(key_down(53, 0), t0).events.is_empty());
        assert!(l.handle(key_down(53, 0), t0 + Duration::from_millis(100)).events.is_empty());
    }

    #[test]
    fn double_esc_cancels_while_recording() {
        let mut l = logic();
        l.set_recording(true);
        let t0 = Instant::now();
        assert!(l.handle(key_down(53, 0), t0).events.is_empty());
        let out = l.handle(key_down(53, 0), t0 + Duration::from_millis(180));
        assert_eq!(out.events, vec![HotkeyEvent::Cancel]);
    }

    #[test]
    fn esc_repeat_and_modified_esc_do_not_count() {
        let mut l = logic();
        l.set_recording(true);
        let t0 = Instant::now();
        l.handle(key_down(53, 0), t0);
        let repeat = RawEvent::KeyDown { code: 53, flags: 0, repeat: true };
        assert!(l.handle(repeat, t0 + Duration::from_millis(50)).events.is_empty());
        assert!(l.handle(key_down(53, FLAG_COMMAND), t0 + Duration::from_millis(60)).events.is_empty());
    }

    #[test]
    fn single_esc_setting() {
        let mut l = logic();
        l.configure_esc(true, Duration::from_millis(400));
        l.set_recording(true);
        assert_eq!(l.handle(key_down(53, 0), Instant::now()).events, vec![HotkeyEvent::Cancel]);
    }

    #[test]
    fn stopping_recording_forgets_a_half_finished_double_esc() {
        let mut l = logic();
        let t0 = Instant::now();
        l.set_recording(true);
        l.handle(key_down(53, 0), t0);
        l.set_recording(false);
        l.set_recording(true);
        assert!(l.handle(key_down(53, 0), t0 + Duration::from_millis(100)).events.is_empty());
    }

    #[test]
    fn the_active_tap_takes_esc_and_sends_the_cancel() {
        let mut l = logic();
        l.set_take_escape(true);
        l.set_active_tap(true);
        l.set_recording(true);
        let t0 = Instant::now();
        assert_eq!(l.active_esc(true, false, 0, t0), (None, true), "the first of a double Esc");
        assert_eq!(l.active_esc(false, false, 0, t0), (None, true));
        assert_eq!(l.active_esc(true, false, 0, t0 + Duration::from_millis(180)), (Some(HotkeyEvent::Cancel), true));
        // The listen-only tap sees the same press, but must not cancel twice.
        l.set_recording(true);
        assert!(l.handle(key_down(53, 0), t0).events.is_empty());
        assert!(l.handle(key_down(53, 0), t0 + Duration::from_millis(100)).events.is_empty());
    }

    #[test]
    fn the_active_tap_leaves_a_modified_esc_and_other_states_alone() {
        let mut l = logic();
        l.set_take_escape(true);
        l.set_active_tap(true);
        let now = Instant::now();
        assert_eq!(l.active_esc(true, false, 0, now), (None, false), "not recording");
        l.set_recording(true);
        assert_eq!(l.active_esc(true, false, FLAG_COMMAND, now), (None, false));
        l.set_capturing(true);
        assert_eq!(l.active_esc(true, false, 0, now), (None, false), "the key recorder is open");
    }

    #[test]
    fn without_the_active_tap_the_listen_tap_cancels_as_before() {
        let mut l = logic();
        l.set_take_escape(true);
        l.configure_esc(true, Duration::from_millis(400));
        l.set_recording(true);
        assert_eq!(l.active_esc(true, false, 0, Instant::now()), (None, false));
        assert_eq!(l.handle(key_down(53, 0), Instant::now()).events, vec![HotkeyEvent::Cancel]);
    }

    #[test]
    fn release_held_reports_a_pending_release_once() {
        let mut l = solo_logic(SoloModifier::RightOption);
        l.handle(flags(61, RIGHT_OPT_DOWN), Instant::now());
        assert_eq!(l.release_held(), Some(HotkeyEvent::PushToTalkUp));
        assert_eq!(l.release_held(), None);
    }

    #[test]
    fn changing_the_binding_releases_a_held_ptt() {
        let mut l = solo_logic(SoloModifier::RightOption);
        l.handle(flags(61, RIGHT_OPT_DOWN), Instant::now());
        assert_eq!(l.set_ptt(None), Some(HotkeyEvent::PushToTalkUp));
        assert!(!l.is_needed());
    }

    #[test]
    fn capture_returns_a_chord_on_key_down() {
        let mut l = logic();
        l.set_capturing(true);
        let out = l.handle(key_down(49, FLAG_ALT), Instant::now());
        assert_eq!(out.captured, Some(Hotkey::toggle_default()));
        assert!(!l.is_needed(), "capture ends after one result");
        // Not delivered twice.
        assert_eq!(l.handle(key_down(49, FLAG_ALT), Instant::now()).captured, None);
    }

    #[test]
    fn capture_ignores_bare_letters_but_takes_f_keys() {
        let mut l = logic();
        l.set_capturing(true);
        assert_eq!(l.handle(key_down(0, 0), Instant::now()).captured, None);
        assert_eq!(l.handle(key_down(53, 0), Instant::now()).captured, None);
        let out = l.handle(key_down(96, 0), Instant::now());
        assert_eq!(out.captured, Some(Hotkey::Chord { modifiers: Modifiers::default(), key: Key::F(5) }));
    }

    #[test]
    fn capture_returns_a_solo_modifier_after_release() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        assert_eq!(l.handle(flags(61, RIGHT_OPT_DOWN), now).captured, None);
        let out = l.handle(flags(61, 0), now);
        assert_eq!(out.captured, Some(Hotkey::Solo(SoloModifier::RightOption)));
    }

    #[test]
    fn capture_does_not_take_a_modifier_that_became_part_of_a_chord() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        l.handle(flags(61, RIGHT_OPT_DOWN), now);
        // Option+Space completes the chord, so the later release must not also fire.
        let chord = l.handle(key_down(49, RIGHT_OPT_DOWN), now);
        assert!(matches!(chord.captured, Some(Hotkey::Chord { .. })));
        l.set_capturing(true);
        assert_eq!(l.handle(flags(61, 0), now).captured, None);
    }

    #[test]
    fn capture_ignores_modifiers_held_before_recording_starts() {
        let mut l = logic();
        let now = Instant::now();
        l.handle(flags(55, LEFT_CMD_DOWN), now);
        l.set_capturing(true);
        l.handle(flags(61, LEFT_CMD_DOWN | RIGHT_OPT_DOWN), now);
        let out = l.handle(flags(61, LEFT_CMD_DOWN), now);
        assert_eq!(out.captured, None);
    }

    #[test]
    fn capture_suppresses_ptt_and_cancel_events() {
        let mut l = solo_logic(SoloModifier::RightOption);
        l.set_recording(true);
        l.set_capturing(true);
        let now = Instant::now();
        let out = l.handle(flags(61, RIGHT_OPT_DOWN), now);
        assert!(out.events.is_empty());
    }
}
