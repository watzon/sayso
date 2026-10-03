//! Pure mapping between [`sayso_core::hotkey`] types and Windows virtual-key
//! codes, `RegisterHotKey` modifier flags, and `global-hotkey` types.
//!
//! On Windows, `command` is the Windows key, `option` is Alt, and `control`
//! is Ctrl. The Fn key never reaches Windows (the keyboard handles it), so a
//! hotkey with Fn cannot work here.
//!
//! No FFI here, so everything is unit-tested.

use global_hotkey::hotkey::{Code, HotKey, Modifiers as GhModifiers};
use sayso_core::hotkey::{Hotkey, Key, Modifiers, SoloModifier};

pub const VK_BACK: u16 = 0x08;
pub const VK_TAB: u16 = 0x09;
pub const VK_RETURN: u16 = 0x0D;
pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12;
pub const VK_ESCAPE: u16 = 0x1B;
pub const VK_SPACE: u16 = 0x20;
pub const VK_LWIN: u16 = 0x5B;
pub const VK_RWIN: u16 = 0x5C;
pub const VK_F1: u16 = 0x70;
pub const VK_LSHIFT: u16 = 0xA0;
pub const VK_RSHIFT: u16 = 0xA1;
pub const VK_LCONTROL: u16 = 0xA2;
pub const VK_RCONTROL: u16 = 0xA3;
pub const VK_LMENU: u16 = 0xA4;
pub const VK_RMENU: u16 = 0xA5;
pub const VK_V: u16 = 0x56;
/// An unassigned key code. Sending it while Alt or Win is held stops the
/// release from opening the menu bar or the Start menu.
pub const VK_MASK: u16 = 0xE8;

/// `RegisterHotKey` modifier flags.
pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;
pub const MOD_NOREPEAT: u32 = 0x4000;

/// The error text for anything with the Fn key.
pub const FN_UNSUPPORTED: &str = "Windows cannot see the Fn key";

/// Virtual-key code for a key, or `None` when the key is out of range
/// (for example `Letter('é')` or `F(30)`). Windows has F1 to F24.
pub fn vk(key: Key) -> Option<u16> {
    match key {
        Key::Space => Some(VK_SPACE),
        Key::Escape => Some(VK_ESCAPE),
        Key::Return => Some(VK_RETURN),
        Key::Tab => Some(VK_TAB),
        Key::Backspace => Some(VK_BACK),
        // The virtual-key codes of letters and digits are their uppercase ASCII codes.
        Key::Letter(c) if c.is_ascii_alphabetic() => Some(u16::from(c.to_ascii_uppercase() as u8)),
        Key::Digit(d) if d <= 9 => Some(u16::from(b'0' + d)),
        Key::F(n) if (1..=20).contains(&n) => Some(VK_F1 + u16::from(n) - 1),
        _ => None,
    }
}

/// Reverse of [`vk`]. Returns `None` for keys Sayso does not support.
pub fn key_from_vk(code: u16) -> Option<Key> {
    Some(match code {
        VK_SPACE => Key::Space,
        VK_ESCAPE => Key::Escape,
        VK_RETURN => Key::Return,
        VK_TAB => Key::Tab,
        VK_BACK => Key::Backspace,
        0x41..=0x5A => Key::Letter((code as u8).to_ascii_lowercase() as char),
        0x30..=0x39 => Key::Digit(code as u8 - b'0'),
        _ if (VK_F1..VK_F1 + 20).contains(&code) => Key::F((code - VK_F1 + 1) as u8),
        _ => return None,
    })
}

/// `RegisterHotKey` flags for a modifier set. Windows has no Fn flag.
pub fn hotkey_mods(m: Modifiers) -> u32 {
    let mut f = 0;
    if m.command {
        f |= MOD_WIN;
    }
    if m.option {
        f |= MOD_ALT;
    }
    if m.control {
        f |= MOD_CONTROL;
    }
    if m.shift {
        f |= MOD_SHIFT;
    }
    f
}

// ---------------------------------------------------------------------------
// Modifier keys
// ---------------------------------------------------------------------------

/// The eight modifier keys, one bit each, left and right apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModKeys(u8);

/// The modifier keys in bit order.
pub const MODIFIER_VKS: [u16; 8] =
    [VK_LSHIFT, VK_RSHIFT, VK_LCONTROL, VK_RCONTROL, VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN];

impl ModKeys {
    fn bit(vk: u16) -> Option<u8> {
        MODIFIER_VKS.iter().position(|m| *m == vk).map(|i| 1 << i)
    }

    /// The set with these keys down.
    pub fn of(vks: &[u16]) -> Self {
        vks.iter().fold(Self::default(), |set, vk| set.with(*vk, true))
    }

    /// The set with `vk` down or up. Other keys leave it unchanged.
    pub fn with(self, vk: u16, down: bool) -> Self {
        match Self::bit(vk) {
            Some(bit) if down => Self(self.0 | bit),
            Some(bit) => Self(self.0 & !bit),
            None => self,
        }
    }

    pub fn contains(self, vk: u16) -> bool {
        Self::bit(vk).is_some_and(|bit| self.0 & bit != 0)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// How many modifier keys are down.
    pub fn count(self) -> u32 {
        self.0.count_ones()
    }

    /// The platform-neutral modifiers. Either side counts.
    pub fn modifiers(self) -> Modifiers {
        let any = |a, b| self.contains(a) || self.contains(b);
        Modifiers {
            command: any(VK_LWIN, VK_RWIN),
            option: any(VK_LMENU, VK_RMENU),
            control: any(VK_LCONTROL, VK_RCONTROL),
            shift: any(VK_LSHIFT, VK_RSHIFT),
            function: false,
        }
    }
}

pub fn is_modifier(vk: u16) -> bool {
    MODIFIER_VKS.contains(&vk)
}

/// The left or right key code for a generic Shift, Ctrl, or Alt code. The
/// keyboard hook normally reports the sided codes already. `extended` is the
/// extended-key flag, and the scan code of Right Shift is `0x36`.
pub fn sided_vk(vk: u16, scan: u32, extended: bool) -> u16 {
    match vk {
        VK_SHIFT if scan & 0xFF == 0x36 => VK_RSHIFT,
        VK_SHIFT => VK_LSHIFT,
        VK_CONTROL if extended => VK_RCONTROL,
        VK_CONTROL => VK_LCONTROL,
        VK_MENU if extended => VK_RMENU,
        VK_MENU => VK_LMENU,
        other => other,
    }
}

/// Keys that `SendInput` must mark as extended, so Windows sends the right
/// side and not the left one.
pub fn is_extended(vk: u16) -> bool {
    matches!(vk, VK_RCONTROL | VK_RMENU | VK_LWIN | VK_RWIN)
}

// ---------------------------------------------------------------------------
// Solo modifiers
// ---------------------------------------------------------------------------

/// The key code of a solo modifier. Fn has none on Windows.
pub fn solo_vk(m: SoloModifier) -> Option<u16> {
    match m {
        SoloModifier::RightOption => Some(VK_RMENU),
        SoloModifier::LeftOption => Some(VK_LMENU),
        SoloModifier::RightCommand => Some(VK_RWIN),
        SoloModifier::RightControl => Some(VK_RCONTROL),
        SoloModifier::RightShift => Some(VK_RSHIFT),
        SoloModifier::Fn => None,
    }
}

/// The solo modifiers Windows can detect, for the key recorder.
pub const ALL_SOLO: [SoloModifier; 5] = [
    SoloModifier::RightOption,
    SoloModifier::LeftOption,
    SoloModifier::RightCommand,
    SoloModifier::RightControl,
    SoloModifier::RightShift,
];

pub fn solo_from_vk(vk: u16) -> Option<SoloModifier> {
    ALL_SOLO.into_iter().find(|m| solo_vk(*m) == Some(vk))
}

/// Whether releasing this modifier on its own makes Windows do something:
/// Alt opens the menu bar of the focused app, and Win opens the Start menu.
pub fn release_has_meaning(vk: u16) -> bool {
    matches!(vk, VK_LMENU | VK_RMENU | VK_LWIN | VK_RWIN)
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

/// Map a chord to a `global-hotkey` hotkey (it calls `RegisterHotKey`). Solo
/// modifiers and chords with Fn cannot be registered, so they return an error text.
pub fn to_global_hotkey(hotkey: &Hotkey) -> Result<HotKey, String> {
    let Hotkey::Chord { modifiers, key } = hotkey else {
        return Err("a modifier key on its own works only as push-to-talk".into());
    };
    if modifiers.function {
        return Err(FN_UNSUPPORTED.into());
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
    fn key_codes_follow_the_windows_table() {
        assert_eq!(vk(Key::Space), Some(0x20));
        assert_eq!(vk(Key::Escape), Some(0x1B));
        assert_eq!(vk(Key::Return), Some(0x0D));
        assert_eq!(vk(Key::Letter('v')), Some(0x56));
        assert_eq!(vk(Key::Letter('A')), Some(0x41));
        assert_eq!(vk(Key::Digit(0)), Some(0x30));
        assert_eq!(vk(Key::Digit(9)), Some(0x39));
        assert_eq!(vk(Key::F(1)), Some(0x70));
        assert_eq!(vk(Key::F(12)), Some(0x7B));
        assert_eq!(vk(Key::F(20)), Some(0x83));
        assert_eq!(vk(Key::F(21)), None);
        assert_eq!(vk(Key::Letter('é')), None);
        assert_eq!(vk(Key::Digit(10)), None);
    }

    #[test]
    fn key_codes_round_trip_for_every_supported_key() {
        let fixed = [Key::Space, Key::Escape, Key::Return, Key::Tab, Key::Backspace];
        let all = fixed
            .into_iter()
            .chain((b'a'..=b'z').map(|c| Key::Letter(c as char)))
            .chain((0..=9).map(Key::Digit))
            .chain((1..=20).map(Key::F));
        for key in all {
            assert_eq!(key_from_vk(vk(key).unwrap()), Some(key), "{key:?}");
        }
        assert_eq!(key_from_vk(VK_LSHIFT), None);
        assert_eq!(key_from_vk(0x84), None, "F21 is not a Sayso key");
        assert_eq!(key_from_vk(VK_MASK), None);
    }

    #[test]
    fn modifier_flags_for_register_hotkey() {
        let Hotkey::Chord { modifiers, .. } = chord("ctrl+cmd+v") else { unreachable!() };
        assert_eq!(hotkey_mods(modifiers), MOD_CONTROL | MOD_WIN);
        let Hotkey::Chord { modifiers, .. } = Hotkey::toggle_default() else { unreachable!() };
        assert_eq!(hotkey_mods(modifiers), MOD_ALT);
        let Hotkey::Chord { modifiers, .. } = chord("fn+shift+f5") else { unreachable!() };
        assert_eq!(hotkey_mods(modifiers), MOD_SHIFT, "Windows has no Fn flag");
    }

    #[test]
    fn mod_keys_keep_left_and_right_apart() {
        let set = ModKeys::of(&[VK_RMENU]);
        assert!(set.contains(VK_RMENU));
        assert!(!set.contains(VK_LMENU));
        assert_eq!(set.modifiers(), Modifiers { option: true, ..Default::default() });
        let set = set.with(VK_LCONTROL, true).with(VK_RMENU, false);
        assert_eq!(set.modifiers(), Modifiers { control: true, ..Default::default() });
        assert_eq!(set.count(), 1);
        assert!(set.with(VK_LCONTROL, false).is_empty());
        // A key that is not a modifier changes nothing.
        assert_eq!(set.with(0x41, true), set);
        let all = ModKeys::of(&MODIFIER_VKS);
        assert_eq!(all.count(), 8);
        assert_eq!(
            all.modifiers(),
            Modifiers { command: true, option: true, control: true, shift: true, function: false }
        );
    }

    #[test]
    fn generic_codes_become_sided() {
        assert_eq!(sided_vk(VK_SHIFT, 0x2A, false), VK_LSHIFT);
        assert_eq!(sided_vk(VK_SHIFT, 0x36, false), VK_RSHIFT);
        assert_eq!(sided_vk(VK_CONTROL, 0x1D, false), VK_LCONTROL);
        assert_eq!(sided_vk(VK_CONTROL, 0x1D, true), VK_RCONTROL);
        assert_eq!(sided_vk(VK_MENU, 0x38, true), VK_RMENU);
        assert_eq!(sided_vk(VK_MENU, 0x38, false), VK_LMENU);
        assert_eq!(sided_vk(VK_RMENU, 0x38, true), VK_RMENU);
        assert_eq!(sided_vk(0x41, 0x1E, false), 0x41);
    }

    #[test]
    fn solo_modifiers_map_to_the_right_keys() {
        assert_eq!(solo_vk(SoloModifier::RightOption), Some(VK_RMENU));
        assert_eq!(solo_vk(SoloModifier::LeftOption), Some(VK_LMENU));
        assert_eq!(solo_vk(SoloModifier::RightCommand), Some(VK_RWIN));
        assert_eq!(solo_vk(SoloModifier::RightControl), Some(VK_RCONTROL));
        assert_eq!(solo_vk(SoloModifier::RightShift), Some(VK_RSHIFT));
        assert_eq!(solo_vk(SoloModifier::Fn), None);
        for m in ALL_SOLO {
            assert_eq!(solo_from_vk(solo_vk(m).unwrap()), Some(m));
        }
        assert_eq!(solo_from_vk(VK_LSHIFT), None);
        assert_eq!(solo_from_vk(VK_LWIN), None);
        assert_eq!(solo_from_vk(VK_LCONTROL), None);
    }

    #[test]
    fn only_alt_and_win_releases_have_a_meaning() {
        assert!(release_has_meaning(VK_RMENU));
        assert!(release_has_meaning(VK_LMENU));
        assert!(release_has_meaning(VK_RWIN));
        assert!(!release_has_meaning(VK_RCONTROL));
        assert!(!release_has_meaning(VK_RSHIFT));
    }

    #[test]
    fn maps_chords_to_global_hotkey() {
        let hk = to_global_hotkey(&Hotkey::toggle_default()).unwrap();
        assert_eq!(hk, HotKey::new(Some(GhModifiers::ALT), Code::Space));
        let hk = to_global_hotkey(&Hotkey::paste_last_default()).unwrap();
        assert_eq!(hk, HotKey::new(Some(GhModifiers::ALT | GhModifiers::SHIFT), Code::KeyV));
        let hk = to_global_hotkey(&chord("ctrl+cmd+v")).unwrap();
        assert_eq!(hk, HotKey::new(Some(GhModifiers::CONTROL | GhModifiers::SUPER), Code::KeyV));
        let hk = to_global_hotkey(&chord("shift+f5")).unwrap();
        assert_eq!(hk, HotKey::new(Some(GhModifiers::SHIFT), Code::F5));
        let hk = to_global_hotkey(&chord("cmd+7")).unwrap();
        assert_eq!(hk, HotKey::new(Some(GhModifiers::SUPER), Code::Digit7));
    }

    #[test]
    fn rejects_what_register_hotkey_cannot_take() {
        assert!(to_global_hotkey(&chord("right_option")).is_err());
        assert_eq!(to_global_hotkey(&chord("fn+space")), Err(FN_UNSUPPORTED.to_string()));
        assert!(to_global_hotkey(&Hotkey::Chord { modifiers: Modifiers::default(), key: Key::F(40) }).is_err());
    }
}
