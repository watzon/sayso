//! Key presses through the XTest extension, for X11 sessions.
//!
//! XTest sends key codes. Sayso finds the key code of each keysym in the
//! keyboard mapping. A character that no key has gets a spare key code for
//! a moment: Sayso maps the keysym to an unused key code, types it, and
//! removes the mapping again (the approach of xdotool).

use super::KeyInjector;
use super::keys::{self, PasteKey, keysym};
use crate::session::Session;
use sayso_platform::InsertError;
use std::thread::sleep;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, Keycode, Window};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// The pause between two typed keys. Some apps drop keys that come faster.
const KEY_DELAY: Duration = Duration::from_millis(3);
/// The wait after a mapping change, so that apps load the new mapping
/// before the key comes.
const REMAP_DELAY: Duration = Duration::from_millis(60);

/// The keyboard mapping: `per` keysyms for each key code from `min`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyMap {
    pub min: Keycode,
    pub per: usize,
    pub syms: Vec<u32>,
}

/// One key to press: its key code, with or without Shift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Press {
    pub code: Keycode,
    pub shift: bool,
}

/// A part of the text: the spare key codes to map first, then the keys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Batch {
    pub remap: Vec<(Keycode, u32)>,
    pub presses: Vec<Press>,
}

impl KeyMap {
    fn keysyms_of(&self, index: usize) -> &[u32] {
        &self.syms[index * self.per..(index + 1) * self.per]
    }

    fn keycodes(&self) -> impl Iterator<Item = (Keycode, &[u32])> {
        let count = self.syms.len().checked_div(self.per).unwrap_or(0);
        (0..count).map(|i| (self.min.saturating_add(i as u8), self.keysyms_of(i)))
    }

    /// The key that gives `sym` on the first level (no Shift) or the second
    /// (Shift). The first level wins.
    pub fn find(&self, sym: u32) -> Option<Press> {
        for level in 0..2.min(self.per) {
            if let Some((code, _)) = self.keycodes().find(|(_, syms)| syms[level] == sym) {
                return Some(Press { code, shift: level == 1 });
            }
        }
        None
    }

    /// Key codes with no keysym at all.
    pub fn spare(&self) -> Vec<Keycode> {
        self.keycodes().filter(|(_, syms)| syms.iter().all(|s| *s == keysym::NO_SYMBOL)).map(|(c, _)| c).collect()
    }

    /// Split `syms` into batches. Each keysym that no key has gets a spare
    /// key code; when the spare codes run out, a new batch starts.
    pub fn plan(&self, syms: &[u32]) -> Result<Vec<Batch>, String> {
        let spare = self.spare();
        let mut batches = vec![Batch::default()];
        for &sym in syms {
            let batch = batches.last_mut().expect("there is always one batch");
            if let Some(press) = self.find(sym) {
                batch.presses.push(press);
                continue;
            }
            if let Some((code, _)) = batch.remap.iter().find(|(_, s)| *s == sym) {
                batch.presses.push(Press { code: *code, shift: false });
                continue;
            }
            if spare.is_empty() {
                return Err(format!("no free key code to type keysym {sym:#x}"));
            }
            if batch.remap.len() == spare.len() {
                batches.push(Batch::default());
            }
            let batch = batches.last_mut().expect("there is always one batch");
            let code = spare[batch.remap.len()];
            batch.remap.push((code, sym));
            batch.presses.push(Press { code, shift: false });
        }
        batches.retain(|b| !b.presses.is_empty());
        Ok(batches)
    }
}

pub struct XTest {
    conn: RustConnection,
    root: Window,
}

fn failed(e: impl std::fmt::Display) -> InsertError {
    InsertError::Failed(format!("XTest: {e}"))
}

impl XTest {
    pub fn connect(session: &Session) -> Result<Self, String> {
        let display = session.x11_display.as_ref().and_then(|d| d.to_str());
        let (conn, screen) = x11rb::connect(display).map_err(|e| e.to_string())?;
        let root = conn.setup().roots.get(screen).ok_or("no X11 screen")?.root;
        let version = conn.xtest_get_version(2, 2).map_err(|e| e.to_string())?.reply();
        version.map_err(|e| format!("the X server has no XTest extension: {e}"))?;
        Ok(Self { conn, root })
    }

    fn keymap(&self) -> Result<KeyMap, InsertError> {
        let setup = self.conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let reply = self.conn.get_keyboard_mapping(min, max - min + 1).map_err(failed)?.reply().map_err(failed)?;
        Ok(KeyMap { min, per: usize::from(reply.keysyms_per_keycode), syms: reply.keysyms })
    }

    fn key(&self, code: Keycode, down: bool) -> Result<(), InsertError> {
        let kind = if down { KEY_PRESS_EVENT } else { KEY_RELEASE_EVENT };
        self.conn.xtest_fake_input(kind, code, x11rb::CURRENT_TIME, self.root, 0, 0, 0).map_err(failed)?;
        Ok(())
    }

    fn press(&self, press: Press, shift: Keycode) -> Result<(), InsertError> {
        if press.shift {
            self.key(shift, true)?;
        }
        self.key(press.code, true)?;
        self.key(press.code, false)?;
        if press.shift {
            self.key(shift, false)?;
        }
        self.conn.flush().map_err(failed)
    }

    /// Give each `(code, sym)` its keysym on both levels, or none.
    fn remap(&self, map: &KeyMap, codes: &[(Keycode, u32)], clear: bool) -> Result<(), InsertError> {
        for &(code, sym) in codes {
            let mut syms = vec![keysym::NO_SYMBOL; map.per];
            if !clear {
                syms.iter_mut().take(2).for_each(|s| *s = sym);
            }
            self.conn.change_keyboard_mapping(1, code, map.per as u8, &syms).map_err(failed)?;
        }
        self.conn.sync().map_err(failed)
    }
}

impl KeyInjector for XTest {
    fn paste_key(&self, key: PasteKey) -> Result<(), InsertError> {
        let map = self.keymap()?;
        let code = |sym: u32| map.find(sym).map(|p| p.code).ok_or_else(|| failed(format!("no key for {sym:#x}")));
        let modifiers: Vec<Keycode> = key.modifiers().iter().map(|m| code(m.keysym)).collect::<Result<_, _>>()?;
        let main = code(key.key().keysym)?;
        for m in &modifiers {
            self.key(*m, true)?;
        }
        self.key(main, true)?;
        self.key(main, false)?;
        for m in modifiers.iter().rev() {
            self.key(*m, false)?;
        }
        self.conn.sync().map_err(failed)
    }

    fn type_text(&self, text: &str) -> Result<(), InsertError> {
        let syms = keys::text_keysyms(text).map_err(|c| failed(format!("cannot type {c:?}")))?;
        let map = self.keymap()?;
        let shift = map.find(keysym::SHIFT_L).map(|p| p.code).ok_or_else(|| failed("no Shift key"))?;
        let batches = map.plan(&syms).map_err(failed)?;
        for batch in batches {
            if !batch.remap.is_empty() {
                self.remap(&map, &batch.remap, false)?;
                sleep(REMAP_DELAY);
            }
            let typed = batch.presses.iter().try_for_each(|p| {
                sleep(KEY_DELAY);
                self.press(*p, shift)
            });
            if !batch.remap.is_empty() {
                // The apps must handle the keys before the mapping goes away.
                let _ = self.conn.sync();
                sleep(REMAP_DELAY);
                self.remap(&map, &batch.remap, true)?;
            }
            typed?;
        }
        self.conn.sync().map_err(failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Key codes 8 to 12: a/A, b/B, Shift_L, nothing, nothing.
    fn map() -> KeyMap {
        KeyMap { min: 8, per: 2, syms: vec![0x61, 0x41, 0x62, 0x42, keysym::SHIFT_L, 0, 0, 0, 0, 0] }
    }

    #[test]
    fn keys_are_found_on_both_levels() {
        assert_eq!(map().find(0x61), Some(Press { code: 8, shift: false }));
        assert_eq!(map().find(0x42), Some(Press { code: 9, shift: true }));
        assert_eq!(map().find(keysym::SHIFT_L), Some(Press { code: 10, shift: false }));
        assert_eq!(map().find(0xfc), None);
        assert_eq!(map().spare(), vec![11, 12]);
    }

    #[test]
    fn missing_keysyms_get_spare_codes_and_reuse_them() {
        let batches = map().plan(&[0x61, 0xfc, 0x41, 0xfc, 0xdf]).unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].remap, vec![(11, 0xfc), (12, 0xdf)]);
        let codes: Vec<(u8, bool)> = batches[0].presses.iter().map(|p| (p.code, p.shift)).collect();
        assert_eq!(codes, vec![(8, false), (11, false), (8, true), (11, false), (12, false)]);
    }

    #[test]
    fn a_new_batch_starts_when_the_spare_codes_run_out() {
        let batches = map().plan(&[0x100, 0x101, 0x102, 0x61]).unwrap();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].remap, vec![(11, 0x100), (12, 0x101)]);
        assert_eq!(batches[1].remap, vec![(11, 0x102)]);
        assert_eq!(batches[1].presses.len(), 2);
    }

    #[test]
    fn a_full_mapping_cannot_type_new_keysyms() {
        let full = KeyMap { min: 8, per: 1, syms: vec![0x61] };
        assert!(full.plan(&[0x61]).is_ok());
        assert!(full.plan(&[0xfc]).is_err());
    }
}
