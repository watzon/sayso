//! The decision logic behind the low-level keyboard hook. No FFI, so it is
//! unit tested with plain values.
//!
//! The hook thread turns each `KBDLLHOOKSTRUCT` into a [`RawEvent`] and calls
//! [`HookLogic::handle`]. The logic answers with hotkey events and, while the
//! key recorder is open, with the captured hotkey.
//!
//! Two Windows details shape it:
//! - The hook has no auto-repeat flag. A held key sends more key-down events
//!   with no key-up between them, so the logic remembers which keys are down.
//! - On keyboard layouts with AltGr (German, French, and many more), Right Alt
//!   comes with a fake Left Ctrl. Its scan code has the `0x200` bit. The logic
//!   ignores that Ctrl, so Right Alt works as a solo push-to-talk key and the
//!   key recorder does not record Ctrl+Alt.

use crate::esc::EscDetector;
use crate::keymap::{self, ModKeys, VK_ESCAPE, VK_LCONTROL};
use sayso_core::hotkey::{Hotkey, Key, Modifiers, SoloModifier};
use sayso_platform::HotkeyEvent;
use std::time::Instant;

/// `KBDLLHOOKSTRUCT.flags`: the event came from `SendInput` or `keybd_event`.
pub const LLKHF_INJECTED: u32 = 0x10;
/// `KBDLLHOOKSTRUCT.flags`: the key is an extended key (right Ctrl and Alt, arrows).
pub const LLKHF_EXTENDED: u32 = 0x01;
/// Scan code bit of the fake Left Ctrl that comes with AltGr.
pub const ALTGR_CTRL_SCAN: u32 = 0x200;

/// One keyboard event from the hook, reduced to the fields we use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawEvent {
    /// Virtual-key code, with left and right apart (see [`keymap::sided_vk`]).
    pub vk: u16,
    pub scan: u32,
    /// `KBDLLHOOKSTRUCT.flags`.
    pub flags: u32,
    pub down: bool,
    /// The modifier keys that were down before this event (from
    /// `GetAsyncKeyState`). The logic applies the event itself on top.
    pub held: ModKeys,
}

/// What the hook should do for one raw event.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub events: Vec<HotkeyEvent>,
    /// Set when the key recorder finished with this event.
    pub captured: Option<Hotkey>,
    /// Send the mask key now: an Alt or Win key that Sayso uses is down, and
    /// its release must not open the menu bar or the Start menu.
    pub mask: bool,
}

/// The push-to-talk binding as the hook sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ptt {
    Solo { vk: u16 },
    Chord { vk: u16, modifiers: Modifiers },
}

/// Which of the 256 key codes are down, as far as the hook has seen.
#[derive(Debug, Default, Clone, Copy)]
struct KeySet([u64; 4]);

impl KeySet {
    /// Mark `vk` as down. Returns true when it was already down: auto-repeat.
    fn press(&mut self, vk: u16) -> bool {
        let (word, bit) = ((vk as usize >> 6) & 3, 1u64 << (vk & 63));
        let was = self.0[word] & bit != 0;
        self.0[word] |= bit;
        was
    }

    fn release(&mut self, vk: u16) {
        self.0[(vk as usize >> 6) & 3] &= !(1u64 << (vk & 63));
    }
}

#[derive(Debug)]
pub struct HookLogic {
    ptt: Option<Ptt>,
    ptt_down: bool,
    recording: bool,
    esc: EscDetector,
    /// The key recorder is open.
    capturing: bool,
    /// A solo modifier went down and nothing else happened since.
    solo_candidate: Option<SoloModifier>,
    pressed: KeySet,
    /// The fake Left Ctrl of AltGr is down.
    altgr_ctrl: bool,
}

/// Whether this is the fake Left Ctrl that the keyboard layout sends with AltGr.
pub fn is_altgr_ctrl(event: &RawEvent) -> bool {
    event.vk == VK_LCONTROL && event.scan & ALTGR_CTRL_SCAN != 0
}

impl HookLogic {
    pub fn new(esc: EscDetector) -> Self {
        Self {
            ptt: None,
            ptt_down: false,
            recording: false,
            esc,
            capturing: false,
            solo_candidate: None,
            pressed: KeySet::default(),
            altgr_ctrl: false,
        }
    }

    /// True when the hook has any reason to be installed.
    pub fn is_needed(&self) -> bool {
        self.ptt.is_some() || self.recording || self.capturing
    }

    /// Set the push-to-talk binding. Returns a release event when a held
    /// push-to-talk is dropped by the change. Hotkeys with Fn cannot work on
    /// Windows and leave push-to-talk off.
    pub fn set_ptt(&mut self, hotkey: Option<Hotkey>) -> Option<HotkeyEvent> {
        let released = self.release_held();
        self.ptt = hotkey.and_then(|h| match h {
            Hotkey::Solo(solo) => keymap::solo_vk(solo).map(|vk| Ptt::Solo { vk }),
            Hotkey::Chord { modifiers, .. } if modifiers.function => None,
            Hotkey::Chord { modifiers, key } => keymap::vk(key).map(|vk| Ptt::Chord { vk, modifiers }),
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

    /// Forget a held push-to-talk, for example when the hook stops.
    pub fn release_held(&mut self) -> Option<HotkeyEvent> {
        std::mem::take(&mut self.ptt_down).then_some(HotkeyEvent::PushToTalkUp)
    }

    /// The modifier keys after this event, without the fake Ctrl of AltGr.
    fn modifiers_after(&self, event: &RawEvent) -> ModKeys {
        let held = event.held.with(event.vk, event.down);
        if self.altgr_ctrl { held.with(VK_LCONTROL, false) } else { held }
    }

    pub fn handle(&mut self, event: RawEvent, now: Instant) -> Outcome {
        // Sayso's own SendInput (paste, typing, the mask key) and other
        // programs' synthetic keys never trigger a hotkey.
        if event.flags & LLKHF_INJECTED != 0 {
            return Outcome::default();
        }
        if is_altgr_ctrl(&event) {
            self.altgr_ctrl = event.down;
            return Outcome::default();
        }
        let repeat = if event.down {
            self.pressed.press(event.vk)
        } else {
            self.pressed.release(event.vk);
            false
        };
        let held = self.modifiers_after(&event);
        if self.capturing {
            return self.capture(&event, repeat, held);
        }
        let mut out = Outcome::default();
        if keymap::is_modifier(event.vk) {
            match self.ptt {
                Some(Ptt::Solo { vk }) if vk == event.vk => {
                    if event.down && !repeat {
                        self.set_ptt_down(true, &mut out.events);
                        out.mask = keymap::release_has_meaning(vk);
                    } else if !event.down {
                        self.set_ptt_down(false, &mut out.events);
                    }
                }
                // The user let go of a modifier before the key.
                Some(Ptt::Chord { modifiers, .. }) if self.ptt_down && held.modifiers() != modifiers => {
                    self.set_ptt_down(false, &mut out.events);
                }
                _ => {}
            }
        } else if event.down {
            if repeat {
                return out;
            }
            if let Some(Ptt::Chord { vk, modifiers }) = self.ptt
                && event.vk == vk
                && held.modifiers() == modifiers
            {
                self.set_ptt_down(true, &mut out.events);
            }
            let m = held.modifiers();
            let plain = !(m.command || m.control || m.option);
            if event.vk == VK_ESCAPE && self.recording && plain && self.esc.press(now) {
                out.events.push(HotkeyEvent::Cancel);
            }
        } else if let Some(Ptt::Chord { vk, .. }) = self.ptt
            && event.vk == vk
        {
            self.set_ptt_down(false, &mut out.events);
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
    fn capture(&mut self, event: &RawEvent, repeat: bool, held: ModKeys) -> Outcome {
        let mut out = Outcome::default();
        let done = |this: &mut Self, out: &mut Outcome, hotkey| {
            this.capturing = false;
            this.solo_candidate = None;
            out.captured = Some(hotkey);
        };
        let solo = keymap::solo_from_vk(event.vk);
        match (keymap::is_modifier(event.vk), event.down) {
            (false, true) => {
                self.solo_candidate = None;
                if repeat {
                    return out;
                }
                let Some(key) = keymap::key_from_vk(event.vk) else { return out };
                let modifiers = held.modifiers();
                // A bare key is only a valid hotkey when it is an F key.
                if modifiers.is_empty() && !matches!(key, Key::F(_)) {
                    return out;
                }
                done(self, &mut out, Hotkey::Chord { modifiers, key });
            }
            (true, true) => {
                if repeat {
                    return out;
                }
                // Alone means: this is the only modifier down.
                let candidate = solo.filter(|_| held.count() == 1 && self.solo_candidate.is_none());
                self.solo_candidate = candidate;
                // Recording Right Alt or Right Win must not open the menu or Start.
                out.mask = candidate.is_some() && keymap::release_has_meaning(event.vk);
            }
            (true, false) => {
                if let Some(solo) = solo
                    && self.solo_candidate == Some(solo)
                {
                    done(self, &mut out, Hotkey::Solo(solo));
                }
            }
            (false, false) => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::*;
    use std::time::Duration;

    const KEY_D: u16 = 0x44;
    const KEY_Q: u16 = 0x51;

    fn logic() -> HookLogic {
        HookLogic::new(EscDetector::new(false, Duration::from_millis(400)))
    }

    fn solo_logic(m: SoloModifier) -> HookLogic {
        let mut l = logic();
        l.set_ptt(Some(Hotkey::Solo(m)));
        l
    }

    /// A key event with these modifiers held before it.
    fn ev(vk: u16, down: bool, held: &[u16]) -> RawEvent {
        RawEvent { vk, scan: 0, flags: 0, down, held: ModKeys::of(held) }
    }

    fn down(vk: u16, held: &[u16]) -> RawEvent {
        ev(vk, true, held)
    }

    fn up(vk: u16, held: &[u16]) -> RawEvent {
        ev(vk, false, held)
    }

    /// The fake Left Ctrl of AltGr.
    fn altgr_ctrl(is_down: bool, held: &[u16]) -> RawEvent {
        RawEvent { vk: VK_LCONTROL, scan: 0x21D, flags: 0, down: is_down, held: ModKeys::of(held) }
    }

    #[test]
    fn right_alt_press_and_release() {
        let mut l = solo_logic(SoloModifier::RightOption);
        let now = Instant::now();
        let pressed = l.handle(down(VK_RMENU, &[]), now);
        assert_eq!(pressed.events, vec![HotkeyEvent::PushToTalkDown]);
        assert!(pressed.mask, "the Alt release must not open the menu bar");
        // Auto-repeat: more key-down events, no key-up.
        let repeat = l.handle(down(VK_RMENU, &[VK_RMENU]), now);
        assert!(repeat.events.is_empty());
        assert!(!repeat.mask);
        assert_eq!(l.handle(up(VK_RMENU, &[VK_RMENU]), now).events, vec![HotkeyEvent::PushToTalkUp]);
    }

    #[test]
    fn left_alt_does_not_trigger_right_alt() {
        let mut l = solo_logic(SoloModifier::RightOption);
        assert!(l.handle(down(VK_LMENU, &[]), Instant::now()).events.is_empty());
    }

    #[test]
    fn right_alt_release_while_left_alt_held() {
        let mut l = solo_logic(SoloModifier::RightOption);
        let now = Instant::now();
        l.handle(down(VK_LMENU, &[]), now);
        l.handle(down(VK_RMENU, &[VK_LMENU]), now);
        let out = l.handle(up(VK_RMENU, &[VK_LMENU, VK_RMENU]), now);
        assert_eq!(out.events, vec![HotkeyEvent::PushToTalkUp]);
    }

    #[test]
    fn altgr_works_as_right_alt_push_to_talk() {
        let mut l = solo_logic(SoloModifier::RightOption);
        let now = Instant::now();
        // AltGr: a fake Left Ctrl, then Right Alt. On release, the same two again.
        assert!(l.handle(altgr_ctrl(true, &[]), now).events.is_empty());
        assert_eq!(l.handle(down(VK_RMENU, &[VK_LCONTROL]), now).events, vec![HotkeyEvent::PushToTalkDown]);
        assert!(l.handle(altgr_ctrl(false, &[VK_LCONTROL, VK_RMENU]), now).events.is_empty());
        assert_eq!(l.handle(up(VK_RMENU, &[VK_RMENU]), now).events, vec![HotkeyEvent::PushToTalkUp]);
    }

    #[test]
    fn right_ctrl_and_right_shift_need_no_mask() {
        for (m, vk) in [(SoloModifier::RightControl, VK_RCONTROL), (SoloModifier::RightShift, VK_RSHIFT)] {
            let mut l = solo_logic(m);
            let out = l.handle(down(vk, &[]), Instant::now());
            assert_eq!(out.events, vec![HotkeyEvent::PushToTalkDown]);
            assert!(!out.mask, "{m:?}");
        }
    }

    #[test]
    fn right_win_push_to_talk_masks_the_start_menu() {
        let mut l = solo_logic(SoloModifier::RightCommand);
        let out = l.handle(down(VK_RWIN, &[]), Instant::now());
        assert_eq!(out.events, vec![HotkeyEvent::PushToTalkDown]);
        assert!(out.mask);
    }

    #[test]
    fn fn_cannot_be_push_to_talk() {
        let mut l = solo_logic(SoloModifier::Fn);
        assert!(!l.is_needed());
        l.set_ptt(Some("fn+space".parse().unwrap()));
        assert!(!l.is_needed());
    }

    #[test]
    fn injected_events_never_trigger() {
        let mut l = solo_logic(SoloModifier::RightOption);
        l.set_recording(true);
        let now = Instant::now();
        let injected = RawEvent { flags: LLKHF_INJECTED, ..down(VK_RMENU, &[]) };
        assert_eq!(l.handle(injected, now), Outcome::default());
        let esc = RawEvent { flags: LLKHF_INJECTED, ..down(VK_ESCAPE, &[]) };
        l.handle(esc, now);
        assert_eq!(l.handle(esc, now + Duration::from_millis(50)), Outcome::default());
        // The injected key-down did not count as "already down".
        assert_eq!(l.handle(down(VK_RMENU, &[]), now).events, vec![HotkeyEvent::PushToTalkDown]);
    }

    #[test]
    fn chord_ptt_follows_key_and_modifiers() {
        let mut l = logic();
        l.set_ptt(Some("ctrl+shift+d".parse().unwrap()));
        let now = Instant::now();
        let mods = [VK_LCONTROL, VK_LSHIFT];
        assert!(l.handle(down(KEY_D, &[VK_LCONTROL]), now).events.is_empty());
        l.handle(up(KEY_D, &[VK_LCONTROL]), now);
        assert_eq!(l.handle(down(KEY_D, &mods), now).events, vec![HotkeyEvent::PushToTalkDown]);
        // Auto-repeat is ignored.
        assert!(l.handle(down(KEY_D, &mods), now).events.is_empty());
        assert_eq!(l.handle(up(KEY_D, &mods), now).events, vec![HotkeyEvent::PushToTalkUp]);
    }

    #[test]
    fn chord_ptt_accepts_either_side_of_a_modifier() {
        let mut l = logic();
        l.set_ptt(Some("ctrl+shift+d".parse().unwrap()));
        let out = l.handle(down(KEY_D, &[VK_RCONTROL, VK_RSHIFT]), Instant::now());
        assert_eq!(out.events, vec![HotkeyEvent::PushToTalkDown]);
    }

    #[test]
    fn chord_ptt_needs_exactly_its_modifiers() {
        let mut l = logic();
        l.set_ptt(Some("ctrl+shift+d".parse().unwrap()));
        let out = l.handle(down(KEY_D, &[VK_LCONTROL, VK_LSHIFT, VK_LMENU]), Instant::now());
        assert!(out.events.is_empty());
    }

    #[test]
    fn chord_ptt_releases_when_a_modifier_goes_first() {
        let mut l = logic();
        l.set_ptt(Some("ctrl+shift+d".parse().unwrap()));
        let now = Instant::now();
        l.handle(down(KEY_D, &[VK_LCONTROL, VK_LSHIFT]), now);
        let out = l.handle(up(VK_LSHIFT, &[VK_LCONTROL, VK_LSHIFT]), now);
        assert_eq!(out.events, vec![HotkeyEvent::PushToTalkUp]);
        // The later key-up must not release twice.
        assert!(l.handle(up(KEY_D, &[VK_LCONTROL]), now).events.is_empty());
    }

    #[test]
    fn chord_ptt_with_altgr_counts_as_alt() {
        let mut l = logic();
        l.set_ptt(Some("opt+d".parse().unwrap()));
        let now = Instant::now();
        l.handle(altgr_ctrl(true, &[]), now);
        l.handle(down(VK_RMENU, &[VK_LCONTROL]), now);
        let out = l.handle(down(KEY_D, &[VK_LCONTROL, VK_RMENU]), now);
        assert_eq!(out.events, vec![HotkeyEvent::PushToTalkDown]);
    }

    #[test]
    fn esc_is_ignored_when_not_recording() {
        let mut l = logic();
        let t0 = Instant::now();
        assert!(l.handle(down(VK_ESCAPE, &[]), t0).events.is_empty());
        l.handle(up(VK_ESCAPE, &[]), t0);
        assert!(l.handle(down(VK_ESCAPE, &[]), t0 + Duration::from_millis(100)).events.is_empty());
    }

    #[test]
    fn double_esc_cancels_while_recording() {
        let mut l = logic();
        l.set_recording(true);
        let t0 = Instant::now();
        assert!(l.handle(down(VK_ESCAPE, &[]), t0).events.is_empty());
        l.handle(up(VK_ESCAPE, &[]), t0);
        let out = l.handle(down(VK_ESCAPE, &[]), t0 + Duration::from_millis(180));
        assert_eq!(out.events, vec![HotkeyEvent::Cancel]);
    }

    #[test]
    fn esc_repeat_and_modified_esc_do_not_count() {
        let mut l = logic();
        l.set_recording(true);
        let t0 = Instant::now();
        l.handle(down(VK_ESCAPE, &[]), t0);
        // Held Esc: a second key-down without a key-up.
        assert!(l.handle(down(VK_ESCAPE, &[]), t0 + Duration::from_millis(50)).events.is_empty());
        l.handle(up(VK_ESCAPE, &[]), t0);
        for modifier in [VK_LCONTROL, VK_RMENU, VK_LWIN] {
            assert!(l.handle(down(VK_ESCAPE, &[modifier]), t0 + Duration::from_millis(60)).events.is_empty());
            l.handle(up(VK_ESCAPE, &[modifier]), t0);
        }
    }

    #[test]
    fn shift_esc_still_counts() {
        let mut l = logic();
        l.configure_esc(true, Duration::from_millis(400));
        l.set_recording(true);
        assert_eq!(l.handle(down(VK_ESCAPE, &[VK_LSHIFT]), Instant::now()).events, vec![HotkeyEvent::Cancel]);
    }

    #[test]
    fn esc_with_altgr_held_is_not_plain() {
        let mut l = logic();
        l.configure_esc(true, Duration::from_millis(400));
        l.set_recording(true);
        let now = Instant::now();
        l.handle(altgr_ctrl(true, &[]), now);
        l.handle(down(VK_RMENU, &[VK_LCONTROL]), now);
        assert!(l.handle(down(VK_ESCAPE, &[VK_LCONTROL, VK_RMENU]), now).events.is_empty());
    }

    #[test]
    fn single_esc_setting() {
        let mut l = logic();
        l.configure_esc(true, Duration::from_millis(400));
        l.set_recording(true);
        assert_eq!(l.handle(down(VK_ESCAPE, &[]), Instant::now()).events, vec![HotkeyEvent::Cancel]);
    }

    #[test]
    fn stopping_recording_forgets_a_half_finished_double_esc() {
        let mut l = logic();
        let t0 = Instant::now();
        l.set_recording(true);
        l.handle(down(VK_ESCAPE, &[]), t0);
        l.handle(up(VK_ESCAPE, &[]), t0);
        l.set_recording(false);
        l.set_recording(true);
        assert!(l.handle(down(VK_ESCAPE, &[]), t0 + Duration::from_millis(100)).events.is_empty());
    }

    #[test]
    fn release_held_reports_a_pending_release_once() {
        let mut l = solo_logic(SoloModifier::RightOption);
        l.handle(down(VK_RMENU, &[]), Instant::now());
        assert_eq!(l.release_held(), Some(HotkeyEvent::PushToTalkUp));
        assert_eq!(l.release_held(), None);
    }

    #[test]
    fn changing_the_binding_releases_a_held_ptt() {
        let mut l = solo_logic(SoloModifier::RightOption);
        l.handle(down(VK_RMENU, &[]), Instant::now());
        assert_eq!(l.set_ptt(None), Some(HotkeyEvent::PushToTalkUp));
        assert!(!l.is_needed());
    }

    #[test]
    fn capture_returns_a_chord_on_key_down() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        l.handle(down(VK_LMENU, &[]), now);
        let out = l.handle(down(VK_SPACE, &[VK_LMENU]), now);
        assert_eq!(out.captured, Some(Hotkey::toggle_default()));
        assert!(!l.is_needed(), "capture ends after one result");
        // Not delivered twice.
        assert_eq!(l.handle(down(VK_SPACE, &[VK_LMENU]), now).captured, None);
    }

    #[test]
    fn capture_maps_the_windows_key_to_command() {
        let mut l = logic();
        l.set_capturing(true);
        let out = l.handle(down(KEY_D, &[VK_LWIN, VK_LCONTROL]), Instant::now());
        assert_eq!(out.captured, Some("ctrl+cmd+d".parse().unwrap()));
    }

    #[test]
    fn capture_ignores_bare_letters_but_takes_f_keys() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        assert_eq!(l.handle(down(0x41, &[]), now).captured, None);
        assert_eq!(l.handle(down(VK_ESCAPE, &[]), now).captured, None);
        let out = l.handle(down(VK_F1 + 4, &[]), now);
        assert_eq!(out.captured, Some(Hotkey::Chord { modifiers: Modifiers::default(), key: Key::F(5) }));
    }

    #[test]
    fn capture_ignores_keys_sayso_cannot_name() {
        let mut l = logic();
        l.set_capturing(true);
        // Ctrl+Period: no Sayso key for the period.
        assert_eq!(l.handle(down(0xBE, &[VK_LCONTROL]), Instant::now()).captured, None);
        assert!(l.is_needed(), "the recorder stays open");
    }

    #[test]
    fn capture_returns_a_solo_modifier_after_release() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        let pressed = l.handle(down(VK_RMENU, &[]), now);
        assert_eq!(pressed.captured, None);
        assert!(pressed.mask, "the recorder must not open the menu bar");
        // Auto-repeat while held keeps the candidate.
        l.handle(down(VK_RMENU, &[VK_RMENU]), now);
        let out = l.handle(up(VK_RMENU, &[VK_RMENU]), now);
        assert_eq!(out.captured, Some(Hotkey::Solo(SoloModifier::RightOption)));
    }

    #[test]
    fn capture_takes_altgr_as_right_alt_and_never_records_ctrl_alt() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        l.handle(altgr_ctrl(true, &[]), now);
        l.handle(down(VK_RMENU, &[VK_LCONTROL]), now);
        l.handle(altgr_ctrl(false, &[VK_LCONTROL, VK_RMENU]), now);
        let out = l.handle(up(VK_RMENU, &[VK_RMENU]), now);
        assert_eq!(out.captured, Some(Hotkey::Solo(SoloModifier::RightOption)));

        // AltGr+Q is recorded as Alt+Q, not Ctrl+Alt+Q.
        l.set_capturing(true);
        l.handle(altgr_ctrl(true, &[]), now);
        l.handle(down(VK_RMENU, &[VK_LCONTROL]), now);
        let chord = l.handle(down(KEY_Q, &[VK_LCONTROL, VK_RMENU]), now);
        assert_eq!(chord.captured, Some("opt+q".parse().unwrap()));
    }

    #[test]
    fn capture_records_a_real_left_ctrl_with_alt() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        l.handle(down(VK_LCONTROL, &[]), now);
        l.handle(down(VK_LMENU, &[VK_LCONTROL]), now);
        let chord = l.handle(down(KEY_Q, &[VK_LCONTROL, VK_LMENU]), now);
        assert_eq!(chord.captured, Some("ctrl+opt+q".parse().unwrap()));
    }

    #[test]
    fn capture_does_not_take_a_modifier_that_became_part_of_a_chord() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        l.handle(down(VK_RMENU, &[]), now);
        // Alt+Space completes the chord, so the later release must not also fire.
        let chord = l.handle(down(VK_SPACE, &[VK_RMENU]), now);
        assert!(matches!(chord.captured, Some(Hotkey::Chord { .. })));
        l.set_capturing(true);
        assert_eq!(l.handle(up(VK_RMENU, &[VK_RMENU]), now).captured, None);
    }

    #[test]
    fn capture_ignores_a_solo_pressed_while_another_modifier_is_held() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        l.handle(down(VK_LWIN, &[]), now);
        let pressed = l.handle(down(VK_RMENU, &[VK_LWIN]), now);
        assert!(!pressed.mask);
        assert_eq!(l.handle(up(VK_RMENU, &[VK_LWIN, VK_RMENU]), now).captured, None);
    }

    #[test]
    fn capture_ignores_left_shift_ctrl_and_win_alone() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        for vk in [VK_LSHIFT, VK_LCONTROL, VK_LWIN] {
            l.handle(down(vk, &[]), now);
            assert_eq!(l.handle(up(vk, &[vk]), now).captured, None, "{vk:#x}");
        }
    }

    #[test]
    fn capture_ignores_a_second_solo_in_a_row() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        l.handle(down(VK_RMENU, &[]), now);
        l.handle(down(VK_RSHIFT, &[VK_RMENU]), now);
        assert_eq!(l.handle(up(VK_RSHIFT, &[VK_RMENU, VK_RSHIFT]), now).captured, None);
        assert_eq!(l.handle(up(VK_RMENU, &[VK_RMENU]), now).captured, None);
    }

    #[test]
    fn capture_suppresses_ptt_and_cancel_events() {
        let mut l = solo_logic(SoloModifier::RightOption);
        l.set_recording(true);
        l.set_capturing(true);
        let now = Instant::now();
        assert!(l.handle(down(VK_RMENU, &[]), now).events.is_empty());
        assert!(l.handle(down(VK_ESCAPE, &[VK_RMENU]), now).events.is_empty());
    }

    #[test]
    fn hook_is_needed_for_ptt_recording_or_capture() {
        let mut l = logic();
        assert!(!l.is_needed());
        l.set_recording(true);
        assert!(l.is_needed());
        l.set_recording(false);
        l.set_capturing(true);
        assert!(l.is_needed());
        l.set_capturing(false);
        l.set_ptt(Some("right_shift".parse().unwrap()));
        assert!(l.is_needed());
    }
}
