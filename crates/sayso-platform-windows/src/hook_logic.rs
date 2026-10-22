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

use crate::enter_send::EnterSend;
use crate::esc::{EscDetector, EscTake};
use crate::keymap::{self, ModKeys, VK_ESCAPE, VK_LCONTROL, VK_RETURN};
use sayso_core::hotkey::{Hotkey, Key, ModifierCapture, ModifierSet, Modifiers};
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
    /// Keep the event from the app (Send with Enter, taking Esc).
    pub swallow: bool,
}

/// The push-to-talk binding as the hook sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ptt {
    Solo { vk: u16 },
    ModifierOnly(ModifierSet),
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

    fn contains(&self, vk: u16) -> bool {
        self.0[(vk as usize >> 6) & 3] & (1u64 << (vk & 63)) != 0
    }

    fn has_regular_key(&self) -> bool {
        self.0.iter().enumerate().any(|(word, bits)| {
            let mut bits = *bits;
            while bits != 0 {
                let vk = (word * 64) as u16 + bits.trailing_zeros() as u16;
                if !keymap::is_modifier(vk) { return true; }
                bits &= bits - 1;
            }
            false
        })
    }

    fn release(&mut self, vk: u16) {
        self.0[(vk as usize >> 6) & 3] &= !(1u64 << (vk & 63));
    }
}

#[derive(Debug)]
pub struct HookLogic {
    ptt: Option<Ptt>,
    toggle_modifiers: Option<ModifierSet>,
    ptt_down: bool,
    pointer_available: bool,
    recording: bool,
    esc: EscDetector,
    /// The key recorder is open.
    capturing: bool,
    /// Modifiers pressed without a regular key in between.
    modifier_capture: ModifierCapture,
    pressed: KeySet,
    /// The fake Left Ctrl of AltGr is down.
    altgr_ctrl: bool,
    enter: EnterSend,
    esc_take: EscTake,
}

/// No Win, Ctrl, or Alt is down. Only a plain Esc cancels.
fn is_plain(held: ModKeys) -> bool {
    let m = held.modifiers();
    !(m.command || m.control || m.option)
}

/// Whether this is the fake Left Ctrl that the keyboard layout sends with AltGr.
pub fn is_altgr_ctrl(event: &RawEvent) -> bool {
    event.vk == VK_LCONTROL && event.scan & ALTGR_CTRL_SCAN != 0
}

impl HookLogic {
    pub fn new(esc: EscDetector) -> Self {
        Self {
            ptt: None,
            toggle_modifiers: None,
            ptt_down: false,
            pointer_available: true,
            recording: false,
            esc,
            capturing: false,
            modifier_capture: ModifierCapture::default(),
            pressed: KeySet::default(),
            altgr_ctrl: false,
            enter: EnterSend::default(),
            esc_take: EscTake::default(),
        }
    }

    /// True when the hook has any reason to be installed. A taken Enter or Esc
    /// that is still down keeps it, so its repeats and release reach no app.
    pub fn is_needed(&self) -> bool {
        self.ptt.is_some() || self.toggle_modifiers.is_some() || self.recording || self.capturing || self.enter.is_held() || self.esc_take.is_held()
    }

    /// Set the push-to-talk binding. Returns a release event when a held
    /// push-to-talk is dropped by the change. Hotkeys with Fn cannot work on
    /// Windows and leave push-to-talk off.
    pub fn set_ptt(&mut self, hotkey: Option<Hotkey>) -> Option<HotkeyEvent> {
        let released = self.release_held();
        self.ptt = hotkey.and_then(|h| match h {
            Hotkey::ModifierOnly(keys) => keymap::can_detect_modifiers(keys).then_some(Ptt::ModifierOnly(keys)),
            Hotkey::Solo(solo) => keymap::solo_vk(solo).map(|vk| Ptt::Solo { vk }),
            Hotkey::Chord { modifiers, .. } if modifiers.function => None,
            Hotkey::Chord { modifiers, key } => keymap::vk(key).map(|vk| Ptt::Chord { vk, modifiers }),
        });
        released
    }

    pub fn set_toggle_modifiers(&mut self, keys: Option<ModifierSet>) {
        self.toggle_modifiers = keys.filter(|keys| keymap::can_detect_modifiers(*keys));
        self.modifier_capture.clear();
    }


    pub fn needs_mouse(&self) -> bool {
        self.toggle_modifiers.is_some()
    }

    pub fn set_pointer_available(&mut self, available: bool) {
        if self.pointer_available != available {
            self.modifier_capture.clear();
        }
        self.pointer_available = available;
    }

    pub fn pointer_available(&self) -> bool {
        self.pointer_available
    }

    pub fn configure_esc(&mut self, single: bool, window: std::time::Duration) {
        self.esc.configure(single, window);
    }

    pub fn set_recording(&mut self, recording: bool) {
        self.recording = recording;
        self.esc.reset();
        self.enter.set_recording(recording);
        self.esc_take.set_recording(recording);
    }

    pub fn set_enter_sends(&mut self, on: bool) {
        self.enter.set_on(on);
    }

    pub fn set_take_escape(&mut self, on: bool) {
        self.esc_take.set_on(on);
    }

    pub fn set_capturing(&mut self, capturing: bool) {
        self.capturing = capturing;
        self.modifier_capture.clear();
    }

    /// Forget a held push-to-talk, for example when the hook stops.
    pub fn release_held(&mut self) -> Option<HotkeyEvent> {
        self.modifier_capture.clear();
        std::mem::take(&mut self.ptt_down).then_some(HotkeyEvent::PushToTalkUp)
    }

    /// A restarted hook cannot receive releases missed by the previous hook.
    pub fn reset_keys(&mut self) -> Option<HotkeyEvent> {
        self.pressed = KeySet::default();
        self.altgr_ctrl = false;
        self.release_held()
    }

    /// Mouse buttons and wheels make the held modifiers a gesture, not a tap.
    pub fn cancel_modifier_tap(&mut self) {
        self.modifier_capture.clear();
    }

    /// Recover keys whose releases happened outside this desktop. A swallowed
    /// down never enters async state: only its observed release can clear it.
    /// Async state is pre-event in a low-level hook: keep the current regular
    /// key, but query even the current modifier. Its up state distinguishes a
    /// fresh press after a lost release from repeat. Modifiers are never taken.
    pub fn prune_pressed(&mut self, current: u16, mut is_down: impl FnMut(u16) -> bool) {
        for vk in 0..256 {
            let swallowed = (vk == VK_RETURN && self.enter.is_held())
                || (vk == VK_ESCAPE && self.esc_take.is_held());
            let modifier = keymap::is_modifier(vk);
            if (vk != current || modifier) && !swallowed && self.pressed.contains(vk) && !is_down(vk) {
                self.pressed.release(vk);
                if modifier {
                    self.modifier_capture.clear();
                }
            }
        }
    }

    /// The modifier keys after this event, without the fake Ctrl of AltGr.
    fn modifiers_after(&self, event: &RawEvent) -> ModKeys {
        let held = event.held.with(event.vk, event.down);
        if self.altgr_ctrl {
            held.with(VK_LCONTROL, self.pressed.contains(VK_LCONTROL))
        } else {
            held
        }
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
        // Return and keypad Enter; the logic below still sees the event.
        if event.vk == VK_RETURN {
            let decision = self.enter.key(event.down, repeat);
            out.swallow = decision.swallow;
            if decision.submit {
                out.events.push(HotkeyEvent::Submit);
            }
        }
        if event.vk == VK_ESCAPE {
            out.swallow = self.esc_take.key(event.down, repeat, is_plain(held));
        }
        if keymap::is_modifier(event.vk) {
            let modifier_keys = keymap::modifier_keys(held);
            if let Some(keys) = self.toggle_modifiers.filter(|_| self.pointer_available) {
                if event.down && !repeat {
                    if !self.pressed.has_regular_key() {
                        self.modifier_capture.press_tap(modifier_keys, now);
                    }
                    out.mask = modifier_keys == keys && self.modifier_capture.is_active()
                        && keys.iter().filter_map(keymap::solo_vk).any(keymap::release_has_meaning);
                } else if !event.down
                    && keymap::solo_from_vk(event.vk).and_then(|m| self.modifier_capture.release_tap(m, now)).and_then(Hotkey::modifier_keys) == Some(keys)
                {
                    out.events.push(HotkeyEvent::Toggle);
                }
            }
            match self.ptt {
                Some(Ptt::ModifierOnly(keys)) => {
                    let down = modifier_keys == keys;
                    if down && !self.ptt_down {
                        out.mask = keys.iter().filter_map(keymap::solo_vk).any(keymap::release_has_meaning);
                    }
                    self.set_ptt_down(down, &mut out.events);
                }
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
            self.modifier_capture.clear();
            if repeat {
                return out;
            }
            if let Some(Ptt::Chord { vk, modifiers }) = self.ptt
                && event.vk == vk
                && held.modifiers() == modifiers
            {
                self.set_ptt_down(true, &mut out.events);
            }
            if event.vk == VK_ESCAPE && self.recording && is_plain(held) && self.esc.press(now) {
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

    /// Chords complete on key down; modifier-only combinations complete on
    /// the first release without a regular key in between.
    fn capture(&mut self, event: &RawEvent, repeat: bool, held: ModKeys) -> Outcome {
        let mut out = Outcome::default();
        let done = |this: &mut Self, out: &mut Outcome, hotkey| {
            this.capturing = false;
            this.modifier_capture.clear();
            out.captured = Some(hotkey);
        };
        let solo = keymap::solo_from_vk(event.vk);
        match (keymap::is_modifier(event.vk), event.down) {
            (false, true) => {
                self.modifier_capture.clear();
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
                if solo.is_some() {
                    self.modifier_capture.press(keymap::modifier_keys(held));
                } else {
                    self.modifier_capture.clear();
                }
                out.mask = self.modifier_capture.is_active() && keymap::release_has_meaning(event.vk);
            }
            (true, false) => {
                if let Some(hk) = solo.and_then(|modifier| self.modifier_capture.release(modifier)) {
                    done(self, &mut out, hk);
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
    use sayso_core::hotkey::SoloModifier;
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
    fn regression_long_hold_and_repeat_do_not_toggle() {
        let now = Instant::now();
        let mut l = logic();
        l.set_toggle_modifiers("left_shift".parse::<Hotkey>().unwrap().modifier_keys());
        l.handle(down(VK_LSHIFT, &[]), now);
        l.handle(down(VK_LSHIFT, &[VK_LSHIFT]), now + Duration::from_millis(390));
        assert!(l.handle(up(VK_LSHIFT, &[VK_LSHIFT]), now + Duration::from_millis(401)).events.is_empty());
        l.handle(down(VK_LSHIFT, &[]), now + Duration::from_secs(1));
        assert_eq!(l.handle(up(VK_LSHIFT, &[VK_LSHIFT]), now + Duration::from_millis(1100)).events, vec![HotkeyEvent::Toggle]);
    }

    #[test]
    fn regression_modifier_expansion_does_not_renew_tap_time() {
        let now = Instant::now();
        let mut l = logic();
        l.set_toggle_modifiers("left_shift+left_control".parse::<Hotkey>().unwrap().modifier_keys());
        l.handle(down(VK_LSHIFT, &[]), now);
        l.handle(down(VK_LCONTROL, &[VK_LSHIFT]), now + Duration::from_millis(390));
        assert!(l.handle(up(VK_LCONTROL, &[VK_LSHIFT, VK_LCONTROL]), now + Duration::from_millis(401)).events.is_empty());
    }

    #[test]
    fn regression_chord_ptt_after_missed_ctrl_keyup() {
        let now = Instant::now();
        for (binding, expected) in [("shift+d", vec![HotkeyEvent::PushToTalkDown]), ("ctrl+shift+d", vec![])] {
            let mut l = logic();
            l.set_ptt(Some(binding.parse().unwrap()));
            l.handle(down(VK_LCONTROL, &[]), now);
            // Ctrl's release happened on the secure desktop; async state is now up.
            assert_eq!(l.handle(down(KEY_D, &[VK_LSHIFT]), now).events, expected, "{binding}");
        }
    }

    #[test]
    fn regression_esc_cancel_after_missed_ctrl_keyup() {
        let now = Instant::now();
        let mut l = logic();
        l.configure_esc(true, Duration::from_millis(400));
        l.set_take_escape(true);
        l.set_recording(true);
        l.handle(down(VK_LCONTROL, &[]), now);
        let out = l.handle(down(VK_ESCAPE, &[]), now);
        assert_eq!(out.events, vec![HotkeyEvent::Cancel]);
        assert!(out.swallow);
    }

    #[test]
    fn regression_recorder_keeps_unbounded_hold() {
        let now = Instant::now();
        let mut l = logic();
        l.set_capturing(true);
        l.handle(down(VK_LSHIFT, &[]), now);
        assert_eq!(l.handle(up(VK_LSHIFT, &[VK_LSHIFT]), now + Duration::from_secs(2)).captured, Some("left_shift".parse().unwrap()));
    }

    #[test]
    fn regression_mouse_cancels_tap_and_next_tap_recovers() {
        let now = Instant::now();
        let mut l = logic();
        l.set_toggle_modifiers("left_shift".parse::<Hotkey>().unwrap().modifier_keys());
        l.handle(down(VK_LSHIFT, &[]), now);
        l.cancel_modifier_tap();
        assert!(l.handle(up(VK_LSHIFT, &[VK_LSHIFT]), now).events.is_empty());
        l.handle(down(VK_LSHIFT, &[]), now);
        assert_eq!(l.handle(up(VK_LSHIFT, &[VK_LSHIFT]), now).events, vec![HotkeyEvent::Toggle]);
    }

    #[test]
    fn regression_prune_first_win_or_ctrl_tap_after_secure_desktop() {
        let now = Instant::now();
        for (vk, binding) in [(VK_LWIN, "left_command"), (VK_LCONTROL, "left_control")] {
            let mut l = logic();
            l.set_toggle_modifiers(binding.parse::<Hotkey>().unwrap().modifier_keys());
            l.handle(down(vk, &[]), now);
            l.handle(down(0x4C, &[vk]), now);
            // Both releases were lost. Pre-event async state is up even for
            // the current modifier: this is its first NEW press, not repeat.
            l.prune_pressed(vk, |_| false);
            l.handle(down(vk, &[]), now);
            assert_eq!(l.handle(up(vk, &[vk]), now).events, vec![HotkeyEvent::Toggle], "{binding}");
        }
    }

    #[test]
    fn regression_prune_preserves_real_ctrl_with_altgr_but_not_stale_ctrl() {
        let now = Instant::now();
        for real_ctrl in [false, true] {
            let mut l = logic();
            l.set_capturing(true);
            l.handle(down(VK_LCONTROL, &[]), now);
            // The synthetic Ctrl down is observed before Windows updates its
            // async state. A real Ctrl still down is distinguishable here.
            l.prune_pressed(VK_LCONTROL, |_| real_ctrl);
            l.handle(altgr_ctrl(true, &[]), now);
            l.prune_pressed(VK_RMENU, |vk| vk == VK_LCONTROL);
            l.handle(down(VK_RMENU, &[VK_LCONTROL]), now);
            let out = l.handle(down(KEY_Q, &[VK_LCONTROL, VK_RMENU]), now);
            assert_eq!(out.captured, Some(if real_ctrl { "ctrl+opt+q" } else { "opt+q" }.parse().unwrap()));
        }
    }

    #[test]
    fn regression_prune_does_not_renew_a_genuinely_held_modifier() {
        let now = Instant::now();
        let mut l = logic();
        l.set_toggle_modifiers("left_shift".parse::<Hotkey>().unwrap().modifier_keys());
        l.handle(down(VK_LSHIFT, &[]), now);
        l.prune_pressed(VK_LSHIFT, |vk| vk == VK_LSHIFT);
        l.handle(down(VK_LSHIFT, &[VK_LSHIFT]), now + Duration::from_millis(390));
        assert!(l.handle(up(VK_LSHIFT, &[VK_LSHIFT]), now + Duration::from_millis(401)).events.is_empty());
    }

    #[test]
    fn regression_prune_recovers_after_win_l_without_dropping_held_keys() {
        let now = Instant::now();
        for still_down in [false, true] {
            let mut l = logic();
            l.set_toggle_modifiers("left_shift".parse::<Hotkey>().unwrap().modifier_keys());
            l.handle(down(VK_LWIN, &[]), now);
            l.handle(down(0x4C, &[VK_LWIN]), now);
            l.prune_pressed(VK_LSHIFT, |vk| {
                assert!(vk == 0x4C || vk == VK_LWIN, "only tracked keys are queried");
                vk == 0x4C && still_down
            });
            l.handle(down(VK_LSHIFT, &[]), now);
            let expected = if still_down { vec![] } else { vec![HotkeyEvent::Toggle] };
            assert_eq!(l.handle(up(VK_LSHIFT, &[VK_LSHIFT]), now).events, expected);
        }
    }

    #[test]
    fn regression_prune_excludes_current_pre_event_key_state() {
        let now = Instant::now();
        let mut l = logic();
        l.set_ptt(Some("shift+d".parse().unwrap()));
        assert_eq!(l.handle(down(KEY_D, &[VK_LSHIFT]), now).events, vec![HotkeyEvent::PushToTalkDown]);
        // Async state can still be up for the current down event. Keep repeat tracking.
        l.prune_pressed(KEY_D, |_| panic!("current event must not be queried"));
        assert!(l.handle(down(KEY_D, &[VK_LSHIFT]), now).events.is_empty());
        assert!(l.pressed.contains(KEY_D));
        assert_eq!(l.handle(up(KEY_D, &[VK_LSHIFT]), now).events, vec![HotkeyEvent::PushToTalkUp]);
    }

    #[test]
    fn modifier_toggle_rejects_both_key_orders() {
        let now = Instant::now();
        for first in 0..2 {
            for last in 0..2 {
                let mut l = logic();
                l.set_toggle_modifiers("left_shift".parse::<Hotkey>().unwrap().modifier_keys());
                let press = [down(KEY_D, if first == 1 { &[VK_LSHIFT] } else { &[] }), down(VK_LSHIFT, &[])];
                let release = [up(KEY_D, if last == 0 { &[VK_LSHIFT] } else { &[] }), up(VK_LSHIFT, &[VK_LSHIFT])];
                for event in [press[first], press[1 - first], release[last], release[1 - last]] {
                    assert!(l.handle(event, now).events.is_empty());
                }
                assert!(l.handle(down(VK_LSHIFT, &[]), now).events.is_empty());
                assert_eq!(l.handle(up(VK_LSHIFT, &[VK_LSHIFT]), now).events, vec![HotkeyEvent::Toggle]);
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
            let mut held = Vec::new();
            let mut events = Vec::new();
            for modifier in keys.iter() {
                let vk = keymap::solo_vk(modifier).unwrap();
                events.extend(l.handle(down(vk, &held), now).events);
                held.push(vk);
            }
            events.extend(l.handle(down(KEY_D, &held), now).events);
            events.extend(l.handle(up(KEY_D, &held), now).events);
            for modifier in keys.iter() {
                let vk = keymap::solo_vk(modifier).unwrap();
                events.extend(l.handle(up(vk, &held), now).events);
                held.retain(|key| *key != vk);
            }
            assert!(events.is_empty(), "{binding} fired for a normal shortcut");
        }
    }

    #[test]
    fn restarted_hook_forgets_ctrl_after_capture() {
        let now = Instant::now();
        let mut l = logic();
        l.set_capturing(true);
        l.handle(down(VK_LCONTROL, &[]), now);
        assert_eq!(l.handle(down(KEY_D, &[VK_LCONTROL]), now).captured, Some("ctrl+d".parse().unwrap()));
        // The hook stopped before Ctrl was released. Its replacement starts fresh.
        l.set_capturing(true);
        l.reset_keys();
        assert!(l.handle(down(VK_RSHIFT, &[]), now).captured.is_none());
        assert_eq!(l.handle(up(VK_RSHIFT, &[VK_RSHIFT]), now).captured, Some("right_shift".parse().unwrap()));
        l.set_toggle_modifiers(Some("right_shift".parse::<Hotkey>().unwrap().modifier_keys().unwrap()));
        assert!(l.handle(down(VK_RSHIFT, &[]), now).events.is_empty());
        assert_eq!(l.handle(up(VK_RSHIFT, &[VK_RSHIFT]), now).events, vec![HotkeyEvent::Toggle]);
    }

    #[test]
    fn every_detectable_modifier_combination_records_and_triggers() {
        let available = SoloModifier::ALL.into_iter().filter(|m| keymap::solo_vk(*m).is_some()).collect::<Vec<_>>();
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
                        let mut held = Vec::new();
                        for (i, modifier) in order.iter().enumerate() {
                            let vk = keymap::solo_vk(*modifier).unwrap();
                            let event = down(vk, &held);
                            let out = active.handle(event, now);
                            assert_eq!(capture.handle(event, now).captured, None);
                            held.push(vk);
                            let expected = if !toggle && i + 1 == order.len() {
                                vec![HotkeyEvent::PushToTalkDown]
                            } else { Vec::new() };
                            assert_eq!(out.events, expected, "{binding}, reverse={reverse}, toggle={toggle}");
                            if toggle {
                                let masks_release = selected.iter().filter_map(|m| keymap::solo_vk(*m)).any(keymap::release_has_meaning);
                                assert_eq!(out.mask, i + 1 == order.len() && masks_release, "{binding}");
                            }
                        }
                        let event = up(keymap::solo_vk(released).unwrap(), &held);
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
        for binding in ["left_shift", "left_control+left_command"] {
            let keys = binding.parse::<Hotkey>().unwrap().modifier_keys().unwrap();
            let mut l = logic();
            l.set_toggle_modifiers(Some(keys));
            assert!(l.handle(down(VK_LMENU, &[]), now).events.is_empty());
            let mut held = vec![VK_LMENU];
            for modifier in keys.iter() {
                let vk = keymap::solo_vk(modifier).unwrap();
                assert!(l.handle(down(vk, &held), now).events.is_empty());
                held.push(vk);
            }
            assert!(l.handle(up(VK_LMENU, &held), now).events.is_empty());
            held.retain(|vk| *vk != VK_LMENU);
            let required = keymap::solo_vk(keys.iter().next().unwrap()).unwrap();
            assert!(l.handle(up(required, &held), now).events.is_empty());
        }
    }

    #[test]
    fn modifiers_the_system_cannot_see_do_not_install_a_binding() {
        let mut l = logic();
        for binding in ["fn", "fn+left_command"] {
            assert_eq!(l.set_ptt(Some(binding.parse().unwrap())), None);
            assert!(!l.is_needed());
        }
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
    fn enter_while_recording_submits_and_reaches_no_app() {
        let mut l = solo_logic(SoloModifier::RightOption);
        l.set_enter_sends(true);
        let now = Instant::now();
        l.handle(down(VK_RMENU, &[]), now);
        l.set_recording(true);
        // Right Alt of push-to-talk is still down.
        let pressed = l.handle(down(VK_RETURN, &[VK_RMENU]), now);
        assert_eq!(pressed.events, vec![HotkeyEvent::Submit]);
        assert!(pressed.swallow);
        let repeat = l.handle(down(VK_RETURN, &[VK_RMENU]), now);
        assert!(repeat.events.is_empty());
        assert!(repeat.swallow);
        let released = l.handle(up(VK_RETURN, &[VK_RMENU]), now);
        assert!(released.swallow);
        assert_eq!(l.handle(up(VK_RMENU, &[VK_RMENU]), now).events, vec![HotkeyEvent::PushToTalkUp]);
    }

    #[test]
    fn a_taken_enter_keeps_the_hook_until_it_is_up() {
        let mut l = logic();
        l.set_enter_sends(true);
        l.set_recording(true);
        let now = Instant::now();
        assert_eq!(l.handle(down(VK_RETURN, &[]), now).events, vec![HotkeyEvent::Submit]);
        l.set_recording(false);
        assert!(l.is_needed(), "Enter is still down");
        assert!(l.handle(down(VK_RETURN, &[]), now).swallow, "auto-repeat after the stop");
        assert!(l.handle(up(VK_RETURN, &[]), now).swallow);
        assert!(!l.is_needed());
    }

    #[test]
    fn enter_goes_to_the_app_with_the_setting_off_or_when_not_recording() {
        let mut l = logic();
        l.set_recording(true);
        let now = Instant::now();
        let off = l.handle(down(VK_RETURN, &[]), now);
        assert!(off.events.is_empty() && !off.swallow);
        l.handle(up(VK_RETURN, &[]), now);
        l.set_enter_sends(true);
        l.set_recording(false);
        let idle = l.handle(down(VK_RETURN, &[]), now);
        assert!(idle.events.is_empty() && !idle.swallow);
    }

    #[test]
    fn a_double_esc_cancels_and_both_presses_reach_no_app() {
        let mut l = logic();
        l.set_take_escape(true);
        l.set_recording(true);
        let t0 = Instant::now();
        let first = l.handle(down(VK_ESCAPE, &[]), t0);
        assert!(first.events.is_empty() && first.swallow);
        assert!(l.handle(up(VK_ESCAPE, &[]), t0).swallow);
        let second = l.handle(down(VK_ESCAPE, &[]), t0 + Duration::from_millis(180));
        assert_eq!(second.events, vec![HotkeyEvent::Cancel]);
        assert!(second.swallow);
        l.set_recording(false);
        assert!(l.is_needed(), "Esc is still down");
        assert!(l.handle(up(VK_ESCAPE, &[]), t0).swallow);
        assert!(!l.is_needed());
    }

    #[test]
    fn esc_goes_to_the_app_with_the_setting_off_modified_or_when_not_recording() {
        let mut l = logic();
        l.set_recording(true);
        let now = Instant::now();
        assert!(!l.handle(down(VK_ESCAPE, &[]), now).swallow, "the setting is off");
        l.handle(up(VK_ESCAPE, &[]), now);
        l.set_take_escape(true);
        assert!(!l.handle(down(VK_ESCAPE, &[VK_LCONTROL]), now).swallow, "Ctrl+Esc opens Start");
        l.handle(up(VK_ESCAPE, &[VK_LCONTROL]), now);
        l.set_recording(false);
        assert!(!l.handle(down(VK_ESCAPE, &[]), now).swallow, "not recording");
    }

    #[test]
    fn our_own_enter_is_never_swallowed() {
        let mut l = logic();
        l.set_enter_sends(true);
        l.set_recording(true);
        let injected = RawEvent { vk: VK_RETURN, scan: 0, flags: LLKHF_INJECTED, down: true, held: ModKeys::of(&[]) };
        assert_eq!(l.handle(injected, Instant::now()), Outcome::default());
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
        l.handle(down(VK_LCONTROL, &[]), now);
        let out = l.handle(down(VK_SPACE, &[VK_LCONTROL]), now);
        assert_eq!(out.captured, Some(Hotkey::toggle_default()));
        assert!(!l.is_needed(), "capture ends after one result");
        // Not delivered twice.
        assert_eq!(l.handle(down(VK_SPACE, &[VK_LCONTROL]), now).captured, None);
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
    fn real_control_with_altgr_and_win_remains_a_three_modifier_binding() {
        let binding: Hotkey = "left_control+left_command+right_option".parse().unwrap();
        let mut active = logic();
        active.set_ptt(Some(binding));
        let mut capture = logic();
        capture.set_capturing(true);
        let now = Instant::now();
        for event in [down(VK_LCONTROL, &[]), altgr_ctrl(true, &[VK_LCONTROL]), down(VK_LWIN, &[VK_LCONTROL])] {
            assert!(active.handle(event, now).events.is_empty());
            assert_eq!(capture.handle(event, now).captured, None);
        }
        let complete = down(VK_RMENU, &[VK_LCONTROL, VK_LWIN]);
        assert_eq!(active.handle(complete, now).events, vec![HotkeyEvent::PushToTalkDown]);
        capture.handle(complete, now);
        let release = up(VK_RMENU, &[VK_LCONTROL, VK_LWIN, VK_RMENU]);
        assert_eq!(active.handle(release, now).events, vec![HotkeyEvent::PushToTalkUp]);
        assert_eq!(capture.handle(release, now).captured, Some(binding));
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
    fn capture_ignores_modifiers_held_before_recording_starts() {
        let mut l = logic();
        let now = Instant::now();
        l.handle(down(VK_LWIN, &[]), now);
        l.set_capturing(true);
        let pressed = l.handle(down(VK_RMENU, &[VK_LWIN]), now);
        assert!(!pressed.mask);
        assert_eq!(l.handle(up(VK_RMENU, &[VK_LWIN, VK_RMENU]), now).captured, None);
    }

    #[test]
    fn capture_records_left_shift_ctrl_and_win_alone() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        for vk in [VK_LSHIFT, VK_LCONTROL, VK_LWIN] {
            l.set_capturing(true);
            l.handle(down(vk, &[]), now);
            assert_eq!(l.handle(up(vk, &[vk]), now).captured, Some(Hotkey::Solo(solo_from_vk(vk).unwrap())));
        }
    }

    #[test]
    fn capture_records_two_modifiers_together() {
        let mut l = logic();
        l.set_capturing(true);
        let now = Instant::now();
        l.handle(down(VK_RMENU, &[]), now);
        l.handle(down(VK_RSHIFT, &[VK_RMENU]), now);
        assert_eq!(l.handle(up(VK_RSHIFT, &[VK_RMENU, VK_RSHIFT]), now).captured, Some("right_option+right_shift".parse().unwrap()));
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
