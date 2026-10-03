//! Key presses through a virtual keyboard in the kernel (`/dev/uinput`),
//! the last resort on Wayland.
//!
//! It needs write access to `/dev/uinput`, which the setup guide explains (a
//! udev rule for the `input` group). The kernel sends key codes, and the
//! desktop turns them into characters with the user's layout. So the paste
//! key works with every common layout (Shift, Ctrl, Insert, and V are in the
//! same places), but typing works only for ASCII under a US layout.

use super::KeyInjector;
use super::keys::{self, PasteKey};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode};
use parking_lot::Mutex;
use sayso_platform::InsertError;
use std::thread::sleep;
use std::time::Duration;

const PATH: &std::ffi::CStr = c"/dev/uinput";
/// The wait after the device is made, so that the compositor finds it
/// before the first key.
const NEW_DEVICE_DELAY: Duration = Duration::from_millis(300);
const KEY_DELAY: Duration = Duration::from_millis(2);

/// The device, made on first use and kept for the life of the app.
static DEVICE: Mutex<Option<VirtualDevice>> = Mutex::new(None);

/// Whether Sayso can write to `/dev/uinput`.
pub fn writable() -> bool {
    // SAFETY: plain system call on a valid C string.
    unsafe { libc::access(PATH.as_ptr(), libc::W_OK) == 0 }
}

fn failed(e: impl std::fmt::Display) -> InsertError {
    InsertError::Failed(format!("uinput: {e}"))
}

fn make_device() -> std::io::Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    for code in [keys::code::LEFTSHIFT, keys::code::LEFTCTRL, keys::code::INSERT] {
        keys.insert(KeyCode::new(code));
    }
    for c in (0x20u8..0x7f).map(char::from).chain(['\n', '\t']) {
        if let Some((code, _)) = keys::us_key(c) {
            keys.insert(KeyCode::new(code));
        }
    }
    let device = VirtualDevice::builder()?.name("Sayso virtual keyboard").with_keys(&keys)?.build()?;
    sleep(NEW_DEVICE_DELAY);
    Ok(device)
}

/// Send `(code, down)` pairs, one sync report each.
fn send(presses: &[(u16, bool)]) -> Result<(), InsertError> {
    let mut device = DEVICE.lock();
    if device.is_none() {
        *device = Some(make_device().map_err(|e| {
            log::warn!("cannot make the uinput keyboard: {e}");
            InsertError::NotTrusted
        })?);
    }
    let Some(d) = device.as_mut() else { return Err(InsertError::NotTrusted) };
    for &(code, down) in presses {
        if let Err(e) = d.emit(&[InputEvent::new(EventType::KEY.0, code, i32::from(down))]) {
            *device = None;
            return Err(failed(e));
        }
        sleep(KEY_DELAY);
    }
    Ok(())
}

/// The key codes that type `text` under a US layout.
pub fn presses_for(text: &str) -> Result<Vec<(u16, bool)>, char> {
    let mut presses = Vec::new();
    for c in text.replace("\r\n", "\n").chars() {
        let (code, shift) = keys::us_key(c).ok_or(c)?;
        if shift {
            presses.push((keys::code::LEFTSHIFT, true));
        }
        presses.extend([(code, true), (code, false)]);
        if shift {
            presses.push((keys::code::LEFTSHIFT, false));
        }
    }
    Ok(presses)
}

pub struct Uinput;

impl KeyInjector for Uinput {
    fn paste_key(&self, key: PasteKey) -> Result<(), InsertError> {
        let mut presses: Vec<(u16, bool)> = key.modifiers().iter().map(|m| (m.code, true)).collect();
        presses.extend([(key.key().code, true), (key.key().code, false)]);
        presses.extend(key.modifiers().iter().rev().map(|m| (m.code, false)));
        send(&presses)
    }

    fn type_text(&self, text: &str) -> Result<(), InsertError> {
        let presses = presses_for(text)
            .map_err(|c| InsertError::Failed(format!("the uinput keyboard can type ASCII only, not {c:?}")))?;
        send(&presses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capitals_are_typed_with_shift() {
        assert_eq!(presses_for("aB"), Ok(vec![(30, true), (30, false), (42, true), (48, true), (48, false), (42, false)]));
    }

    #[test]
    fn non_ascii_text_is_refused_before_any_key() {
        assert_eq!(presses_for("Grüße"), Err('ü'));
    }
}
