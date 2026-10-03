//! Keys without system code: the paste key, characters to X11 keysyms, and
//! characters to Linux key codes under a US layout.

/// Keysyms that the key injection uses.
pub mod keysym {
    pub const SHIFT_L: u32 = 0xffe1;
    pub const CONTROL_L: u32 = 0xffe3;
    pub const INSERT: u32 = 0xff63;
    pub const RETURN: u32 = 0xff0d;
    pub const TAB: u32 = 0xff09;
    pub const LOWER_V: u32 = 0x76;
    /// The empty slot in an X11 keyboard mapping.
    pub const NO_SYMBOL: u32 = 0;
}

/// Linux input key codes (`linux/input-event-codes.h`).
pub mod code {
    pub const LEFTCTRL: u16 = 29;
    pub const LEFTSHIFT: u16 = 42;
    pub const INSERT: u16 = 110;
    pub const V: u16 = 47;
}

/// The key that tells the focused app to paste.
///
/// The default is Shift+Insert. GTK, Qt, Chromium, Electron, Firefox,
/// LibreOffice, and VTE terminals paste with it, and so do xterm, kitty,
/// alacritty, and foot. Some of these apps paste the primary selection
/// instead of the clipboard. The paste puts the text in both, so either
/// works. Ctrl+V is not the default, because a terminal sends it to the
/// program in the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteKey {
    ShiftInsert,
    CtrlV,
    CtrlShiftV,
}

/// One key of a key combination: its keysym and its Linux key code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub keysym: u32,
    pub code: u16,
}

pub const SHIFT: Key = Key { keysym: keysym::SHIFT_L, code: code::LEFTSHIFT };
pub const CONTROL: Key = Key { keysym: keysym::CONTROL_L, code: code::LEFTCTRL };

impl PasteKey {
    /// Read `SAYSO_PASTE_KEY` (`shift+insert`, `ctrl+v`, `ctrl+shift+v`).
    /// Shift+Insert when the variable is not set or not known.
    pub fn from_env() -> Self {
        match std::env::var("SAYSO_PASTE_KEY") {
            Ok(value) => Self::parse(&value).unwrap_or_else(|| {
                log::warn!("SAYSO_PASTE_KEY={value:?} is not known; Sayso uses shift+insert");
                Self::ShiftInsert
            }),
            Err(_) => Self::ShiftInsert,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let mut parts: Vec<String> = value.split('+').map(|p| p.trim().to_ascii_lowercase()).collect();
        parts.sort();
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        match parts.as_slice() {
            ["insert", "shift"] | ["ins", "shift"] => Some(Self::ShiftInsert),
            ["ctrl", "v"] | ["control", "v"] => Some(Self::CtrlV),
            ["ctrl", "shift", "v"] | ["control", "shift", "v"] => Some(Self::CtrlShiftV),
            _ => None,
        }
    }

    /// The modifiers to hold, in press order.
    pub fn modifiers(self) -> &'static [Key] {
        match self {
            Self::ShiftInsert => &[SHIFT],
            Self::CtrlV => &[CONTROL],
            Self::CtrlShiftV => &[CONTROL, SHIFT],
        }
    }

    /// The key to press while the modifiers are down.
    pub fn key(self) -> Key {
        match self {
            Self::ShiftInsert => Key { keysym: keysym::INSERT, code: code::INSERT },
            Self::CtrlV | Self::CtrlShiftV => Key { keysym: keysym::LOWER_V, code: code::V },
        }
    }

    /// The X11 modifier mask of the modifiers (Shift is bit 0, Control bit 2).
    pub fn modifier_mask(self) -> u32 {
        self.modifiers().iter().map(|m| if m.keysym == keysym::SHIFT_L { 1 } else { 4 }).fold(0, |a, b| a | b)
    }
}

/// The X11 keysym that types `c`. None for control characters other than
/// newline and tab.
///
/// Latin-1 characters have keysyms equal to their code point. Every other
/// character has the Unicode keysym `0x0100_0000 + code point`, which X11
/// and xkbcommon both understand.
pub fn char_to_keysym(c: char) -> Option<u32> {
    let cp = u32::from(c);
    match c {
        '\n' | '\r' => Some(keysym::RETURN),
        '\t' => Some(keysym::TAB),
        _ if cp < 0x20 || (0x7f..0xa0).contains(&cp) => None,
        _ if cp < 0x100 => Some(cp),
        _ => Some(0x0100_0000 + cp),
    }
}

/// The keysyms of `text`, or the first character that has none.
pub fn text_keysyms(text: &str) -> Result<Vec<u32>, char> {
    // "\r\n" is one line break, not two.
    let text = text.replace("\r\n", "\n");
    text.chars().map(|c| char_to_keysym(c).ok_or(c)).collect()
}

/// The Linux key code and Shift state that type `c` under a US layout.
///
/// The kernel sends key codes, and the desktop turns them into characters
/// with the user's layout. So this is right only under a US (or similar)
/// layout, and only for ASCII.
pub fn us_key(c: char) -> Option<(u16, bool)> {
    const LETTERS: [u16; 26] =
        [30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44];
    const DIGITS: [u16; 10] = [11, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    let key = match c {
        'a'..='z' => (LETTERS[(c as u8 - b'a') as usize], false),
        'A'..='Z' => (LETTERS[(c as u8 - b'A') as usize], true),
        '0'..='9' => (DIGITS[(c as u8 - b'0') as usize], false),
        ' ' => (57, false),
        '\n' => (28, false),
        '\t' => (15, false),
        '-' => (12, false),
        '_' => (12, true),
        '=' => (13, false),
        '+' => (13, true),
        '[' => (26, false),
        '{' => (26, true),
        ']' => (27, false),
        '}' => (27, true),
        ';' => (39, false),
        ':' => (39, true),
        '\'' => (40, false),
        '"' => (40, true),
        '`' => (41, false),
        '~' => (41, true),
        '\\' => (43, false),
        '|' => (43, true),
        ',' => (51, false),
        '<' => (51, true),
        '.' => (52, false),
        '>' => (52, true),
        '/' => (53, false),
        '?' => (53, true),
        '!' => (2, true),
        '@' => (3, true),
        '#' => (4, true),
        '$' => (5, true),
        '%' => (6, true),
        '^' => (7, true),
        '&' => (8, true),
        '*' => (9, true),
        '(' => (10, true),
        ')' => (11, true),
        _ => return None,
    };
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_keys_are_read_in_any_order_and_case() {
        assert_eq!(PasteKey::parse("shift+insert"), Some(PasteKey::ShiftInsert));
        assert_eq!(PasteKey::parse("Insert+Shift"), Some(PasteKey::ShiftInsert));
        assert_eq!(PasteKey::parse("ctrl+v"), Some(PasteKey::CtrlV));
        assert_eq!(PasteKey::parse("CTRL + V"), Some(PasteKey::CtrlV));
        assert_eq!(PasteKey::parse("ctrl+shift+v"), Some(PasteKey::CtrlShiftV));
        assert_eq!(PasteKey::parse("shift+ctrl+v"), Some(PasteKey::CtrlShiftV));
        assert_eq!(PasteKey::parse("alt+v"), None);
        assert_eq!(PasteKey::parse(""), None);
    }

    #[test]
    fn paste_keys_have_the_right_modifiers_and_mask() {
        assert_eq!(PasteKey::ShiftInsert.modifiers(), &[SHIFT]);
        assert_eq!(PasteKey::ShiftInsert.key().keysym, keysym::INSERT);
        assert_eq!(PasteKey::ShiftInsert.modifier_mask(), 1);
        assert_eq!(PasteKey::CtrlV.modifier_mask(), 4);
        assert_eq!(PasteKey::CtrlShiftV.modifier_mask(), 5);
        assert_eq!(PasteKey::CtrlV.key().code, code::V);
    }

    #[test]
    fn latin1_characters_keep_their_code_point() {
        assert_eq!(char_to_keysym('a'), Some(0x61));
        assert_eq!(char_to_keysym(' '), Some(0x20));
        assert_eq!(char_to_keysym('ü'), Some(0xfc));
        assert_eq!(char_to_keysym('ß'), Some(0xdf));
        assert_eq!(char_to_keysym('\u{a0}'), Some(0xa0));
    }

    #[test]
    fn other_characters_use_unicode_keysyms() {
        assert_eq!(char_to_keysym('–'), Some(0x0100_2013));
        assert_eq!(char_to_keysym('€'), Some(0x0100_20ac));
        assert_eq!(char_to_keysym('😀'), Some(0x0101_f600));
    }

    #[test]
    fn line_breaks_and_tabs_are_keys_and_other_controls_are_not() {
        assert_eq!(char_to_keysym('\n'), Some(keysym::RETURN));
        assert_eq!(char_to_keysym('\t'), Some(keysym::TAB));
        assert_eq!(char_to_keysym('\u{7}'), None);
        assert_eq!(char_to_keysym('\u{7f}'), None);
        assert_eq!(char_to_keysym('\u{85}'), None);
    }

    #[test]
    fn crlf_is_one_return() {
        assert_eq!(text_keysyms("a\r\nb"), Ok(vec![0x61, keysym::RETURN, 0x62]));
        assert_eq!(text_keysyms("a\u{7}"), Err('\u{7}'));
    }

    #[test]
    fn us_keys_cover_ascii() {
        assert_eq!(us_key('a'), Some((30, false)));
        assert_eq!(us_key('Z'), Some((44, true)));
        assert_eq!(us_key('0'), Some((11, false)));
        assert_eq!(us_key('!'), Some((2, true)));
        assert_eq!(us_key('?'), Some((53, true)));
        assert_eq!(us_key('ü'), None);
        for c in (0x20u8..0x7f).map(char::from) {
            assert!(us_key(c).is_some(), "no key for {c:?}");
        }
    }
}
