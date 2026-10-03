//! Pure mapping between [`sayso_core::hotkey`] types and Linux key codes.
//!
//! Keys map to evdev key codes by physical position, like the ANSI key codes
//! on macOS. An X11 key code is the evdev code plus 8. The modifiers map as
//! follows: control is Ctrl, option is Alt, shift is Shift, and command is
//! Super. Linux has no Fn modifier that software can see.

use sayso_core::hotkey::{Hotkey, Key, Modifiers, SoloModifier};

pub const KEY_ESC: u16 = 1;
pub const KEY_BACKSPACE: u16 = 14;
pub const KEY_TAB: u16 = 15;
pub const KEY_ENTER: u16 = 28;
pub const KEY_LEFTCTRL: u16 = 29;
pub const KEY_LEFTSHIFT: u16 = 42;
pub const KEY_RIGHTSHIFT: u16 = 54;
pub const KEY_LEFTALT: u16 = 56;
pub const KEY_SPACE: u16 = 57;
pub const KEY_RIGHTCTRL: u16 = 97;
pub const KEY_RIGHTALT: u16 = 100;
pub const KEY_LEFTMETA: u16 = 125;
pub const KEY_RIGHTMETA: u16 = 126;

/// X11 key codes are evdev codes plus this offset.
pub const X11_OFFSET: u16 = 8;

/// evdev codes for `a` to `z` (QWERTY positions).
const LETTER_CODES: [u16; 26] = [
    30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44,
];
/// evdev codes for `0` to `9`.
const DIGIT_CODES: [u16; 10] = [11, 2, 3, 4, 5, 6, 7, 8, 9, 10];
/// evdev codes for `F1` to `F20`.
const F_CODES: [u16; 20] = [59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88, 183, 184, 185, 186, 187, 188, 189, 190];

/// The evdev code of a key, or `None` when the key is out of range.
pub fn code(key: Key) -> Option<u16> {
    match key {
        Key::Space => Some(KEY_SPACE),
        Key::Escape => Some(KEY_ESC),
        Key::Return => Some(KEY_ENTER),
        Key::Tab => Some(KEY_TAB),
        Key::Backspace => Some(KEY_BACKSPACE),
        Key::Letter(c) if c.is_ascii_alphabetic() => Some(LETTER_CODES[(c.to_ascii_lowercase() as u8 - b'a') as usize]),
        Key::Digit(d) if d <= 9 => Some(DIGIT_CODES[d as usize]),
        Key::F(n) if (1..=20).contains(&n) => Some(F_CODES[n as usize - 1]),
        _ => None,
    }
}

/// Reverse of [`code`]. Returns `None` for keys that Sayso does not support.
pub fn key_from_code(code_: u16) -> Option<Key> {
    let fixed = [Key::Space, Key::Escape, Key::Return, Key::Tab, Key::Backspace];
    fixed
        .into_iter()
        .chain((b'a'..=b'z').map(|c| Key::Letter(c as char)))
        .chain((0..=9).map(Key::Digit))
        .chain((1..=20).map(Key::F))
        .find(|k| code(*k) == Some(code_))
}

/// The evdev code of a solo modifier. Fn has none.
pub fn solo_code(m: SoloModifier) -> Option<u16> {
    match m {
        SoloModifier::RightOption => Some(KEY_RIGHTALT),
        SoloModifier::LeftOption => Some(KEY_LEFTALT),
        SoloModifier::RightCommand => Some(KEY_RIGHTMETA),
        SoloModifier::RightControl => Some(KEY_RIGHTCTRL),
        SoloModifier::RightShift => Some(KEY_RIGHTSHIFT),
        SoloModifier::Fn => None,
    }
}

pub fn solo_from_code(code_: u16) -> Option<SoloModifier> {
    [
        SoloModifier::RightOption,
        SoloModifier::LeftOption,
        SoloModifier::RightCommand,
        SoloModifier::RightControl,
        SoloModifier::RightShift,
    ]
    .into_iter()
    .find(|m| solo_code(*m) == Some(code_))
}

/// True for the eight modifier keys (left and right Ctrl, Shift, Alt, Super).
pub fn is_modifier(code_: u16) -> bool {
    matches!(
        code_,
        KEY_LEFTCTRL
            | KEY_RIGHTCTRL
            | KEY_LEFTSHIFT
            | KEY_RIGHTSHIFT
            | KEY_LEFTALT
            | KEY_RIGHTALT
            | KEY_LEFTMETA
            | KEY_RIGHTMETA
    )
}

/// The modifiers that a set of pressed keys holds down.
pub fn modifiers_of(pressed: impl IntoIterator<Item = u16>) -> Modifiers {
    let mut m = Modifiers::default();
    for code_ in pressed {
        match code_ {
            KEY_LEFTCTRL | KEY_RIGHTCTRL => m.control = true,
            KEY_LEFTSHIFT | KEY_RIGHTSHIFT => m.shift = true,
            KEY_LEFTALT | KEY_RIGHTALT => m.option = true,
            KEY_LEFTMETA | KEY_RIGHTMETA => m.command = true,
            _ => {}
        }
    }
    m
}

/// A chord as the key logic and the backends see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub code: u16,
    pub modifiers: Modifiers,
}

/// Check that a hotkey is a chord that Linux can use, and map it.
pub fn chord(hotkey: &Hotkey) -> Result<Chord, String> {
    let Hotkey::Chord { modifiers, key } = *hotkey else {
        return Err("a modifier key on its own works only as push-to-talk".into());
    };
    if modifiers.function {
        return Err("Linux cannot see the fn key, so it cannot be part of a shortcut".into());
    }
    let code_ = code(key).ok_or_else(|| format!("unsupported key {key:?}"))?;
    Ok(Chord { code: code_, modifiers })
}

/// Check that a hotkey can be a push-to-talk key: a chord, or a solo
/// modifier other than Fn.
pub fn check_push_to_talk(hotkey: &Hotkey) -> Result<(), String> {
    match hotkey {
        Hotkey::Solo(m) => solo_code(*m)
            .map(|_| ())
            .ok_or_else(|| "Linux cannot see the fn key. Choose another key".to_string()),
        Hotkey::Chord { .. } => chord(hotkey).map(|_| ()),
    }
}

/// The trigger text for the XDG GlobalShortcuts portal, in the format of the
/// freedesktop shortcuts specification, for example `CTRL+ALT+space`.
pub fn portal_trigger(chord_: &Chord) -> Option<String> {
    let key = key_from_code(chord_.code)?;
    let mut parts: Vec<String> = Vec::new();
    let m = chord_.modifiers;
    if m.control {
        parts.push("CTRL".into());
    }
    if m.option {
        parts.push("ALT".into());
    }
    if m.shift {
        parts.push("SHIFT".into());
    }
    if m.command {
        parts.push("LOGO".into());
    }
    parts.push(keysym_name(key));
    Some(parts.join("+"))
}

/// The XKB keysym name of a key, as the shortcuts specification uses it.
fn keysym_name(key: Key) -> String {
    match key {
        Key::Space => "space".into(),
        Key::Escape => "Escape".into(),
        Key::Return => "Return".into(),
        Key::Tab => "Tab".into(),
        Key::Backspace => "BackSpace".into(),
        Key::Letter(c) => c.to_ascii_lowercase().to_string(),
        Key::Digit(d) => d.to_string(),
        Key::F(n) => format!("F{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hk(s: &str) -> Hotkey {
        s.parse().unwrap()
    }

    #[test]
    fn codes_follow_the_evdev_layout() {
        assert_eq!(code(Key::Space), Some(57));
        assert_eq!(code(Key::Escape), Some(1));
        assert_eq!(code(Key::Letter('a')), Some(30));
        assert_eq!(code(Key::Letter('Q')), Some(16));
        assert_eq!(code(Key::Letter('v')), Some(47));
        assert_eq!(code(Key::Letter('z')), Some(44));
        assert_eq!(code(Key::Digit(1)), Some(2));
        assert_eq!(code(Key::Digit(0)), Some(11));
        assert_eq!(code(Key::F(1)), Some(59));
        assert_eq!(code(Key::F(11)), Some(87));
        assert_eq!(code(Key::F(13)), Some(183));
        assert_eq!(code(Key::F(21)), None);
        assert_eq!(code(Key::Letter('é')), None);
    }

    #[test]
    fn codes_round_trip_and_are_unique() {
        let mut all = Vec::new();
        let keys = [Key::Space, Key::Escape, Key::Return, Key::Tab, Key::Backspace]
            .into_iter()
            .chain((b'a'..=b'z').map(|c| Key::Letter(c as char)))
            .chain((0..=9).map(Key::Digit))
            .chain((1..=20).map(Key::F));
        for k in keys {
            let c = code(k).unwrap();
            assert_eq!(key_from_code(c), Some(k));
            assert!(!is_modifier(c));
            all.push(c);
        }
        let n = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), n);
        assert_eq!(key_from_code(KEY_LEFTCTRL), None);
    }

    #[test]
    fn solo_modifiers_map_to_one_side() {
        assert_eq!(solo_code(SoloModifier::RightOption), Some(KEY_RIGHTALT));
        assert_eq!(solo_code(SoloModifier::LeftOption), Some(KEY_LEFTALT));
        assert_eq!(solo_code(SoloModifier::RightCommand), Some(KEY_RIGHTMETA));
        assert_eq!(solo_code(SoloModifier::Fn), None);
        assert_eq!(solo_from_code(KEY_RIGHTCTRL), Some(SoloModifier::RightControl));
        assert_eq!(solo_from_code(KEY_LEFTCTRL), None);
        assert_eq!(solo_from_code(KEY_SPACE), None);
    }

    #[test]
    fn modifiers_come_from_both_sides() {
        let m = modifiers_of([KEY_RIGHTCTRL, KEY_LEFTALT, KEY_SPACE]);
        assert_eq!(m, Modifiers { control: true, option: true, ..Default::default() });
        let m = modifiers_of([KEY_LEFTMETA, KEY_RIGHTSHIFT]);
        assert_eq!(m, Modifiers { command: true, shift: true, ..Default::default() });
        assert!(modifiers_of([]).is_empty());
    }

    #[test]
    fn chords_are_checked() {
        let c = chord(&hk("ctrl+opt+space")).unwrap();
        assert_eq!(c.code, KEY_SPACE);
        assert!(c.modifiers.control && c.modifiers.option);
        assert!(chord(&hk("right_option")).is_err());
        assert!(chord(&hk("fn+space")).is_err());
        assert!(check_push_to_talk(&hk("right_control")).is_ok());
        assert!(check_push_to_talk(&hk("ctrl+shift+d")).is_ok());
        assert!(check_push_to_talk(&hk("fn")).is_err());
    }

    #[test]
    fn portal_triggers_use_the_shortcuts_spec() {
        let t = |s: &str| portal_trigger(&chord(&hk(s)).unwrap()).unwrap();
        assert_eq!(t("opt+space"), "ALT+space");
        assert_eq!(t("ctrl+cmd+v"), "CTRL+LOGO+v");
        assert_eq!(t("cmd+shift+ctrl+opt+k"), "CTRL+ALT+SHIFT+LOGO+k");
        assert_eq!(t("shift+f5"), "SHIFT+F5");
        assert_eq!(t("ctrl+backspace"), "CTRL+BackSpace");
        assert_eq!(t("opt+return"), "ALT+Return");
        assert_eq!(t("ctrl+7"), "CTRL+7");
        assert_eq!(t("f9"), "F9");
    }
}
