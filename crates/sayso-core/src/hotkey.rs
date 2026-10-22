//! Platform-neutral hotkey description.
//!
//! A hotkey is either a chord (modifiers plus one key, for example
//! `opt+space`), a single modifier (for example `right_option`), or a combination
//! of side-specific modifiers (for example `left_command+left_option`).
//! The platform crate maps these to key codes.
//!
//! The modifier names are the Mac ones. On Linux and Windows, `option` is
//! Alt and `command` is the Super (Windows) key. The config file and the
//! keycaps use each system's own names; the parser takes both.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    Space,
    Escape,
    Return,
    Tab,
    Backspace,
    Letter(char),
    Digit(u8),
    F(u8),
}

/// A modifier key on its own, used for toggle or push-to-talk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoloModifier {
    RightOption,
    LeftOption,
    RightCommand,
    LeftCommand,
    RightControl,
    LeftControl,
    RightShift,
    LeftShift,
    Fn,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Modifiers {
    pub command: bool,
    pub option: bool,
    pub control: bool,
    pub shift: bool,
    pub function: bool,
}

impl Modifiers {
    pub fn is_empty(&self) -> bool {
        !(self.command || self.option || self.control || self.shift || self.function)
    }
}

/// A side-specific set of modifier keys. Its order is independent of press order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ModifierSet(u16);

impl ModifierSet {
    pub fn insert(&mut self, modifier: SoloModifier) -> bool {
        let bit = 1 << modifier as u16;
        let fresh = self.0 & bit == 0;
        self.0 |= bit;
        fresh
    }

    pub fn contains(self, modifier: SoloModifier) -> bool {
        self.0 & (1 << modifier as u16) != 0
    }

    pub fn len(self) -> usize { self.0.count_ones() as usize }
    pub fn is_empty(self) -> bool { self.0 == 0 }
    pub fn is_subset_of(self, other: Self) -> bool { self.0 & other.0 == self.0 }

    pub fn iter(self) -> impl Iterator<Item = SoloModifier> {
        SoloModifier::ALL.into_iter().filter(move |m| self.contains(*m))
    }
}

impl FromIterator<SoloModifier> for ModifierSet {
    fn from_iter<T: IntoIterator<Item = SoloModifier>>(iter: T) -> Self {
        let mut set = Self::default();
        for modifier in iter { set.insert(modifier); }
        set
    }
}

#[cfg(test)]
mod tap_timing_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn deliberate_taps_have_a_ceiling_without_renewing_on_expansion() {
        let now = Instant::now();
        let one: ModifierSet = [SoloModifier::LeftControl].into_iter().collect();
        let two: ModifierSet = [SoloModifier::LeftControl, SoloModifier::LeftShift].into_iter().collect();
        for (delay, expected) in [(400, true), (401, false)] {
            let mut capture = ModifierCapture::default();
            capture.press_tap(one, now);
            capture.press_tap(two, now + Duration::from_millis(350));
            assert_eq!(capture.release_tap(SoloModifier::LeftShift, now + Duration::from_millis(delay)).is_some(), expected);
            assert!(!capture.is_active());
        }
    }

    #[test]
    fn clearing_a_tap_resets_its_clock() {
        let now = Instant::now();
        let keys = [SoloModifier::LeftControl].into_iter().collect();
        let mut capture = ModifierCapture::default();
        capture.press_tap(keys, now);
        capture.clear();
        capture.press_tap(keys, now + Duration::from_secs(1));
        assert!(capture.release_tap(SoloModifier::LeftControl, now + Duration::from_millis(1100)).is_some());
    }
}

/// The modifier-only part of hotkey recording, shared by the native listeners.
#[derive(Debug, Default)]
pub struct ModifierCapture {
    candidate: Option<ModifierSet>,
    tap_started: Option<std::time::Instant>,
}

impl ModifierCapture {
    pub fn clear(&mut self) {
        self.candidate = None;
        self.tap_started = None;
    }

    /// A deliberate toggle tap ends within 400 ms of its first modifier.
    /// Recorder gestures remain untimed; holding a key to choose it is harmless.
    pub fn press_tap(&mut self, held: ModifierSet, now: std::time::Instant) {
        let started = self.tap_started.unwrap_or(now);
        self.press(held);
        self.tap_started = self.candidate.map(|_| started);
    }

    pub fn release_tap(&mut self, modifier: SoloModifier, now: std::time::Instant) -> Option<Hotkey> {
        if !self.tap_started.is_some_and(|start| now.saturating_duration_since(start) <= std::time::Duration::from_millis(400)) {
            self.clear();
            return None;
        }
        self.release(modifier)
    }
    pub fn is_active(&self) -> bool { self.candidate.is_some() }

    pub fn press(&mut self, held: ModifierSet) {
        self.candidate = if held.len() == 1 || self.candidate.is_some_and(|keys| keys.is_subset_of(held)) {
            Some(held)
        } else {
            None
        };
    }

    pub fn release(&mut self, modifier: SoloModifier) -> Option<Hotkey> {
        let candidate = self.candidate?;
        if !candidate.contains(modifier) { return None; }
        self.clear();
        Hotkey::modifier_only(candidate)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hotkey {
    Chord { modifiers: Modifiers, key: Key },
    Solo(SoloModifier),
    /// Modifier keys held together, used for toggle or push-to-talk.
    ModifierOnly(ModifierSet),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HotkeyParseError {
    #[error("the hotkey is empty")]
    Empty,
    #[error("unknown key name \"{0}\"")]
    UnknownKey(String),
    #[error("a hotkey needs exactly one key after its modifiers")]
    WrongKeyCount,
}

impl Hotkey {
    /// Modifier-only actions cannot share a gesture. Chords can share the
    /// toggle's modifiers because a regular key cancels a modifier-only toggle.
    pub fn conflicts_with_toggle(self, toggle: Self) -> bool {
        if self == toggle { return true; }
        let Some(toggle_keys) = toggle.modifier_keys() else { return false };
        if let Some(keys) = self.modifier_keys() {
            return keys.is_subset_of(toggle_keys) || toggle_keys.is_subset_of(keys);
        }
        false
    }

    pub fn modifier_keys(self) -> Option<ModifierSet> {
        match self {
            Self::Solo(modifier) => Some([modifier].into_iter().collect()),
            Self::ModifierOnly(keys) => Some(keys),
            Self::Chord { .. } => None,
        }
    }

    pub fn modifier_only(modifiers: ModifierSet) -> Option<Self> {
        match modifiers.len() {
            0 => None,
            1 => modifiers.iter().next().map(Self::Solo),
            _ => Some(Self::ModifierOnly(modifiers)),
        }
    }

    /// Option+Space on macOS. Ctrl+Space on Windows, where Alt+Space opens
    /// the window menu and is the default of the common launchers.
    /// Ctrl+Alt+Space on Linux, because Alt+Space opens the window menu there.
    pub fn toggle_default() -> Self {
        let modifiers = if cfg!(target_os = "macos") {
            Modifiers { option: true, ..Default::default() }
        } else if cfg!(windows) {
            Modifiers { control: true, ..Default::default() }
        } else {
            Modifiers { control: true, option: true, ..Default::default() }
        };
        Hotkey::Chord { modifiers, key: Key::Space }
    }
    /// Ctrl+Cmd+V on macOS. Windows uses Ctrl+Win+V for the sound output
    /// panel, so there it is Alt+Shift+V. Ctrl+Alt+V on Linux.
    pub fn paste_last_default() -> Self {
        let modifiers = if cfg!(target_os = "macos") {
            Modifiers { control: true, command: true, ..Default::default() }
        } else if cfg!(windows) {
            Modifiers { option: true, shift: true, ..Default::default() }
        } else {
            Modifiers { control: true, option: true, ..Default::default() }
        };
        Hotkey::Chord { modifiers, key: Key::Letter('v') }
    }

    /// Keycap labels for the UI, for example `["⌥", "Space"]` on macOS and
    /// `["Ctrl", "Alt", "Space"]` elsewhere.
    pub fn keycaps(&self) -> Vec<String> {
        match self {
            Hotkey::Solo(m) => vec![m.label().to_string()],
            Hotkey::ModifierOnly(modifiers) => modifiers.iter().map(|m| m.label().to_string()).collect(),
            Hotkey::Chord { modifiers, key } => {
                let names = ModifierNames::SYSTEM;
                let mut caps = Vec::new();
                if modifiers.function {
                    caps.push("fn".to_string());
                }
                if cfg!(target_os = "macos") {
                    // The Mac order: ⌃ ⌥ ⇧ ⌘.
                    for (on, cap) in [
                        (modifiers.control, names.control),
                        (modifiers.option, names.option),
                        (modifiers.shift, names.shift),
                        (modifiers.command, names.command),
                    ] {
                        if on {
                            caps.push(cap.to_string());
                        }
                    }
                } else {
                    // The PC order: Super, Ctrl, Alt, Shift.
                    for (on, cap) in [
                        (modifiers.command, names.command),
                        (modifiers.control, names.control),
                        (modifiers.option, names.option),
                        (modifiers.shift, names.shift),
                    ] {
                        if on {
                            caps.push(cap.to_string());
                        }
                    }
                }
                caps.push(key.label());
                caps
            }
        }
    }
}

impl Hotkey {
    /// The keycaps as one short label: "⌃⌘V" on macOS, "Alt+Shift+V" elsewhere.
    pub fn compact_label(&self) -> String {
        self.keycaps().join(if cfg!(target_os = "macos") { "" } else { "+" })
    }
}

/// Keycap labels of the modifiers on one system.
struct ModifierNames {
    control: &'static str,
    option: &'static str,
    shift: &'static str,
    command: &'static str,
}

impl ModifierNames {
    #[cfg(target_os = "macos")]
    const SYSTEM: ModifierNames = ModifierNames { control: "⌃", option: "⌥", shift: "⇧", command: "⌘" };
    #[cfg(target_os = "windows")]
    const SYSTEM: ModifierNames = ModifierNames { control: "Ctrl", option: "Alt", shift: "Shift", command: "Win" };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    const SYSTEM: ModifierNames = ModifierNames { control: "Ctrl", option: "Alt", shift: "Shift", command: "Super" };
}

/// The config file names of `option` and `command` on this system.
const OPTION_TOKEN: &str = if cfg!(target_os = "macos") { "opt" } else { "alt" };
const COMMAND_TOKEN: &str = if cfg!(target_os = "macos") {
    "cmd"
} else if cfg!(target_os = "windows") {
    "win"
} else {
    "super"
};

impl SoloModifier {
    pub const ALL: [Self; 9] = [
        Self::Fn,
        Self::LeftCommand,
        Self::LeftControl,
        Self::LeftOption,
        Self::LeftShift,
        Self::RightCommand,
        Self::RightControl,
        Self::RightOption,
        Self::RightShift,
    ];

    fn parse(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| token.trim().eq_ignore_ascii_case(m.token()))
    }

    pub fn label(&self) -> &'static str {
        #[cfg(target_os = "macos")]
        return match self {
            SoloModifier::RightOption => "Right ⌥",
            SoloModifier::LeftOption => "Left ⌥",
            SoloModifier::RightCommand => "Right ⌘",
            SoloModifier::LeftCommand => "Left ⌘",
            SoloModifier::RightControl => "Right ⌃",
            SoloModifier::LeftControl => "Left ⌃",
            SoloModifier::RightShift => "Right ⇧",
            SoloModifier::LeftShift => "Left ⇧",
            SoloModifier::Fn => "fn",
        };
        #[cfg(not(target_os = "macos"))]
        return match self {
            SoloModifier::RightOption => "Right Alt",
            SoloModifier::LeftOption => "Left Alt",
            SoloModifier::RightCommand => if cfg!(target_os = "windows") { "Right Win" } else { "Right Super" },
            SoloModifier::LeftCommand => if cfg!(target_os = "windows") { "Left Win" } else { "Left Super" },
            SoloModifier::RightControl => "Right Ctrl",
            SoloModifier::LeftControl => "Left Ctrl",
            SoloModifier::RightShift => "Right Shift",
            SoloModifier::LeftShift => "Left Shift",
            SoloModifier::Fn => "fn",
        };
    }
    fn token(&self) -> &'static str {
        match self {
            SoloModifier::RightOption => "right_option",
            SoloModifier::LeftOption => "left_option",
            SoloModifier::RightCommand => "right_command",
            SoloModifier::LeftCommand => "left_command",
            SoloModifier::RightControl => "right_control",
            SoloModifier::LeftControl => "left_control",
            SoloModifier::RightShift => "right_shift",
            SoloModifier::LeftShift => "left_shift",
            SoloModifier::Fn => "fn",
        }
    }
}

impl Key {
    pub fn label(&self) -> String {
        match self {
            Key::Space => "Space".into(),
            Key::Escape => "Esc".into(),
            Key::Return => "↵".into(),
            Key::Tab => "⇥".into(),
            Key::Backspace => "⌫".into(),
            Key::Letter(c) => c.to_ascii_uppercase().to_string(),
            Key::Digit(d) => d.to_string(),
            Key::F(n) => format!("F{n}"),
        }
    }
    fn token(&self) -> String {
        match self {
            Key::Space => "space".into(),
            Key::Escape => "esc".into(),
            Key::Return => "return".into(),
            Key::Tab => "tab".into(),
            Key::Backspace => "backspace".into(),
            Key::Letter(c) => c.to_ascii_lowercase().to_string(),
            Key::Digit(d) => d.to_string(),
            Key::F(n) => format!("f{n}"),
        }
    }
    fn parse(token: &str) -> Option<Key> {
        let t = token.to_ascii_lowercase();
        Some(match t.as_str() {
            "space" => Key::Space,
            "esc" | "escape" => Key::Escape,
            "return" | "enter" => Key::Return,
            "tab" => Key::Tab,
            "backspace" | "delete" => Key::Backspace,
            _ if t.len() == 1 && t.chars().all(|c| c.is_ascii_lowercase()) => {
                Key::Letter(t.chars().next()?)
            }
            _ if t.len() == 1 && t.chars().all(|c| c.is_ascii_digit()) => {
                Key::Digit(t.parse().ok()?)
            }
            _ if t.starts_with('f') && t[1..].parse::<u8>().is_ok_and(|n| (1..=20).contains(&n)) => {
                Key::F(t[1..].parse().ok()?)
            }
            _ => return None,
        })
    }
}

impl FromStr for Hotkey {
    type Err = HotkeyParseError;

    /// Parse `"opt+space"`, `"ctrl+cmd+v"`, or a solo modifier such as
    /// `"right_option"` or `"fn"`, or a combination such as `"left_command+left_option"`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(HotkeyParseError::Empty);
        }
        if let Some(solo) = SoloModifier::parse(s) {
            return Ok(Hotkey::Solo(solo));
        }
        let mut modifier_keys = ModifierSet::default();
        let mut modifier_only = true;
        for part in s.split('+') {
            if let Some(modifier) = SoloModifier::parse(part) {
                if !modifier_keys.insert(modifier) {
                    return Err(HotkeyParseError::WrongKeyCount);
                }
            } else {
                modifier_only = false;
                break;
            }
        }
        if modifier_only {
            return Self::modifier_only(modifier_keys).ok_or(HotkeyParseError::WrongKeyCount);
        }
        let mut modifiers = Modifiers::default();
        let mut key = None;
        for part in s.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                // The Windows and Super keys sit where Command does on a Mac keyboard.
                "cmd" | "command" | "super" | "win" | "meta" | "logo" => modifiers.command = true,
                "opt" | "option" | "alt" => modifiers.option = true,
                "ctrl" | "control" => modifiers.control = true,
                "shift" => modifiers.shift = true,
                "fn" => modifiers.function = true,
                other => {
                    if key.is_some() {
                        return Err(HotkeyParseError::WrongKeyCount);
                    }
                    key = Some(Key::parse(other).ok_or_else(|| HotkeyParseError::UnknownKey(other.to_string()))?);
                }
            }
        }
        let key = key.ok_or(HotkeyParseError::WrongKeyCount)?;
        Ok(Hotkey::Chord { modifiers, key })
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Hotkey::Solo(m) => f.write_str(m.token()),
            Hotkey::ModifierOnly(modifiers) => {
                for (i, modifier) in modifiers.iter().enumerate() {
                    if i > 0 { f.write_str("+")?; }
                    f.write_str(modifier.token())?;
                }
                Ok(())
            }
            Hotkey::Chord { modifiers, key } => {
                let mut parts = Vec::new();
                if modifiers.function {
                    parts.push("fn".to_string());
                }
                if modifiers.control {
                    parts.push("ctrl".to_string());
                }
                if modifiers.option {
                    parts.push(OPTION_TOKEN.to_string());
                }
                if modifiers.shift {
                    parts.push("shift".to_string());
                }
                if modifiers.command {
                    parts.push(COMMAND_TOKEN.to_string());
                }
                parts.push(key.token());
                f.write_str(&parts.join("+"))
            }
        }
    }
}

impl Serialize for Hotkey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Hotkey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_round_trips() {
        let list: &[&str] = if cfg!(target_os = "macos") {
            &["opt+space", "ctrl+cmd+v", "right_option", "fn", "ctrl+shift+d", "cmd+f5"]
        } else if cfg!(target_os = "windows") {
            &["alt+space", "ctrl+win+v", "right_option", "ctrl+shift+d", "win+f5"]
        } else {
            &["ctrl+alt+space", "ctrl+super+v", "right_option", "ctrl+shift+d", "super+f5"]
        };
        for s in list {
            let hk: Hotkey = s.parse().unwrap();
            assert_eq!(hk.to_string(), *s);
        }
    }

    #[test]
    fn all_modifier_combinations_round_trip_in_any_order() {
        for mask in 0usize..(1 << SoloModifier::ALL.len()) {
            let selected: Vec<_> = SoloModifier::ALL.into_iter().enumerate().filter_map(|(i, m)| (mask & (1 << i) != 0).then_some(m)).collect();
            if selected.len() < 2 { continue; }
            let text = selected.iter().rev().map(|m| Hotkey::Solo(*m).to_string()).collect::<Vec<_>>().join("+");
            let hotkey: Hotkey = text.parse().unwrap();
            assert_eq!(hotkey, Hotkey::modifier_only(selected.into_iter().collect()).unwrap());
            assert_eq!(hotkey.to_string().parse::<Hotkey>().unwrap(), hotkey);
        }
    }

    #[test]
    fn modifier_pairs_round_trip_independent_of_press_order() {
        let pair: Hotkey = "left_command+left_option".parse().unwrap();
        assert_eq!(" LEFT_OPTION + LEFT_COMMAND ".parse::<Hotkey>().unwrap(), pair);
        assert_eq!(pair.to_string(), "left_command+left_option");
        assert_eq!(serde_json::from_str::<Hotkey>(&serde_json::to_string(&pair).unwrap()).unwrap(), pair);
        assert_eq!(pair.keycaps().len(), 2);
        assert_eq!("left_command".parse::<Hotkey>().unwrap(), Hotkey::Solo(SoloModifier::LeftCommand));
        for invalid in ["left_command+left_command", "left_command+space"] {
            assert!(invalid.parse::<Hotkey>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn aliases_normalize() {
        let opt_space = Hotkey::Chord { modifiers: Modifiers { option: true, ..Default::default() }, key: Key::Space };
        assert_eq!("Option + Space".parse::<Hotkey>().unwrap(), opt_space);
        assert_eq!("alt+space".parse::<Hotkey>().unwrap(), opt_space);
        let cmd_v = Hotkey::Chord { modifiers: Modifiers { command: true, ..Default::default() }, key: Key::Letter('v') };
        for s in ["command+V", "super+v", "win+v", "meta+v"] {
            assert_eq!(s.parse::<Hotkey>().unwrap(), cmd_v, "{s}");
        }
    }

    #[test]
    fn defaults_parse_from_their_own_text() {
        for hk in [Hotkey::toggle_default(), Hotkey::paste_last_default()] {
            assert_eq!(hk.to_string().parse::<Hotkey>().unwrap(), hk);
        }
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!("".parse::<Hotkey>(), Err(HotkeyParseError::Empty));
        assert_eq!("opt+cmd".parse::<Hotkey>(), Err(HotkeyParseError::WrongKeyCount));
        assert_eq!("opt+a+b".parse::<Hotkey>(), Err(HotkeyParseError::WrongKeyCount));
        assert!(matches!("opt+banana".parse::<Hotkey>(), Err(HotkeyParseError::UnknownKey(_))));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn keycaps_follow_mac_order() {
        let hk: Hotkey = "cmd+shift+ctrl+opt+k".parse().unwrap();
        assert_eq!(hk.keycaps(), vec!["⌃", "⌥", "⇧", "⌘", "K"]);
        assert_eq!(Hotkey::toggle_default().keycaps(), vec!["⌥", "Space"]);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn keycaps_follow_pc_order() {
        let hk: Hotkey = "cmd+shift+ctrl+opt+k".parse().unwrap();
        assert_eq!(hk.keycaps(), vec!["Super", "Ctrl", "Alt", "Shift", "K"]);
        assert_eq!(Hotkey::toggle_default().keycaps(), vec!["Ctrl", "Alt", "Space"]);
        assert_eq!(Hotkey::Solo(SoloModifier::RightOption).keycaps(), vec!["Right Alt"]);
    }

    #[test]
    #[cfg(windows)]
    fn keycaps_follow_windows_order() {
        let hk: Hotkey = "cmd+shift+ctrl+opt+k".parse().unwrap();
        assert_eq!(hk.keycaps(), vec!["Win", "Ctrl", "Alt", "Shift", "K"]);
        assert_eq!(Hotkey::toggle_default().keycaps(), vec!["Ctrl", "Space"]);
        assert_eq!(Hotkey::paste_last_default().to_string(), "alt+shift+v");
        assert_eq!(Hotkey::Solo(SoloModifier::RightOption).keycaps(), vec!["Right Alt"]);
    }
}
