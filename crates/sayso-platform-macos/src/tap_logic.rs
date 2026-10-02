//! The decision logic behind the listen-only event tap. No FFI, so it is unit
//! tested with plain values.
//!
//! The tap thread turns each CGEvent into a [`RawEvent`] and calls
//! [`TapLogic::handle`]. The logic answers with hotkey events and, while the
//! key recorder is open, with the captured hotkey.

use crate::esc::EscDetector;
use crate::keymap::{
    self, KC_ESCAPE, modifiers_from_cg_flags, solo_from_keycode, solo_is_down,
};
use crate::ffi::{FLAG_ALT, FLAG_COMMAND, FLAG_CONTROL};
use sayso_core::hotkey::{Hotkey, Key, Modifiers, SoloModifier};
use sayso_platform::HotkeyEvent;
use std::time::Instant;

/// One keyboard event from the tap, reduced to the fields we use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawEvent {
    KeyDown { code: u16, flags: u64, repeat: bool },
    KeyUp { code: u16, flags: u64 },
    FlagsChanged { code: u16, flags: u64 },
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
    Chord { keycode: u16, modifiers: Modifiers },
}

#[derive(Debug)]
pub struct TapLogic {
    ptt: Option<Ptt>,
    ptt_down: bool,
    recording: bool,
    esc: EscDetector,
    /// The key recorder is open.
    capturing: bool,
    /// A solo modifier went down and nothing else happened since.
    solo_candidate: Option<SoloModifier>,
}

impl TapLogic {
    pub fn new(esc: EscDetector) -> Self {
        Self { ptt: None, ptt_down: false, recording: false, esc, capturing: false, solo_candidate: None }
    }

    /// True when the tap has any reason to run.
    pub fn is_needed(&self) -> bool {
        self.ptt.is_some() || self.recording || self.capturing
    }

    /// Set the push-to-talk binding. Returns a release event when a held
    /// push-to-talk is dropped by the change.
    pub fn set_ptt(&mut self, hotkey: Option<Hotkey>) -> Option<HotkeyEvent> {
        let released = self.release_held();
        self.ptt = hotkey.and_then(|h| match h {
            Hotkey::Solo(m) => Some(Ptt::Solo(m)),
            Hotkey::Chord { modifiers, key } => keymap::keycode(key).map(|keycode| Ptt::Chord { keycode, modifiers }),
        });
        released
    }

    pub fn configure_esc(&mut self, single: bool, window: std::time::Duration) {
        self.esc.configure(single, window);
    }

    pub fn set_recording(&mut self, recording: bool) {
        self.recording = recording;
        self.esc.reset();
    }

    pub fn set_capturing(&mut self, capturing: bool) {
        self.capturing = capturing;
        self.solo_candidate = None;
    }

    /// The system disabled the tap, so key-up events may have been missed.
    pub fn release_held(&mut self) -> Option<HotkeyEvent> {
        std::mem::take(&mut self.ptt_down).then_some(HotkeyEvent::PushToTalkUp)
    }

    pub fn handle(&mut self, event: RawEvent, now: Instant) -> Outcome {
        if self.capturing {
            return Outcome { events: Vec::new(), captured: self.capture(event) };
        }
        let mut out = Outcome::default();
        match event {
            RawEvent::FlagsChanged { code, flags } => match self.ptt {
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
            },
            RawEvent::KeyDown { code, flags, repeat } => {
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
                let plain = flags & (FLAG_COMMAND | FLAG_CONTROL | FLAG_ALT) == 0;
                if code == KC_ESCAPE && self.recording && plain && self.esc.press(now) {
                    out.events.push(HotkeyEvent::Cancel);
                }
            }
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

    /// Key recorder: a chord completes on key down, a solo modifier completes
    /// when it is released without any other key in between.
    fn capture(&mut self, event: RawEvent) -> Option<Hotkey> {
        let done = |this: &mut Self, hotkey| {
            this.capturing = false;
            this.solo_candidate = None;
            Some(hotkey)
        };
        match event {
            RawEvent::KeyDown { code, flags, repeat } => {
                self.solo_candidate = None;
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
            RawEvent::FlagsChanged { code, flags } => {
                let Some(solo) = solo_from_keycode(code) else {
                    // Some other modifier changed: this is a combination in progress.
                    self.solo_candidate = None;
                    return None;
                };
                if solo_is_down(solo, flags) {
                    let others = modifiers_from_cg_flags(flags);
                    let alone = match solo {
                        SoloModifier::Fn => others.is_empty(),
                        _ => [others.command, others.option, others.control, others.shift].iter().filter(|b| **b).count() == 1,
                    };
                    self.solo_candidate = (alone && self.solo_candidate.is_none()).then_some(solo);
                    None
                } else if self.solo_candidate == Some(solo) {
                    done(self, Hotkey::Solo(solo))
                } else {
                    None
                }
            }
            RawEvent::KeyUp { .. } => None,
        }
    }
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

    #[test]
    fn right_option_press_and_release() {
        let mut l = solo_logic(SoloModifier::RightOption);
        let now = Instant::now();
        let down = l.handle(RawEvent::FlagsChanged { code: 61, flags: RIGHT_OPT_DOWN }, now);
        assert_eq!(down.events, vec![HotkeyEvent::PushToTalkDown]);
        // A repeated flagsChanged with the same state does nothing.
        assert!(l.handle(RawEvent::FlagsChanged { code: 61, flags: RIGHT_OPT_DOWN }, now).events.is_empty());
        let up = l.handle(RawEvent::FlagsChanged { code: 61, flags: 0 }, now);
        assert_eq!(up.events, vec![HotkeyEvent::PushToTalkUp]);
    }

    #[test]
    fn left_option_does_not_trigger_right_option() {
        let mut l = solo_logic(SoloModifier::RightOption);
        let out = l.handle(RawEvent::FlagsChanged { code: 58, flags: FLAG_ALT | 0x20 }, Instant::now());
        assert!(out.events.is_empty());
    }

    #[test]
    fn right_option_release_while_left_option_held() {
        let mut l = solo_logic(SoloModifier::RightOption);
        let now = Instant::now();
        l.handle(RawEvent::FlagsChanged { code: 61, flags: RIGHT_OPT_DOWN | 0x20 }, now);
        // Right released, left still down: generic Alt flag stays set.
        let up = l.handle(RawEvent::FlagsChanged { code: 61, flags: FLAG_ALT | 0x20 }, now);
        assert_eq!(up.events, vec![HotkeyEvent::PushToTalkUp]);
    }

    #[test]
    fn fn_press_and_release() {
        let mut l = solo_logic(SoloModifier::Fn);
        let now = Instant::now();
        assert_eq!(
            l.handle(RawEvent::FlagsChanged { code: 63, flags: FLAG_SECONDARY_FN }, now).events,
            vec![HotkeyEvent::PushToTalkDown]
        );
        assert_eq!(
            l.handle(RawEvent::FlagsChanged { code: 63, flags: 0 }, now).events,
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
        let out = l.handle(RawEvent::FlagsChanged { code: 56, flags: FLAG_CONTROL }, now);
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
    fn release_held_reports_a_pending_release_once() {
        let mut l = solo_logic(SoloModifier::RightOption);
        l.handle(RawEvent::FlagsChanged { code: 61, flags: RIGHT_OPT_DOWN }, Instant::now());
        assert_eq!(l.release_held(), Some(HotkeyEvent::PushToTalkUp));
        assert_eq!(l.release_held(), None);
    }

    #[test]
    fn changing_the_binding_releases_a_held_ptt() {
        let mut l = solo_logic(SoloModifier::RightOption);
        l.handle(RawEvent::FlagsChanged { code: 61, flags: RIGHT_OPT_DOWN }, Instant::now());
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
        assert_eq!(l.handle(RawEvent::FlagsChanged { code: 61, flags: RIGHT_OPT_DOWN }, now).captured, None);
        let out = l.handle(RawEvent::FlagsChanged { code: 61, flags: 0 }, now);
        assert_eq!(out.captured, Some(Hotkey::Solo(SoloModifier::RightOption)));
    }

    #[test]
    fn capture_does_not_take_a_modifier_that_became_part_of_a_chord() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        l.handle(RawEvent::FlagsChanged { code: 61, flags: RIGHT_OPT_DOWN }, now);
        // Option+Space completes the chord, so the later release must not also fire.
        let chord = l.handle(key_down(49, RIGHT_OPT_DOWN), now);
        assert!(matches!(chord.captured, Some(Hotkey::Chord { .. })));
        l.set_capturing(true);
        assert_eq!(l.handle(RawEvent::FlagsChanged { code: 61, flags: 0 }, now).captured, None);
    }

    #[test]
    fn capture_ignores_a_solo_pressed_while_another_modifier_is_held() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        l.handle(RawEvent::FlagsChanged { code: 55, flags: FLAG_COMMAND }, now);
        l.handle(RawEvent::FlagsChanged { code: 61, flags: FLAG_COMMAND | RIGHT_OPT_DOWN }, now);
        let out = l.handle(RawEvent::FlagsChanged { code: 61, flags: FLAG_COMMAND }, now);
        assert_eq!(out.captured, None);
    }

    #[test]
    fn capture_suppresses_ptt_and_cancel_events() {
        let mut l = solo_logic(SoloModifier::RightOption);
        l.set_recording(true);
        l.set_capturing(true);
        let now = Instant::now();
        let out = l.handle(RawEvent::FlagsChanged { code: 61, flags: RIGHT_OPT_DOWN }, now);
        assert!(out.events.is_empty());
    }
}
