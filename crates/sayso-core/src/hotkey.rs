//! Platform-neutral hotkey description.
//!
//! A hotkey is either a chord (modifiers plus one key, for example
//! `opt+space`) or a single modifier held alone (for example `right_option`).
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

/// A modifier key on its own, used for push-to-talk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoloModifier {
    RightOption,
    LeftOption,
    RightCommand,
    RightControl,
    RightShift,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hotkey {
    Chord { modifiers: Modifiers, key: Key },
    Solo(SoloModifier),
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
    pub fn label(&self) -> &'static str {
        #[cfg(target_os = "macos")]
        return match self {
            SoloModifier::RightOption => "Right ⌥",
            SoloModifier::LeftOption => "Left ⌥",
            SoloModifier::RightCommand => "Right ⌘",
            SoloModifier::RightControl => "Right ⌃",
            SoloModifier::RightShift => "Right ⇧",
            SoloModifier::Fn => "fn",
        };
        #[cfg(not(target_os = "macos"))]
        return match self {
            SoloModifier::RightOption => "Right Alt",
            SoloModifier::LeftOption => "Left Alt",
            SoloModifier::RightCommand => if cfg!(target_os = "windows") { "Right Win" } else { "Right Super" },
            SoloModifier::RightControl => "Right Ctrl",
            SoloModifier::RightShift => "Right Shift",
            SoloModifier::Fn => "fn",
        };
    }
    fn token(&self) -> &'static str {
        match self {
            SoloModifier::RightOption => "right_option",
            SoloModifier::LeftOption => "left_option",
            SoloModifier::RightCommand => "right_command",
            SoloModifier::RightControl => "right_control",
            SoloModifier::RightShift => "right_shift",
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
    /// `"right_option"` or `"fn"`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(HotkeyParseError::Empty);
        }
        for solo in [
            SoloModifier::RightOption,
            SoloModifier::LeftOption,
            SoloModifier::RightCommand,
            SoloModifier::RightControl,
            SoloModifier::RightShift,
            SoloModifier::Fn,
        ] {
            if s.eq_ignore_ascii_case(solo.token()) {
                return Ok(Hotkey::Solo(solo));
            }
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
