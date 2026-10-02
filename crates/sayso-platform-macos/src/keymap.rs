//! Pure mapping between [`sayso_core::hotkey`] types and macOS key codes,
//! `CGEventFlags`, Carbon modifier masks, and `global-hotkey` types.
//!
//! No FFI here, so everything is unit-tested.

use crate::ffi::{FLAG_ALT, FLAG_COMMAND, FLAG_CONTROL, FLAG_SECONDARY_FN, FLAG_SHIFT};
use global_hotkey::hotkey::{Code, HotKey, Modifiers as GhModifiers};
use sayso_core::hotkey::{Hotkey, Key, Modifiers, SoloModifier};

/// Carbon modifier mask bits (`cmdKey`, `shiftKey`, `optionKey`, `controlKey`).
pub const CARBON_CMD: u32 = 1 << 8;
pub const CARBON_SHIFT: u32 = 1 << 9;
pub const CARBON_OPTION: u32 = 1 << 11;
pub const CARBON_CONTROL: u32 = 1 << 12;
pub const CARBON_ALL: u32 = CARBON_CMD | CARBON_SHIFT | CARBON_OPTION | CARBON_CONTROL;

/// The four `CGEventFlags` modifier bits that distinguish chords.
/// Fn is left out on purpose: arrow keys and F keys set it by themselves.
pub const CG_MODIFIER_MASK: u64 = FLAG_SHIFT | FLAG_CONTROL | FLAG_ALT | FLAG_COMMAND;

pub const KC_ESCAPE: u16 = 53;

/// ANSI letter key codes, `a` to `z`.
const LETTER_CODES: [u16; 26] = [
    0, 11, 8, 2, 14, 3, 5, 4, 34, 38, 40, 37, 46, 45, 31, 35, 12, 15, 1, 17, 32, 9, 13, 7, 16, 6,
];
/// Digit key codes, `0` to `9`.
const DIGIT_CODES: [u16; 10] = [29, 18, 19, 20, 21, 23, 22, 26, 28, 25];
/// Function key codes, `F1` to `F20`.
const F_CODES: [u16; 20] = [
    122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113, 106, 64, 79, 80, 90,
];

/// Virtual key code for a key, or `None` when the key is out of range
/// (for example `Letter('é')` or `F(30)`).
pub fn keycode(key: Key) -> Option<u16> {
    match key {
        Key::Space => Some(49),
        Key::Escape => Some(KC_ESCAPE),
        Key::Return => Some(36),
        Key::Tab => Some(48),
        Key::Backspace => Some(51),
        Key::Letter(c) if c.is_ascii_alphabetic() => {
            Some(LETTER_CODES[(c.to_ascii_lowercase() as u8 - b'a') as usize])
        }
        Key::Digit(d) if d <= 9 => Some(DIGIT_CODES[d as usize]),
        Key::F(n) if (1..=20).contains(&n) => Some(F_CODES[n as usize - 1]),
        _ => None,
    }
}

/// Reverse of [`keycode`]. Returns `None` for keys Sayso does not support.
pub fn key_from_keycode(code: u16) -> Option<Key> {
    let fixed = [Key::Space, Key::Escape, Key::Return, Key::Tab, Key::Backspace];
    fixed
        .into_iter()
        .chain((b'a'..=b'z').map(|c| Key::Letter(c as char)))
        .chain((0..=9).map(Key::Digit))
        .chain((1..=20).map(Key::F))
        .find(|k| keycode(*k) == Some(code))
}

/// `CGEventFlags` for a modifier set (includes the Fn bit when `function` is set).
pub fn cg_flags(m: Modifiers) -> u64 {
    let mut f = 0;
    if m.command {
        f |= FLAG_COMMAND;
    }
    if m.option {
        f |= FLAG_ALT;
    }
    if m.control {
        f |= FLAG_CONTROL;
    }
    if m.shift {
        f |= FLAG_SHIFT;
    }
    if m.function {
        f |= FLAG_SECONDARY_FN;
    }
    f
}

/// The four-modifier part of `CGEventFlags` as a [`Modifiers`] (Fn is not read).
pub fn modifiers_from_cg_flags(flags: u64) -> Modifiers {
    Modifiers {
        command: flags & FLAG_COMMAND != 0,
        option: flags & FLAG_ALT != 0,
        control: flags & FLAG_CONTROL != 0,
        shift: flags & FLAG_SHIFT != 0,
        function: false,
    }
}

/// Carbon modifier mask, as used by `CopySymbolicHotKeys` and `RegisterEventHotKey`.
/// Carbon has no Fn bit.
pub fn carbon_mask(m: Modifiers) -> u32 {
    let mut f = 0;
    if m.command {
        f |= CARBON_CMD;
    }
    if m.option {
        f |= CARBON_OPTION;
    }
    if m.control {
        f |= CARBON_CONTROL;
    }
    if m.shift {
        f |= CARBON_SHIFT;
    }
    f
}

/// How to detect one modifier key on its own from `flagsChanged` events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoloSpec {
    pub keycode: u16,
    /// Flag bit that is set while this exact key is down. For left/right keys
    /// this is the device-dependent bit (IOKit `NX_DEVICE*KEYMASK`), so the
    /// other side of the pair does not match. For Fn it is `SecondaryFn`.
    pub down_bit: u64,
}

pub fn solo_spec(m: SoloModifier) -> SoloSpec {
    let (keycode, down_bit) = match m {
        SoloModifier::RightOption => (61, 0x40),
        SoloModifier::LeftOption => (58, 0x20),
        SoloModifier::RightCommand => (54, 0x10),
        SoloModifier::RightControl => (62, 0x2000),
        SoloModifier::RightShift => (60, 0x04),
        SoloModifier::Fn => (63, FLAG_SECONDARY_FN),
    };
    SoloSpec { keycode, down_bit }
}

/// Every solo modifier, for the key recorder.
pub const ALL_SOLO: [SoloModifier; 6] = [
    SoloModifier::RightOption,
    SoloModifier::LeftOption,
    SoloModifier::RightCommand,
    SoloModifier::RightControl,
    SoloModifier::RightShift,
    SoloModifier::Fn,
];

pub fn solo_from_keycode(code: u16) -> Option<SoloModifier> {
    ALL_SOLO.into_iter().find(|m| solo_spec(*m).keycode == code)
}

/// Whether a `flagsChanged` event for `code` shows the solo modifier as down.
pub fn solo_is_down(m: SoloModifier, flags: u64) -> bool {
    flags & solo_spec(m).down_bit != 0
}

// ---------------------------------------------------------------------------
// global-hotkey
// ---------------------------------------------------------------------------

const GH_LETTERS: [Code; 26] = [
    Code::KeyA, Code::KeyB, Code::KeyC, Code::KeyD, Code::KeyE, Code::KeyF, Code::KeyG,
    Code::KeyH, Code::KeyI, Code::KeyJ, Code::KeyK, Code::KeyL, Code::KeyM, Code::KeyN,
    Code::KeyO, Code::KeyP, Code::KeyQ, Code::KeyR, Code::KeyS, Code::KeyT, Code::KeyU,
    Code::KeyV, Code::KeyW, Code::KeyX, Code::KeyY, Code::KeyZ,
];
const GH_DIGITS: [Code; 10] = [
    Code::Digit0, Code::Digit1, Code::Digit2, Code::Digit3, Code::Digit4, Code::Digit5,
    Code::Digit6, Code::Digit7, Code::Digit8, Code::Digit9,
];
const GH_FKEYS: [Code; 20] = [
    Code::F1, Code::F2, Code::F3, Code::F4, Code::F5, Code::F6, Code::F7, Code::F8, Code::F9,
    Code::F10, Code::F11, Code::F12, Code::F13, Code::F14, Code::F15, Code::F16, Code::F17,
    Code::F18, Code::F19, Code::F20,
];

/// Map a chord to a `global-hotkey` hotkey. Solo modifiers and chords with the
/// Fn modifier cannot go through Carbon, so they return an error text.
pub fn to_global_hotkey(hotkey: &Hotkey) -> Result<HotKey, String> {
    let Hotkey::Chord { modifiers, key } = hotkey else {
        return Err("a modifier key on its own works only as push-to-talk".into());
    };
    if modifiers.function {
        return Err("the fn key cannot be part of a chord for this action".into());
    }
    let code = match *key {
        Key::Space => Code::Space,
        Key::Escape => Code::Escape,
        Key::Return => Code::Enter,
        Key::Tab => Code::Tab,
        Key::Backspace => Code::Backspace,
        Key::Letter(c) if c.is_ascii_alphabetic() => GH_LETTERS[(c.to_ascii_lowercase() as u8 - b'a') as usize],
        Key::Digit(d) if d <= 9 => GH_DIGITS[d as usize],
        Key::F(n) if (1..=20).contains(&n) => GH_FKEYS[n as usize - 1],
        other => return Err(format!("unsupported key {other:?}")),
    };
    let mut mods = GhModifiers::empty();
    if modifiers.command {
        mods |= GhModifiers::SUPER;
    }
    if modifiers.option {
        mods |= GhModifiers::ALT;
    }
    if modifiers.control {
        mods |= GhModifiers::CONTROL;
    }
    if modifiers.shift {
        mods |= GhModifiers::SHIFT;
    }
    Ok(HotKey::new(if mods.is_empty() { None } else { Some(mods) }, code))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(s: &str) -> Hotkey {
        s.parse().unwrap()
    }

    #[test]
    fn keycodes_match_the_ansi_layout() {
        assert_eq!(keycode(Key::Space), Some(49));
        assert_eq!(keycode(Key::Escape), Some(53));
        assert_eq!(keycode(Key::Letter('v')), Some(9));
        assert_eq!(keycode(Key::Letter('D')), Some(2));
        assert_eq!(keycode(Key::Letter('z')), Some(6));
        assert_eq!(keycode(Key::Digit(1)), Some(18));
        assert_eq!(keycode(Key::Digit(0)), Some(29));
        assert_eq!(keycode(Key::F(1)), Some(122));
        assert_eq!(keycode(Key::F(5)), Some(96));
        assert_eq!(keycode(Key::F(12)), Some(111));
        assert_eq!(keycode(Key::F(21)), None);
        assert_eq!(keycode(Key::Letter('é')), None);
    }

    #[test]
    fn keycode_round_trips_for_every_supported_key() {
        for c in b'a'..=b'z' {
            let k = Key::Letter(c as char);
            assert_eq!(key_from_keycode(keycode(k).unwrap()), Some(k));
        }
        for d in 0..=9 {
            assert_eq!(key_from_keycode(keycode(Key::Digit(d)).unwrap()), Some(Key::Digit(d)));
        }
        for n in 1..=20 {
            assert_eq!(key_from_keycode(keycode(Key::F(n)).unwrap()), Some(Key::F(n)));
        }
        assert_eq!(key_from_keycode(53), Some(Key::Escape));
        assert_eq!(key_from_keycode(200), None);
    }

    #[test]
    fn letter_and_digit_tables_have_unique_codes() {
        let mut all: Vec<u16> = LETTER_CODES.iter().chain(&DIGIT_CODES).chain(&F_CODES).copied().collect();
        all.extend([49, 53, 36, 48, 51]);
        let n = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), n);
    }

    #[test]
    fn flags_and_carbon_masks() {
        let Hotkey::Chord { modifiers, .. } = chord("ctrl+cmd+v") else { unreachable!() };
        assert_eq!(cg_flags(modifiers), FLAG_CONTROL | FLAG_COMMAND);
        assert_eq!(carbon_mask(modifiers), CARBON_CONTROL | CARBON_CMD);
        let Hotkey::Chord { modifiers, .. } = chord("fn+shift+f5") else { unreachable!() };
        assert_eq!(cg_flags(modifiers), FLAG_SHIFT | FLAG_SECONDARY_FN);
        assert_eq!(carbon_mask(modifiers), CARBON_SHIFT);
        assert_eq!(modifiers_from_cg_flags(FLAG_ALT | FLAG_SHIFT | 0x20), Modifiers { option: true, shift: true, ..Default::default() });
    }

    #[test]
    fn solo_modifiers_use_device_bits() {
        let right_opt = solo_spec(SoloModifier::RightOption);
        assert_eq!(right_opt.keycode, 61);
        assert!(solo_is_down(SoloModifier::RightOption, FLAG_ALT | 0x40));
        // Left Option held: generic Alt flag set, right-side bit clear.
        assert!(!solo_is_down(SoloModifier::RightOption, FLAG_ALT | 0x20));
        assert!(solo_is_down(SoloModifier::LeftOption, FLAG_ALT | 0x20));
        assert!(solo_is_down(SoloModifier::Fn, FLAG_SECONDARY_FN));
        assert!(!solo_is_down(SoloModifier::Fn, 0));
        assert_eq!(solo_from_keycode(63), Some(SoloModifier::Fn));
        assert_eq!(solo_from_keycode(55), None);
        let mut codes: Vec<u16> = ALL_SOLO.iter().map(|m| solo_spec(*m).keycode).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), ALL_SOLO.len());
    }

    #[test]
    fn maps_chords_to_global_hotkey() {
        let hk = to_global_hotkey(&Hotkey::toggle_default()).unwrap();
        assert_eq!(hk, HotKey::new(Some(GhModifiers::ALT), Code::Space));
        let hk = to_global_hotkey(&Hotkey::paste_last_default()).unwrap();
        assert_eq!(hk, HotKey::new(Some(GhModifiers::CONTROL | GhModifiers::SUPER), Code::KeyV));
        let hk = to_global_hotkey(&chord("shift+f5")).unwrap();
        assert_eq!(hk, HotKey::new(Some(GhModifiers::SHIFT), Code::F5));
        let hk = to_global_hotkey(&chord("cmd+7")).unwrap();
        assert_eq!(hk, HotKey::new(Some(GhModifiers::SUPER), Code::Digit7));
    }

    #[test]
    fn rejects_what_carbon_cannot_register() {
        assert!(to_global_hotkey(&chord("right_option")).is_err());
        assert!(to_global_hotkey(&chord("fn+space")).is_err());
        assert!(to_global_hotkey(&Hotkey::Chord { modifiers: Modifiers::default(), key: Key::F(40) }).is_err());
    }
}
