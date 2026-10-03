//! Hotkeys through the X server, for X11 sessions.
//!
//! - Chords use `XGrabKey` on the root window. The grab takes the key, so the
//!   focused app does not get it. A chord that another client grabbed already
//!   fails with `BadAccess`.
//! - The listen-only stream uses XInput2 raw key events from all master
//!   devices. Raw events arrive also while another client holds a grab, and
//!   they do not take the key from anyone.
//!
//! While one of our grabs is active, the server sends us no raw key release
//! events. The grab sends the core key events to us instead, so the event
//! thread feeds them into the key logic too. A key that arrives both ways is
//! harmless: a second press counts as auto-repeat, and a second release does
//! nothing.
//!
//! Both use one connection. Requests go out from the calling thread, and one
//! thread waits for events. A client message to a private window wakes that
//! thread when it must stop.

use super::ChordAction;
use super::keymap::{Chord, X11_OFFSET};
use super::listen::Listen;
use super::logic::KeyAction;
use parking_lot::Mutex;
use sayso_core::hotkey::Hotkey;
use sayso_platform::HotkeyEvent;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::Event;
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, GrabMode, ModMask, Window,
    WindowClass,
};
use x11rb::rust_connection::RustConnection;

#[derive(Debug, Clone, Copy)]
struct Grab {
    keycode: u8,
    /// The modifier mask without the lock bits.
    mods: u16,
    action: ChordAction,
}

/// The modifier bits of Alt, Super, and NumLock in this server's map.
#[derive(Debug, Clone, Copy)]
struct ModMasks {
    alt: u16,
    super_: u16,
    num_lock: u16,
}

impl ModMasks {
    fn of(&self, chord: &Chord) -> u16 {
        let m = chord.modifiers;
        let mut mask = 0;
        if m.control {
            mask |= u16::from(ModMask::CONTROL);
        }
        if m.shift {
            mask |= u16::from(ModMask::SHIFT);
        }
        if m.option {
            mask |= self.alt;
        }
        if m.command {
            mask |= self.super_;
        }
        mask
    }

    /// The bits that tell chords apart. Lock bits do not count.
    fn relevant(&self) -> u16 {
        u16::from(ModMask::CONTROL) | u16::from(ModMask::SHIFT) | self.alt | self.super_
    }

    /// The extra lock combinations to grab, so a chord works with CapsLock
    /// or NumLock on.
    fn lock_variants(&self) -> [u16; 4] {
        let caps = u16::from(ModMask::LOCK);
        [0, caps, self.num_lock, caps | self.num_lock]
    }
}

struct Shared {
    conn: RustConnection,
    root: Window,
    wake_window: Window,
    masks: ModMasks,
    grabs: Mutex<Vec<Grab>>,
    /// The raw key events are selected.
    raw: AtomicBool,
    stop: AtomicBool,
}

/// The X11 connection and its event thread. Dropping it stops the thread.
pub struct X11Keys {
    shared: Arc<Shared>,
    xi2: bool,
    join: Option<JoinHandle<()>>,
}

impl X11Keys {
    /// Connect to `display` (or `$DISPLAY`) and start the event thread.
    pub fn connect(display: Option<&str>, listen: Arc<Listen>) -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(display).map_err(|e| format!("cannot connect to the X server: {e}"))?;
        let root = conn.setup().roots[screen].root;
        let masks = mod_masks(&conn);
        log::debug!("X11 modifier masks: {masks:?}");
        enable_detectable_autorepeat(&conn);
        let xi2 = match conn.xinput_xi_query_version(2, 2).map_err(|e| e.to_string()).and_then(|c| c.reply().map_err(|e| e.to_string())) {
            Ok(reply) if (reply.major_version, reply.minor_version) >= (2, 1) => true,
            Ok(reply) => {
                log::warn!("XInput {}.{} has no raw key events", reply.major_version, reply.minor_version);
                false
            }
            Err(e) => {
                log::warn!("XInput2 is not available: {e}");
                false
            }
        };
        let wake_window = conn.generate_id().map_err(|e| e.to_string())?;
        conn.create_window(0, wake_window, root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new())
            .map_err(|e| e.to_string())?;
        conn.flush().map_err(|e| e.to_string())?;
        let shared = Arc::new(Shared { conn, root, wake_window, masks, grabs: Mutex::default(), raw: AtomicBool::new(false), stop: AtomicBool::new(false) });
        let thread_shared = shared.clone();
        let join = std::thread::Builder::new()
            .name("sayso-x11-keys".into())
            .spawn(move || run(thread_shared, listen))
            .map_err(|e| format!("cannot start the X11 key thread: {e}"))?;
        Ok(Self { shared, xi2, join: Some(join) })
    }

    /// Replace all grabs. Returns the chords that could not be grabbed, with
    /// the reason.
    pub fn set_grabs(&self, chords: &[(Hotkey, Chord, ChordAction)]) -> Vec<(Hotkey, String)> {
        let s = &self.shared;
        let mut grabs = s.grabs.lock();
        for old in grabs.drain(..) {
            for lock in s.masks.lock_variants() {
                let _ = s.conn.ungrab_key(old.keycode, s.root, ModMask::from(old.mods | lock));
            }
        }
        let mut failed = Vec::new();
        for (hotkey, chord, action) in chords {
            let Ok(keycode) = u8::try_from(chord.code + X11_OFFSET) else {
                failed.push((*hotkey, "the key has no X11 key code".to_string()));
                continue;
            };
            let mods = s.masks.of(chord);
            match grab(s, keycode, mods) {
                Ok(()) => grabs.push(Grab { keycode, mods, action: *action }),
                Err(reason) => failed.push((*hotkey, reason)),
            }
        }
        if let Err(e) = s.conn.flush() {
            log::warn!("cannot flush the X11 connection: {e}");
        }
        failed
    }

    /// Start or stop the XInput2 raw key events.
    pub fn listen_raw(&self, on: bool) -> Result<(), String> {
        if !self.xi2 {
            return Err("the X server has no XInput 2.1".into());
        }
        let s = &self.shared;
        let mask = if on {
            xinput::XIEventMask::RAW_KEY_PRESS | xinput::XIEventMask::RAW_KEY_RELEASE
        } else {
            xinput::XIEventMask::from(0u32)
        };
        let masks = [xinput::EventMask { deviceid: xinput::Device::ALL_MASTER.into(), mask: vec![mask] }];
        s.conn
            .xinput_xi_select_events(s.root, &masks)
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| format!("cannot select raw key events: {e}"))?;
        s.raw.store(on, Ordering::SeqCst);
        Ok(())
    }
}

impl Drop for X11Keys {
    fn drop(&mut self) {
        let s = &self.shared;
        s.stop.store(true, Ordering::SeqCst);
        let event = ClientMessageEvent::new(32, s.wake_window, AtomEnum::NOTICE, [0u32; 5]);
        let sent = s.conn.send_event(false, s.wake_window, EventMask::NO_EVENT, event).and_then(|_| s.conn.flush());
        if let Err(e) = sent {
            log::warn!("cannot wake the X11 key thread: {e}");
            return;
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Grab one chord with every lock combination. On failure, undo the
/// variants that worked.
fn grab(s: &Shared, keycode: u8, mods: u16) -> Result<(), String> {
    let mut done = Vec::new();
    for lock in s.masks.lock_variants() {
        let mask = ModMask::from(mods | lock);
        if done.contains(&(mods | lock)) {
            continue;
        }
        let result = s
            .conn
            .grab_key(false, s.root, mask, keycode, GrabMode::ASYNC, GrabMode::ASYNC)
            .map_err(ReplyError::from)
            .and_then(|cookie| cookie.check());
        match result {
            Ok(()) => done.push(mods | lock),
            Err(e) => {
                for m in done {
                    let _ = s.conn.ungrab_key(keycode, s.root, ModMask::from(m));
                }
                return Err(match e {
                    ReplyError::X11Error(err) if err.error_kind == x11rb::protocol::ErrorKind::Access => {
                        "another app already uses this shortcut".to_string()
                    }
                    other => format!("the X server refused the shortcut: {other:?}"),
                });
            }
        }
    }
    Ok(())
}

/// The event thread. Never panics: errors are logged.
fn run(s: Arc<Shared>, listen: Arc<Listen>) {
    // Grabbed keys that are down, to drop auto-repeat presses.
    let mut down: Vec<u8> = Vec::new();
    let mut ptt_down = false;
    loop {
        let event = match s.conn.wait_for_event() {
            Ok(event) => event,
            Err(e) => {
                log::error!("the X11 connection failed, hotkeys stop: {e}");
                break;
            }
        };
        if s.stop.load(Ordering::SeqCst) {
            break;
        }
        match event {
            Event::XinputRawKeyPress(e) => raw_key(&listen, e.detail, KeyAction::Down),
            Event::XinputRawKeyRelease(e) => raw_key(&listen, e.detail, KeyAction::Up),
            Event::KeyPress(e) => {
                if s.raw.load(Ordering::SeqCst) {
                    raw_key(&listen, e.detail.into(), KeyAction::Down);
                }
                if down.contains(&e.detail) {
                    continue;
                }
                let state = u16::from(e.state) & s.masks.relevant();
                let action = s.grabs.lock().iter().find(|g| g.keycode == e.detail && g.mods == state).map(|g| g.action);
                let Some(action) = action else { continue };
                down.push(e.detail);
                if listen.chords_muted() {
                    continue;
                }
                match action {
                    ChordAction::Press(event) => listen.send(event),
                    ChordAction::PushToTalk => {
                        ptt_down = true;
                        listen.send(HotkeyEvent::PushToTalkDown);
                    }
                }
            }
            Event::KeyRelease(e) => {
                if s.raw.load(Ordering::SeqCst) {
                    raw_key(&listen, e.detail.into(), KeyAction::Up);
                }
                down.retain(|k| *k != e.detail);
                // During the grab, the release of the key or of one of its
                // modifiers ends push-to-talk.
                if std::mem::take(&mut ptt_down) {
                    listen.send(HotkeyEvent::PushToTalkUp);
                }
            }
            Event::Error(e) => log::debug!("X11 error: {e:?}"),
            _ => {}
        }
    }
    if ptt_down {
        listen.send(HotkeyEvent::PushToTalkUp);
    }
    listen.reset_keys();
}

fn raw_key(listen: &Listen, detail: u32, action: KeyAction) {
    let Some(code) = detail.checked_sub(u32::from(X11_OFFSET)).and_then(|c| u16::try_from(c).ok()) else { return };
    listen.key(code, action);
}

/// Ask the server to send no fake key releases for auto-repeat. Repeats then
/// arrive as presses only, and the event thread drops them.
fn enable_detectable_autorepeat(conn: &RustConnection) {
    let flag = xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT;
    let result = conn
        .xkb_use_extension(1, 0)
        .map_err(ReplyError::from)
        .and_then(|c| c.reply())
        .and_then(|_| {
            conn.xkb_per_client_flags(
                xkb::ID::USE_CORE_KBD.into(),
                flag,
                flag,
                xkb::BoolCtrl::from(0u32),
                xkb::BoolCtrl::from(0u32),
                xkb::BoolCtrl::from(0u32),
            )
            .map_err(ReplyError::from)
        })
        .and_then(|c| c.reply());
    if let Err(e) = result {
        log::warn!("cannot turn on detectable auto-repeat: {e}");
    }
}

/// Find the modifier bits of Alt, Super, and NumLock in the server's
/// modifier map. Falls back to the usual Mod1, Mod4, and Mod2.
fn mod_masks(conn: &RustConnection) -> ModMasks {
    const NUM_LOCK: u32 = 0xff7f;
    const ALT: [u32; 2] = [0xffe9, 0xffea];
    const SUPER: [u32; 2] = [0xffeb, 0xffec];
    let mut masks =
        ModMasks { alt: u16::from(ModMask::M1), super_: u16::from(ModMask::M4), num_lock: u16::from(ModMask::M2) };
    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let modmap = conn.get_modifier_mapping().ok().and_then(|c| c.reply().ok());
    let keymap = conn.get_keyboard_mapping(min, max - min + 1).ok().and_then(|c| c.reply().ok());
    let (Some(modmap), Some(keymap)) = (modmap, keymap) else { return masks };
    let per_key = usize::from(keymap.keysyms_per_keycode);
    let per_mod = usize::from(modmap.keycodes_per_modifier());
    if per_key == 0 || per_mod == 0 {
        return masks;
    }
    let keysyms_of = |keycode: u8| -> &[u32] {
        let start = usize::from(keycode.saturating_sub(min)) * per_key;
        keymap.keysyms.get(start..start + per_key).unwrap_or(&[])
    };
    let (mut num_lock, mut alt, mut super_) = (None, None, None);
    // Mod1 to Mod5 only; Shift, Lock, and Control are fixed. The first match wins.
    for (index, keycodes) in modmap.keycodes.chunks(per_mod).enumerate().take(8).skip(3) {
        let bit = 1u16 << index;
        let syms: Vec<u32> = keycodes.iter().filter(|k| **k != 0).flat_map(|k| keysyms_of(*k).to_vec()).collect();
        if syms.contains(&NUM_LOCK) {
            num_lock.get_or_insert(bit);
        }
        if syms.iter().any(|s| ALT.contains(s)) {
            alt.get_or_insert(bit);
        }
        if syms.iter().any(|s| SUPER.contains(s)) {
            super_.get_or_insert(bit);
        }
    }
    masks.num_lock = num_lock.unwrap_or(masks.num_lock);
    masks.alt = alt.unwrap_or(masks.alt);
    masks.super_ = super_.unwrap_or(masks.super_);
    masks
}
