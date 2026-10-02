//! Platform-neutral hotkey description.
//!
//! A hotkey is either a chord (modifiers plus one key, for example
//! `opt+space`) or a single modifier held alone (for example `right_option`).
//! The platform crate maps these to key codes.

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
    pub fn toggle_default() -> Self {
        Hotkey::Chord { modifiers: Modifiers { option: true, ..Default::default() }, key: Key::Space }
    }
    pub fn paste_last_default() -> Self {
        Hotkey::Chord {
            modifiers: Modifiers { control: true, command: true, ..Default::default() },
            key: Key::Letter('v'),
        }
    }

    /// Keycap labels for the UI, for example `["⌥", "Space"]`.
    pub fn keycaps(&self) -> Vec<String> {
        match self {
            Hotkey::Solo(m) => vec![m.label().to_string()],
            Hotkey::Chord { modifiers, key } => {
                let mut caps = Vec::new();
                if modifiers.function {
                    caps.push("fn".to_string());
                }
                if modifiers.control {
                    caps.push("⌃".to_string());
                }
                if modifiers.option {
                    caps.push("⌥".to_string());
                }
                if modifiers.shift {
                    caps.push("⇧".to_string());
                }
                if modifiers.command {
                    caps.push("⌘".to_string());
                }
                caps.push(key.label());
                caps
            }
        }
    }
}

impl SoloModifier {
    pub fn label(&self) -> &'static str {
        match self {
            SoloModifier::RightOption => "Right ⌥",
            SoloModifier::LeftOption => "Left ⌥",
            SoloModifier::RightCommand => "Right ⌘",
            SoloModifier::RightControl => "Right ⌃",
            SoloModifier::RightShift => "Right ⇧",
            SoloModifier::Fn => "fn",
        }
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
                "cmd" | "command" => modifiers.command = true,
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
                    parts.push("opt".to_string());
                }
                if modifiers.shift {
                    parts.push("shift".to_string());
                }
                if modifiers.command {
                    parts.push("cmd".to_string());
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
        for s in ["opt+space", "ctrl+cmd+v", "right_option", "fn", "ctrl+shift+d", "cmd+f5"] {
            let hk: Hotkey = s.parse().unwrap();
            assert_eq!(hk.to_string(), s);
        }
    }

    #[test]
    fn aliases_normalize() {
        let hk: Hotkey = "Option + Space".parse().unwrap();
        assert_eq!(hk, Hotkey::toggle_default());
        assert_eq!("control+command+V".parse::<Hotkey>().unwrap(), Hotkey::paste_last_default());
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!("".parse::<Hotkey>(), Err(HotkeyParseError::Empty));
        assert_eq!("opt+cmd".parse::<Hotkey>(), Err(HotkeyParseError::WrongKeyCount));
        assert_eq!("opt+a+b".parse::<Hotkey>(), Err(HotkeyParseError::WrongKeyCount));
        assert!(matches!("opt+banana".parse::<Hotkey>(), Err(HotkeyParseError::UnknownKey(_))));
    }

    #[test]
    fn keycaps_follow_mac_order() {
        let hk: Hotkey = "cmd+shift+ctrl+opt+k".parse().unwrap();
        assert_eq!(hk.keycaps(), vec!["⌃", "⌥", "⇧", "⌘", "K"]);
        assert_eq!(Hotkey::toggle_default().keycaps(), vec!["⌥", "Space"]);
    }
}
