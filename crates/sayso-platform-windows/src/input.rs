//! Synthetic keys with `SendInput`.
//!
//! The key lists are built by pure functions, so the order of the keys is
//! unit tested. Only [`send`] and [`held_modifiers`] call Windows.
//!
//! Windows marks every key sent here as injected. The keyboard hook ignores
//! injected keys, so Sayso never triggers its own hotkeys.

use crate::keymap::{self, VK_LCONTROL, VK_MASK, VK_RETURN, VK_V};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput, VIRTUAL_KEY,
};

/// Tag in `dwExtraInfo` of every key Sayso sends ("SAYS").
pub const EXTRA_INFO: usize = 0x5341_5953;

/// One synthetic key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stroke {
    /// A key by virtual-key code.
    Key { vk: u16, down: bool },
    /// One UTF-16 unit, delivered to the app as a character.
    Unicode { unit: u16, down: bool },
}

fn tap(vk: u16) -> [Stroke; 2] {
    [Stroke::Key { vk, down: true }, Stroke::Key { vk, down: false }]
}

/// The modifier keys the user holds now. `with_ctrl` includes the Ctrl keys.
pub fn held_modifiers(with_ctrl: bool) -> Vec<u16> {
    keymap::MODIFIER_VKS
        .into_iter()
        .filter(|vk| with_ctrl || !matches!(*vk, keymap::VK_LCONTROL | keymap::VK_RCONTROL))
        // SAFETY: plain key state query.
        .filter(|vk| unsafe { GetAsyncKeyState(i32::from(*vk)) } < 0)
        .collect()
}

/// The mask key: a key with no meaning, pressed and released. Sent while Alt
/// or Win is down, it stops their release from opening a menu.
pub fn mask_strokes() -> [Stroke; 2] {
    tap(VK_MASK)
}

/// Ctrl+V, in two parts with a short gap between them.
///
/// `held` are the Alt, Shift, and Win keys the user still holds. They would
/// turn the paste into another shortcut, so they are released first. Ctrl
/// goes down before them: a key between an Alt press and its release stops
/// the release from opening the menu bar.
pub fn paste_strokes(held: &[u16]) -> (Vec<Stroke>, Vec<Stroke>) {
    let mut first = vec![Stroke::Key { vk: VK_LCONTROL, down: true }];
    first.extend(held.iter().map(|vk| Stroke::Key { vk: *vk, down: false }));
    first.push(Stroke::Key { vk: VK_V, down: true });
    let second = vec![Stroke::Key { vk: VK_V, down: false }, Stroke::Key { vk: VK_LCONTROL, down: false }];
    (first, second)
}

/// Release every modifier the user holds before typing, so the characters do
/// not become shortcuts. The mask key goes first, for the same reason as the
/// Ctrl in [`paste_strokes`].
pub fn release_strokes(held: &[u16]) -> Vec<Stroke> {
    if held.is_empty() {
        return Vec::new();
    }
    let mut strokes = mask_strokes().to_vec();
    strokes.extend(held.iter().map(|vk| Stroke::Key { vk: *vk, down: false }));
    strokes
}

/// Key events that type these UTF-16 units. A line break becomes the Return
/// key, because many apps ignore a line break character.
pub fn text_strokes(units: &[u16]) -> Vec<Stroke> {
    let mut strokes = Vec::with_capacity(units.len() * 2);
    for &unit in units {
        if unit == u16::from(b'\n') || unit == u16::from(b'\r') {
            strokes.extend(tap(VK_RETURN));
        } else {
            strokes.push(Stroke::Unicode { unit, down: true });
            strokes.push(Stroke::Unicode { unit, down: false });
        }
    }
    strokes
}

fn to_input(stroke: Stroke) -> INPUT {
    let (vk, scan, mut flags, down) = match stroke {
        Stroke::Key { vk, down } => {
            // SAFETY: plain table lookup.
            let scan = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) } as u16;
            let flags = if keymap::is_extended(vk) { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) };
            (vk, scan, flags, down)
        }
        Stroke::Unicode { unit, down } => (0, unit, KEYEVENTF_UNICODE, down),
    };
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: EXTRA_INFO },
        },
    }
}

/// Send the strokes in one `SendInput` call, so no user key lands between them.
pub fn send(strokes: &[Stroke]) -> Result<(), String> {
    if strokes.is_empty() {
        return Ok(());
    }
    let inputs: Vec<INPUT> = strokes.iter().copied().map(to_input).collect();
    // SAFETY: the slice and the struct size describe valid INPUT records.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(format!("Windows took {sent} of {} synthetic keys: {}", inputs.len(), windows::core::Error::from_thread()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{VK_LMENU, VK_RMENU, VK_RSHIFT, VK_RWIN};

    fn key(vk: u16, down: bool) -> Stroke {
        Stroke::Key { vk, down }
    }

    #[test]
    fn a_plain_paste_is_ctrl_v() {
        let (first, second) = paste_strokes(&[]);
        assert_eq!(first, [key(VK_LCONTROL, true), key(VK_V, true)]);
        assert_eq!(second, [key(VK_V, false), key(VK_LCONTROL, false)]);
    }

    #[test]
    fn held_modifiers_are_released_after_ctrl_goes_down() {
        let (first, _) = paste_strokes(&[VK_RMENU, VK_RSHIFT]);
        assert_eq!(
            first,
            [key(VK_LCONTROL, true), key(VK_RMENU, false), key(VK_RSHIFT, false), key(VK_V, true)]
        );
    }

    #[test]
    fn typing_releases_held_keys_behind_the_mask_key() {
        assert!(release_strokes(&[]).is_empty());
        assert_eq!(
            release_strokes(&[VK_LMENU, VK_RWIN]),
            [key(VK_MASK, true), key(VK_MASK, false), key(VK_LMENU, false), key(VK_RWIN, false)]
        );
    }

    #[test]
    fn text_becomes_unicode_pairs_and_line_breaks_become_return() {
        let units: Vec<u16> = "a\nb".encode_utf16().collect();
        assert_eq!(
            text_strokes(&units),
            [
                Stroke::Unicode { unit: 'a' as u16, down: true },
                Stroke::Unicode { unit: 'a' as u16, down: false },
                key(VK_RETURN, true),
                key(VK_RETURN, false),
                Stroke::Unicode { unit: 'b' as u16, down: true },
                Stroke::Unicode { unit: 'b' as u16, down: false },
            ]
        );
    }

    #[test]
    fn surrogate_pairs_go_out_unit_by_unit() {
        let units: Vec<u16> = "😀".encode_utf16().collect();
        let strokes = text_strokes(&units);
        assert_eq!(strokes.len(), 4);
        assert_eq!(strokes[0], Stroke::Unicode { unit: 0xD83D, down: true });
        assert_eq!(strokes[2], Stroke::Unicode { unit: 0xDE00, down: true });
    }

    #[test]
    fn inputs_carry_the_right_flags() {
        let up = to_input(key(keymap::VK_RMENU, false));
        // SAFETY: the union holds a keyboard input.
        let ki = unsafe { up.Anonymous.ki };
        assert_eq!(up.r#type, INPUT_KEYBOARD);
        assert_eq!(ki.dwFlags, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP, "Right Alt is an extended key");
        assert_eq!(ki.dwExtraInfo, EXTRA_INFO);
        let unicode = to_input(Stroke::Unicode { unit: 0x00E9, down: true });
        // SAFETY: as above.
        let ki = unsafe { unicode.Anonymous.ki };
        assert_eq!((ki.wVk, ki.wScan, ki.dwFlags), (VIRTUAL_KEY(0), 0x00E9, KEYEVENTF_UNICODE));
    }
}
