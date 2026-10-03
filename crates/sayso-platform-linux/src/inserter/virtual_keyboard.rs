//! Key presses through the virtual keyboard protocol
//! (`zwp_virtual_keyboard_v1`), for sway, Hyprland, and other wlroots
//! compositors.
//!
//! A virtual keyboard brings its own keymap. So Sayso writes a keymap that
//! has exactly the keysyms it needs, one per key, and every character can be
//! typed, whatever the user's layout is (the approach of wtype). The user's
//! own keymap comes back with the next key press on the real keyboard.

use super::KeyInjector;
use super::keys::{self, PasteKey};
use super::wl;
use crate::session::Session;
use sayso_platform::InsertError;
use std::fmt::Write as _;
use std::io::Write as _;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry::WlRegistry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};

/// The most keys in one keymap. X11 apps on XWayland see key codes up to
/// 255 only, and the first usable code is 9.
pub const MAX_KEYS: usize = 240;
/// The wait for the apps to handle the keys before the keymap changes.
const SETTLE: Duration = Duration::from_millis(50);
/// `WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1`.
const KEYMAP_FORMAT_XKB_V1: u32 = 1;

/// An XKB keymap with one key for each keysym. Key `i` has the Linux key
/// code `i + 1` (XKB code `i + 9`). `modifiers` names the keys that are
/// modifiers, as `(index, "Shift")`. XWayland drops a keymap without a
/// modifier, so give at least one.
pub fn keymap(syms: &[u32], modifiers: &[(usize, &str)]) -> String {
    let mut map = String::from("xkb_keymap {\nxkb_keycodes \"sayso\" {\nminimum = 8;\n");
    let _ = writeln!(map, "maximum = {};", syms.len().max(1) + 8);
    for i in 0..syms.len() {
        let _ = writeln!(map, "<K{}> = {};", i + 1, i + 9);
    }
    map.push_str("};\nxkb_types \"sayso\" { include \"complete\" };\n");
    map.push_str("xkb_compatibility \"sayso\" { include \"complete\" };\nxkb_symbols \"sayso\" {\n");
    for (i, sym) in syms.iter().enumerate() {
        let _ = writeln!(map, "key <K{}> {{[ {sym:#x} ]}};", i + 1);
    }
    for (i, name) in modifiers {
        let _ = writeln!(map, "modifier_map {name} {{ <K{}> }};", i + 1);
    }
    map.push_str("};\n};\n");
    map
}

/// Split the keysyms of a text into parts of at most `max` distinct
/// keysyms. Each part gives its keysyms and, for each character, the index
/// of its key.
pub fn chunks(syms: &[u32], max: usize) -> Vec<(Vec<u32>, Vec<usize>)> {
    let mut parts: Vec<(Vec<u32>, Vec<usize>)> = Vec::new();
    for &sym in syms {
        let fits = parts.last().is_some_and(|(keys, _)| keys.contains(&sym) || keys.len() < max);
        if !fits {
            parts.push((Vec::new(), Vec::new()));
        }
        let (keys, presses) = parts.last_mut().expect("a part exists");
        let index = keys.iter().position(|k| *k == sym).unwrap_or_else(|| {
            keys.push(sym);
            keys.len() - 1
        });
        presses.push(index);
    }
    parts
}

struct Ignore;

impl Dispatch<WlRegistry, GlobalListContents> for Ignore {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as wayland_client::Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {
    }
}
delegate_noop!(Ignore: ignore WlSeat);
delegate_noop!(Ignore: ZwpVirtualKeyboardManagerV1);
delegate_noop!(Ignore: ZwpVirtualKeyboardV1);

fn failed(e: impl std::fmt::Display) -> InsertError {
    InsertError::Failed(format!("virtual keyboard: {e}"))
}

/// One virtual keyboard for one insertion.
struct Device {
    _conn: Connection,
    queue: EventQueue<Ignore>,
    keyboard: ZwpVirtualKeyboardV1,
    started: Instant,
}

impl Device {
    fn open(session: &Session) -> Result<Self, InsertError> {
        let conn = wl::connect(session).map_err(failed)?;
        let (globals, queue) = registry_queue_init::<Ignore>(&conn).map_err(failed)?;
        let qh = queue.handle();
        let seat: WlSeat = globals.bind(&qh, 1..=1, ()).map_err(failed)?;
        let manager: ZwpVirtualKeyboardManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|e| {
            log::warn!("the compositor has no virtual keyboard: {e}");
            InsertError::NotTrusted
        })?;
        let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
        Ok(Self { _conn: conn, queue, keyboard, started: Instant::now() })
    }

    fn upload(&self, map: &str) -> Result<(), InsertError> {
        // SAFETY: plain system call; the name is a valid C string.
        let fd = unsafe { libc::memfd_create(c"sayso-keymap".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(failed(std::io::Error::last_os_error()));
        }
        // SAFETY: `fd` is a new descriptor that nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut file = std::fs::File::from(fd);
        file.write_all(map.as_bytes()).and_then(|()| file.write_all(&[0])).map_err(failed)?;
        let size = u32::try_from(map.len() + 1).map_err(failed)?;
        self.keyboard.keymap(KEYMAP_FORMAT_XKB_V1, file.as_fd(), size);
        Ok(())
    }

    fn time(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    fn key(&self, index: usize, down: bool) {
        self.keyboard.key(self.time(), (index + 1) as u32, u32::from(down));
    }

    /// Wait until the compositor has passed the keys on, and give the apps
    /// time to handle them. XWayland handles keys with the keymap it has
    /// when it gets to them, not with the keymap they came with. So the
    /// keymap must not change (a new part, or the real keyboard after
    /// `destroy`) before it has handled them.
    fn settle(&mut self) -> Result<(), InsertError> {
        self.queue.roundtrip(&mut Ignore).map_err(failed)?;
        std::thread::sleep(SETTLE);
        Ok(())
    }

    /// Send everything, let the apps handle it, and remove the keyboard.
    fn finish(mut self) -> Result<(), InsertError> {
        let synced = self.settle();
        self.keyboard.destroy();
        let _ = self.queue.roundtrip(&mut Ignore);
        synced.map(|_| ())
    }
}

pub struct VirtualKeyboard {
    session: &'static Session,
}

impl VirtualKeyboard {
    pub fn new(session: &'static Session) -> Self {
        Self { session }
    }
}

impl KeyInjector for VirtualKeyboard {
    fn paste_key(&self, key: PasteKey) -> Result<(), InsertError> {
        let device = Device::open(self.session)?;
        let modifiers = key.modifiers();
        let mut syms: Vec<u32> = modifiers.iter().map(|m| m.keysym).collect();
        syms.push(key.key().keysym);
        let mut names: Vec<(usize, &str)> = modifiers
            .iter()
            .enumerate()
            .map(|(i, m)| (i, if m.keysym == keys::keysym::SHIFT_L { "Shift" } else { "Control" }))
            .collect();
        if !syms.contains(&keys::keysym::SHIFT_L) {
            // XWayland drops a keymap without Shift (see `type_text`).
            syms.push(keys::keysym::SHIFT_L);
            names.push((syms.len() - 1, "Shift"));
        }
        device.upload(&keymap(&syms, &names))?;
        let main = modifiers.len();
        let mut mask = 0;
        for (i, m) in modifiers.iter().enumerate() {
            device.key(i, true);
            mask |= if m.keysym == keys::keysym::SHIFT_L { 1 } else { 4 };
            device.keyboard.modifiers(mask, 0, 0, 0);
        }
        device.key(main, true);
        device.key(main, false);
        for i in (0..modifiers.len()).rev() {
            device.key(i, false);
        }
        device.keyboard.modifiers(0, 0, 0, 0);
        device.finish()
    }

    fn type_text(&self, text: &str) -> Result<(), InsertError> {
        let syms = keys::text_keysyms(text).map_err(|c| failed(format!("cannot type {c:?}")))?;
        let mut device = Device::open(self.session)?;
        let parts = chunks(&syms, MAX_KEYS - 1);
        let count = parts.len();
        for (n, (mut keys, presses)) in parts.into_iter().enumerate() {
            // XWayland drops a keymap without modifiers, so each part has a Shift key.
            keys.push(keys::keysym::SHIFT_L);
            device.upload(&keymap(&keys, &[(keys.len() - 1, "Shift")]))?;
            for index in presses {
                device.key(index, true);
                device.key(index, false);
            }
            if n + 1 < count {
                device.settle()?;
            }
        }
        device.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keymap_has_one_key_per_keysym_and_the_modifier_map() {
        let map = keymap(&[0xffe1, 0xff63], &[(0, "Shift")]);
        assert!(map.contains("maximum = 10;"));
        assert!(map.contains("<K1> = 9;"));
        assert!(map.contains("<K2> = 10;"));
        assert!(map.contains("key <K1> {[ 0xffe1 ]};"));
        assert!(map.contains("key <K2> {[ 0xff63 ]};"));
        assert!(map.contains("modifier_map Shift { <K1> };"));
        assert_eq!(map.matches('{').count(), map.matches('}').count());
    }

    #[test]
    fn repeated_characters_share_a_key() {
        let parts = chunks(&[0x61, 0x62, 0x61, 0x100_2013], 10);
        assert_eq!(parts, vec![(vec![0x61, 0x62, 0x100_2013], vec![0, 1, 0, 2])]);
    }

    #[test]
    fn a_new_part_starts_when_the_keymap_is_full() {
        let parts = chunks(&[1, 2, 1, 3, 2, 3], 2);
        assert_eq!(parts, vec![(vec![1, 2], vec![0, 1, 0]), (vec![3, 2], vec![0, 1, 0])]);
        assert!(chunks(&[], 2).is_empty());
    }
}
