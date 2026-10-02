//! Chord string parsing ("cmd+space") into keycode + modifier masks.
use crate::ffi::*;

#[derive(Debug, Clone, Copy)]
pub struct Chord {
    pub keycode: u32,
    /// Carbon modifier mask (cmdKey, optionKey, ...) for RegisterEventHotKey.
    pub carbon: u32,
    /// CGEventFlags mask, same encoding used by com.apple.symbolichotkeys.
    pub cg: u64,
}

const KEYS: &[(&str, u32)] = &[
    ("a", 0), ("s", 1), ("d", 2), ("f", 3), ("h", 4), ("g", 5), ("z", 6), ("x", 7),
    ("c", 8), ("v", 9), ("b", 11), ("q", 12), ("w", 13), ("e", 14), ("r", 15),
    ("y", 16), ("t", 17), ("1", 18), ("2", 19), ("3", 20), ("4", 21), ("6", 22),
    ("5", 23), ("9", 25), ("7", 26), ("8", 28), ("0", 29), ("o", 31), ("u", 32),
    ("i", 34), ("p", 35), ("return", 36), ("l", 37), ("j", 38), ("k", 40),
    ("n", 45), ("m", 46), ("tab", 48), ("space", 49), ("`", 50), ("delete", 51),
    ("esc", 53), ("escape", 53), ("f1", 122), ("f2", 120), ("f3", 99), ("f4", 118),
    ("f5", 96), ("f6", 97), ("f7", 98), ("f8", 100), ("f9", 101), ("f10", 109),
    ("f11", 103), ("f12", 111), ("left", 123), ("right", 124), ("down", 125), ("up", 126),
];

pub fn keycode_name(code: u32) -> String {
    KEYS.iter()
        .find(|(_, c)| *c == code)
        .map(|(n, _)| n.to_string())
        .unwrap_or_else(|| format!("keycode {code}"))
}

pub fn parse(s: &str) -> Result<Chord, String> {
    let (mut carbon, mut cg, mut key) = (0u32, 0u64, None);
    for part in s.to_lowercase().split('+') {
        match part.trim() {
            "cmd" | "command" => { carbon |= cmdKey; cg |= FLAG_COMMAND; }
            "opt" | "option" | "alt" => { carbon |= optionKey; cg |= FLAG_ALT; }
            "ctrl" | "control" => { carbon |= controlKey; cg |= FLAG_CONTROL; }
            "shift" => { carbon |= shiftKey; cg |= FLAG_SHIFT; }
            k => {
                let code = KEYS.iter().find(|(n, _)| *n == k).ok_or(format!("unknown key '{k}'"))?.1;
                if key.replace(code).is_some() {
                    return Err("more than one non-modifier key".into());
                }
            }
        }
    }
    Ok(Chord { keycode: key.ok_or("no key in chord")?, carbon, cg })
}

pub fn describe(c: &Chord) -> String {
    let mut s = String::new();
    for (f, n) in [(FLAG_COMMAND, "Cmd+"), (FLAG_ALT, "Opt+"), (FLAG_CONTROL, "Ctrl+"), (FLAG_SHIFT, "Shift+")] {
        if c.cg & f != 0 { s.push_str(n); }
    }
    s + &keycode_name(c.keycode)
}
