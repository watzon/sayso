//! [`TextInserter`] for X11 and Wayland: put text into the focused app.
//!
//! The Insertion has two methods, as on macOS:
//! - **Paste**: save the clipboard, put our text on the clipboard and on the
//!   primary selection, send the paste key, and wait until an app reads the
//!   text. Then restore the old content if nobody changed it meanwhile. With
//!   no read, the paste failed, and the text stays on the clipboard.
//! - **Type**: send one key press for each character.
//!
//! Two parts do the work, each with a backend per desktop:
//!
//! | Desktop | Clipboard | Keys |
//! |---|---|---|
//! | X11 session | X11 selections | XTest |
//! | GNOME (Wayland) | X11 selections through XWayland | remote desktop portal, else `/dev/uinput` |
//! | KDE Plasma (Wayland) | data control | remote desktop portal, else `/dev/uinput` |
//! | sway, Hyprland, other wlroots | data control | virtual keyboard protocol |
//!
//! The paste key is Shift+Insert (see [`keys::PasteKey`]). Set
//! `SAYSO_PASTE_KEY=ctrl+v` or `=ctrl+shift+v` to change it. For tests,
//! `SAYSO_CLIPBOARD=x11`, `=ext`, or `=wlr` selects the clipboard backend.
//!
//! **Receipts.** Both clipboard backends serve the text on request, so Sayso
//! sees each read: the X11 server sends a selection request, and the
//! compositor sends a data-control `send` event. A read after the paste key
//! is the receipt. On GNOME, Mutter bridges the X11 clipboard to Wayland apps,
//! also when no X11 window has focus. The bridge reads the X11 selection when
//! a Wayland app pastes, so that read is a real receipt too. Clipboard
//! managers can read each new clipboard at once. Sayso waits
//! [`SETTLE_BEFORE_KEY`] before the paste key, so that such reads come before
//! the key and do not count, and it offers [`PASSWORD_HINT`] so that managers
//! that know the marker do not read at all.
//!
//! **Limits.**
//! - The restore works only while Sayso runs: Sayso serves the old content
//!   itself. When Sayso quits, a clipboard that it restored is empty.
//! - The remote desktop portal and uinput cannot type every character: GNOME
//!   drops keysyms that the active layout does not have, and uinput types
//!   ASCII under a US layout only. The paste is not affected.
//! - Sayso cannot see a password field or a missing text field on Linux, so
//!   it never returns `SecureInput` or `NoFocusedField`.

mod clipboard_wayland;
mod clipboard_x11;
pub mod keys;
pub mod portal;
mod uinput;
mod virtual_keyboard;
mod wl;
mod xtest;

use crate::paste_receipt::{Receipts, Verdict};
use crate::session::{Session, SessionKind};
use keys::PasteKey;
use parking_lot::Mutex;
use sayso_platform::{InsertError, InsertMethod, InsertResult, PermissionState, TextInserter};
use std::sync::{Arc, OnceLock};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// How often the paste looks for a read of the text.
const RECEIPT_POLL: Duration = Duration::from_millis(10);
/// The wait between taking the clipboard and the paste key. Clipboard
/// managers and Mutter read a new clipboard at once; their reads must come
/// before the key, so that they do not look like the receipt.
pub const SETTLE_BEFORE_KEY: Duration = Duration::from_millis(50);
/// The marker that tells clipboard managers to skip an entry (Klipper,
/// CopyQ, and `wl-paste --watch` know it).
pub const PASSWORD_HINT: &str = "x-kde-passwordManagerHint";
/// The formats that carry our text. The names are X11 targets and MIME
/// types at the same time; both backends offer all of them.
pub const TEXT_FORMATS: [&str; 5] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING", "TEXT"];

/// The two selections that the paste fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Selection {
    /// What Ctrl+C and Ctrl+V use.
    Clipboard,
    /// The last selected text, pasted with the middle button or Shift+Insert.
    Primary,
}

impl Selection {
    pub const BOTH: [Selection; 2] = [Selection::Clipboard, Selection::Primary];

    fn index(self) -> usize {
        match self {
            Selection::Clipboard => 0,
            Selection::Primary => 1,
        }
    }
}

/// What a selection holds: each format name with its bytes. Empty means
/// that nothing owns the selection.
pub type Content = Vec<(String, Vec<u8>)>;

/// The two selections before the paste. None for a selection that could
/// not be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub clipboard: Option<Content>,
    pub primary: Option<Content>,
}

impl Snapshot {
    fn get(&self, selection: Selection) -> Option<&Content> {
        match selection {
            Selection::Clipboard => self.clipboard.as_ref(),
            Selection::Primary => self.primary.as_ref(),
        }
    }
}

/// The reads of one paste, shared with the clipboard backend.
pub type SharedReceipts = Arc<Mutex<Receipts>>;

/// A clipboard backend. It keeps serving what it owns until another app
/// takes the selection.
pub trait Clipboard: Send + Sync {
    /// Read both selections.
    fn snapshot(&self) -> Snapshot;
    /// Own `selection` with `content`. Each read of a text format goes to
    /// `receipts`. Returns a number that [`Clipboard::still_ours`] checks.
    fn publish(&self, selection: Selection, content: Content, receipts: Option<SharedReceipts>) -> Result<u64, String>;
    /// Whether `selection` still holds the content of `publish` number `generation`.
    fn still_ours(&self, selection: Selection, generation: u64) -> bool;
    /// Empty `selection`, if Sayso owns it.
    fn clear(&self, selection: Selection);
}

/// A way to send key presses to the focused app.
pub trait KeyInjector {
    fn paste_key(&self, key: PasteKey) -> Result<(), InsertError>;
    fn type_text(&self, text: &str) -> Result<(), InsertError>;
}

/// The content that carries `text`. `transient` adds the marker for
/// clipboard managers.
pub fn text_content(text: &str, transient: bool) -> Content {
    let mut content: Content = TEXT_FORMATS.iter().map(|f| (f.to_string(), text.as_bytes().to_vec())).collect();
    if transient {
        content.push((PASSWORD_HINT.to_string(), b"secret".to_vec()));
    }
    content
}

/// Whether a read of `format` shows that an app took the text. A clipboard
/// manager reads the marker only to decide whether to skip the entry.
pub fn read_counts(format: &str) -> bool {
    format != PASSWORD_HINT && format != "TARGETS"
}

// ---------------------------------------------------------------------------
// Paste with a read receipt
// ---------------------------------------------------------------------------

/// Paste `text` through `clipboard` and wait for the receipt.
///
/// `send_key` sends the paste key. `quiet` is how long the reads must have
/// stopped before the old content comes back.
///
/// With no read, the app did not take the text. The text then stays on the
/// clipboard (without the marker, so clipboard managers keep it): the user
/// can paste it by hand, and an app that reads very late cannot get stale
/// content.
pub fn paste_with_receipt(
    clipboard: &dyn Clipboard,
    text: &str,
    restore_clipboard: bool,
    quiet: Duration,
    settle: Duration,
    send_key: impl FnOnce() -> Result<(), InsertError>,
) -> Result<InsertResult, InsertError> {
    let saved = restore_clipboard.then(|| clipboard.snapshot());
    let receipts: SharedReceipts = Arc::default();
    let ours = clipboard
        .publish(Selection::Clipboard, text_content(text, restore_clipboard), Some(receipts.clone()))
        .map_err(|e| InsertError::Failed(format!("Sayso cannot use the clipboard: {e}")))?;
    let ours_primary = clipboard
        .publish(Selection::Primary, text_content(text, restore_clipboard), Some(receipts.clone()))
        .inspect_err(|e| log::warn!("cannot set the primary selection: {e}"))
        .ok();
    sleep(settle);
    // Before the key: a fast app can read while the key is still down.
    receipts.lock().mark_sent(Instant::now());
    if let Err(e) = send_key() {
        keep_text(clipboard, text, restore_clipboard, ours);
        return Err(e);
    }
    let verdict = loop {
        match receipts.lock().verdict(Instant::now(), quiet) {
            Verdict::Wait => {}
            done => break done,
        }
        sleep(RECEIPT_POLL);
    };
    let still_ours = clipboard.still_ours(Selection::Clipboard, ours);
    match verdict {
        Verdict::Taken => {
            let clipboard_restored = match &saved {
                Some(saved) => {
                    if ours_primary.is_some_and(|g| clipboard.still_ours(Selection::Primary, g)) {
                        restore(clipboard, Selection::Primary, saved);
                    }
                    still_ours && restore(clipboard, Selection::Clipboard, saved)
                }
                None => false,
            };
            Ok(InsertResult::Pasted { clipboard_restored })
        }
        Verdict::NotTaken | Verdict::Wait => {
            keep_text(clipboard, text, restore_clipboard, ours);
            Err(InsertError::NotTaken)
        }
    }
}

/// Leave `text` on the clipboard, without the marker, if it is still ours.
fn keep_text(clipboard: &dyn Clipboard, text: &str, had_marker: bool, ours: u64) {
    if had_marker
        && clipboard.still_ours(Selection::Clipboard, ours)
        && let Err(e) = clipboard.publish(Selection::Clipboard, text_content(text, false), None)
    {
        log::warn!("cannot leave the text on the clipboard: {e}");
    }
}

/// Put the saved content of `selection` back. False when it was not read.
fn restore(clipboard: &dyn Clipboard, selection: Selection, saved: &Snapshot) -> bool {
    match saved.get(selection) {
        Some(content) if content.is_empty() => {
            clipboard.clear(selection);
            true
        }
        Some(content) => match clipboard.publish(selection, content.clone(), None) {
            Ok(_) => true,
            Err(e) => {
                log::warn!("cannot restore the {selection:?} selection: {e}");
                false
            }
        },
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Backend choice
// ---------------------------------------------------------------------------

/// The key injection backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Injection {
    XTest,
    VirtualKeyboard,
    Portal,
    Uinput,
}

/// What the system offers for key injection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InjectionCaps {
    /// The compositor offers `zwp_virtual_keyboard_manager_v1`.
    pub virtual_keyboard: bool,
    /// The remote desktop portal exists.
    pub portal: bool,
    /// The portal session runs, or a saved token can start it without a dialog.
    pub portal_ready: bool,
    /// Sayso can write to `/dev/uinput`.
    pub uinput: bool,
}

/// The backend for key presses. On Wayland: the virtual keyboard, then a
/// portal that needs no dialog, then uinput, then the portal with its dialog.
pub fn choose_injection(kind: SessionKind, caps: InjectionCaps) -> Option<Injection> {
    match kind {
        SessionKind::X11 => Some(Injection::XTest),
        SessionKind::None => None,
        SessionKind::Wayland if caps.virtual_keyboard => Some(Injection::VirtualKeyboard),
        SessionKind::Wayland if caps.portal && caps.portal_ready => Some(Injection::Portal),
        SessionKind::Wayland if caps.uinput => Some(Injection::Uinput),
        SessionKind::Wayland if caps.portal => Some(Injection::Portal),
        SessionKind::Wayland => None,
    }
}

/// The permission state that `caps` give.
pub fn injection_state_for(kind: SessionKind, caps: InjectionCaps) -> PermissionState {
    match choose_injection(kind, caps) {
        None => PermissionState::Denied,
        Some(Injection::Portal) if !caps.portal_ready => PermissionState::NotDetermined,
        Some(_) => PermissionState::Granted,
    }
}

/// The clipboard backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardKind {
    X11,
    ExtDataControl,
    WlrDataControl,
}

impl ClipboardKind {
    /// Read `SAYSO_CLIPBOARD` (`x11`, `ext`, `wlr`), which overrides the
    /// choice for tests.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "x11" => Some(Self::X11),
            "ext" => Some(Self::ExtDataControl),
            "wlr" => Some(Self::WlrDataControl),
            _ => None,
        }
    }
}

/// The clipboard backend. On Wayland: data control when the compositor has
/// it, else the X11 clipboard of XWayland (GNOME), which the compositor
/// bridges to Wayland apps.
pub fn choose_clipboard(kind: SessionKind, ext: bool, wlr: bool, has_x11: bool) -> Option<ClipboardKind> {
    match kind {
        SessionKind::Wayland if ext => Some(ClipboardKind::ExtDataControl),
        SessionKind::Wayland if wlr => Some(ClipboardKind::WlrDataControl),
        SessionKind::Wayland | SessionKind::X11 | SessionKind::None if has_x11 => Some(ClipboardKind::X11),
        _ => None,
    }
}

fn caps(session: &Session) -> InjectionCaps {
    let wayland = session.is_wayland();
    let portal = wayland && portal::available();
    InjectionCaps {
        virtual_keyboard: session.has_global("zwp_virtual_keyboard_manager_v1"),
        portal,
        portal_ready: portal && portal::ready(),
        uinput: wayland && uinput::writable(),
    }
}

// ---------------------------------------------------------------------------
// The inserter
// ---------------------------------------------------------------------------

pub struct LinuxInserter {
    session: &'static Session,
    clipboard: OnceLock<Option<Box<dyn Clipboard>>>,
    xtest: Mutex<Option<xtest::XTest>>,
}

impl LinuxInserter {
    pub fn new(session: &'static Session) -> Self {
        if session.is_wayland() {
            // The portal check is a D-Bus call; do it before the UI asks.
            let _ = std::thread::Builder::new().name("sayso-portal-probe".into()).spawn(portal::available);
        }
        Self { session, clipboard: OnceLock::new(), xtest: Mutex::new(None) }
    }

    /// The clipboard backend, started on first use. None when the session
    /// has no clipboard that Sayso can reach.
    fn clipboard(&self) -> Option<&dyn Clipboard> {
        self.clipboard
            .get_or_init(|| {
                let s = self.session;
                let forced = std::env::var("SAYSO_CLIPBOARD").ok().and_then(|v| ClipboardKind::parse(&v));
                let kind = forced.or_else(|| {
                    choose_clipboard(
                        s.kind,
                        s.has_global("ext_data_control_manager_v1"),
                        s.has_global("zwlr_data_control_manager_v1"),
                        s.x11_display.is_some(),
                    )
                })?;
                log::info!("clipboard backend: {kind:?}");
                let started: Result<Box<dyn Clipboard>, String> = match kind {
                    ClipboardKind::X11 => clipboard_x11::X11Clipboard::start(s).map(|c| Box::new(c) as _),
                    ClipboardKind::ExtDataControl => clipboard_wayland::start_ext(s).map(|c| Box::new(c) as _),
                    ClipboardKind::WlrDataControl => clipboard_wayland::start_wlr(s).map(|c| Box::new(c) as _),
                };
                started.inspect_err(|e| log::warn!("cannot start the {kind:?} clipboard: {e}")).ok()
            })
            .as_deref()
    }

    /// Run `f` with the key injection backend of the session.
    fn with_injector<R>(&self, f: impl FnOnce(&dyn KeyInjector) -> Result<R, InsertError>) -> Result<R, InsertError> {
        let choice = choose_injection(self.session.kind, caps(self.session));
        log::debug!("key injection: {choice:?}");
        match choice {
            None => Err(InsertError::NotTrusted),
            Some(Injection::XTest) => {
                let mut xtest = self.xtest.lock();
                if xtest.is_none() {
                    *xtest = Some(xtest::XTest::connect(self.session).map_err(|e| {
                        log::warn!("cannot use XTest: {e}");
                        InsertError::NotTrusted
                    })?);
                }
                let Some(x) = xtest.as_ref() else { return Err(InsertError::NotTrusted) };
                let result = f(x);
                if matches!(result, Err(InsertError::Failed(_))) {
                    // A broken connection: connect again next time.
                    *xtest = None;
                }
                result
            }
            Some(Injection::VirtualKeyboard) => f(&virtual_keyboard::VirtualKeyboard::new(self.session)),
            Some(Injection::Portal) => f(portal::Portal::get()),
            Some(Injection::Uinput) => f(&uinput::Uinput),
        }
    }
}

impl TextInserter for LinuxInserter {
    fn insert(&self, text: &str, method: InsertMethod) -> Result<InsertResult, InsertError> {
        match method {
            InsertMethod::Type => self.with_injector(|keys| keys.type_text(text)).map(|()| InsertResult::Typed),
            InsertMethod::Paste { restore_clipboard, restore_delay } => {
                let Some(clipboard) = self.clipboard() else {
                    log::warn!("no clipboard backend; Sayso types the text");
                    return self.with_injector(|keys| keys.type_text(text)).map(|()| InsertResult::Typed);
                };
                let key = PasteKey::from_env();
                self.with_injector(|keys| {
                    paste_with_receipt(clipboard, text, restore_clipboard, restore_delay, SETTLE_BEFORE_KEY, || {
                        keys.paste_key(key)
                    })
                })
            }
        }
    }

    fn copy_to_clipboard(&self, text: &str) {
        match self.clipboard() {
            Some(clipboard) => {
                if let Err(e) = clipboard.publish(Selection::Clipboard, text_content(text, false), None) {
                    log::warn!("cannot copy to the clipboard: {e}");
                }
            }
            None => log::warn!("no clipboard backend; the text is not copied"),
        }
    }
}

/// Whether Sayso can send key presses to other apps. The Linux form of
/// Accessibility.
pub fn injection_state(session: &Session) -> PermissionState {
    injection_state_for(session.kind, caps(session))
}

/// Ask for the right to send key presses (the remote desktop portal dialog),
/// where the desktop has such a dialog. Else open the setup guide. Returns
/// at once; the dialog runs in the background.
pub fn request_injection(session: &Session) {
    if session.is_wayland() && portal::available() {
        portal::Portal::get().request();
    } else if injection_state(session) != PermissionState::Granted {
        crate::permissions::open_uri(session, crate::permissions::GUIDE_URL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// An in-memory clipboard. `read` plays an app that pastes.
    #[derive(Default)]
    struct FakeClipboard {
        inner: Mutex<FakeState>,
    }

    #[derive(Default)]
    struct FakeState {
        content: HashMap<Selection, Content>,
        owner: HashMap<Selection, u64>,
        receipts: HashMap<Selection, SharedReceipts>,
        next: u64,
        unreadable: bool,
    }

    impl FakeClipboard {
        fn with_text(text: &str) -> Self {
            let fake = Self::default();
            let mut s = fake.inner.lock();
            for sel in Selection::BOTH {
                s.content.insert(sel, vec![("UTF8_STRING".into(), text.as_bytes().to_vec())]);
            }
            drop(s);
            fake
        }

        /// What an app gets when it pastes `selection`, as the server would serve it.
        fn read(&self, selection: Selection) -> Option<String> {
            let s = self.inner.lock();
            if s.owner.contains_key(&selection)
                && let Some(r) = s.receipts.get(&selection)
            {
                r.lock().record_read(Instant::now());
            }
            self.peek_locked(&s, selection)
        }

        fn peek(&self, selection: Selection) -> Option<String> {
            let s = self.inner.lock();
            self.peek_locked(&s, selection)
        }

        fn peek_locked(&self, s: &FakeState, selection: Selection) -> Option<String> {
            let content = s.content.get(&selection)?;
            content.iter().find(|(f, _)| f == "UTF8_STRING").map(|(_, b)| String::from_utf8(b.clone()).unwrap())
        }

        fn has_marker(&self) -> bool {
            self.inner.lock().content[&Selection::Clipboard].iter().any(|(f, _)| f == PASSWORD_HINT)
        }

        /// Another app copies `text`.
        fn copy_elsewhere(&self, text: &str) {
            let mut s = self.inner.lock();
            s.owner.remove(&Selection::Clipboard);
            if let Some(r) = s.receipts.remove(&Selection::Clipboard) {
                r.lock().mark_replaced();
            }
            s.content.insert(Selection::Clipboard, vec![("UTF8_STRING".into(), text.as_bytes().to_vec())]);
        }
    }

    impl Clipboard for FakeClipboard {
        fn snapshot(&self) -> Snapshot {
            let s = self.inner.lock();
            if s.unreadable {
                return Snapshot::default();
            }
            Snapshot {
                clipboard: Some(s.content.get(&Selection::Clipboard).cloned().unwrap_or_default()),
                primary: Some(s.content.get(&Selection::Primary).cloned().unwrap_or_default()),
            }
        }

        fn publish(&self, sel: Selection, content: Content, receipts: Option<SharedReceipts>) -> Result<u64, String> {
            let mut s = self.inner.lock();
            s.next += 1;
            let generation = s.next;
            s.content.insert(sel, content);
            s.owner.insert(sel, generation);
            match receipts {
                Some(r) => s.receipts.insert(sel, r),
                None => s.receipts.remove(&sel),
            };
            Ok(generation)
        }

        fn still_ours(&self, sel: Selection, generation: u64) -> bool {
            self.inner.lock().owner.get(&sel) == Some(&generation)
        }

        fn clear(&self, sel: Selection) {
            let mut s = self.inner.lock();
            s.content.remove(&sel);
            s.owner.remove(&sel);
        }
    }

    const QUIET: Duration = Duration::from_millis(30);
    const NO_SETTLE: Duration = Duration::ZERO;

    #[test]
    fn a_paste_that_an_app_reads_succeeds_and_restores_both_selections() {
        let board = FakeClipboard::with_text("what the user copied");
        let mut read = None;
        let result = paste_with_receipt(&board, "dictated text", true, QUIET, NO_SETTLE, || {
            read = board.read(Selection::Primary);
            Ok(())
        });
        assert_eq!(read.as_deref(), Some("dictated text"));
        assert_eq!(result, Ok(InsertResult::Pasted { clipboard_restored: true }));
        assert_eq!(board.peek(Selection::Clipboard).as_deref(), Some("what the user copied"));
        assert_eq!(board.peek(Selection::Primary).as_deref(), Some("what the user copied"));
    }

    #[test]
    fn a_paste_that_no_app_reads_fails_and_leaves_the_text_without_the_marker() {
        let board = FakeClipboard::with_text("what the user copied");
        let started = Instant::now();
        let result = paste_with_receipt(&board, "dictated text", true, QUIET, NO_SETTLE, || Ok(()));
        assert_eq!(result, Err(InsertError::NotTaken));
        assert!(started.elapsed() >= crate::paste_receipt::NO_READ_TIMEOUT);
        assert_eq!(board.peek(Selection::Clipboard).as_deref(), Some("dictated text"));
        assert!(!board.has_marker(), "clipboard managers may keep the text now");
    }

    #[test]
    fn an_empty_clipboard_is_empty_again_after_the_paste() {
        let board = FakeClipboard::default();
        let result = paste_with_receipt(&board, "dictated text", true, QUIET, NO_SETTLE, || {
            board.read(Selection::Clipboard);
            Ok(())
        });
        assert_eq!(result, Ok(InsertResult::Pasted { clipboard_restored: true }));
        assert_eq!(board.peek(Selection::Clipboard), None);
    }

    #[test]
    fn an_unreadable_clipboard_is_not_restored() {
        let board = FakeClipboard::with_text("what the user copied");
        board.inner.lock().unreadable = true;
        let result = paste_with_receipt(&board, "dictated text", true, QUIET, NO_SETTLE, || {
            board.read(Selection::Clipboard);
            Ok(())
        });
        assert_eq!(result, Ok(InsertResult::Pasted { clipboard_restored: false }));
        assert_eq!(board.peek(Selection::Clipboard).as_deref(), Some("dictated text"));
    }

    #[test]
    fn without_restore_the_text_stays_and_has_no_marker() {
        let board = FakeClipboard::with_text("what the user copied");
        let result = paste_with_receipt(&board, "dictated text", false, QUIET, NO_SETTLE, || {
            board.read(Selection::Clipboard);
            Ok(())
        });
        assert_eq!(result, Ok(InsertResult::Pasted { clipboard_restored: false }));
        assert_eq!(board.peek(Selection::Clipboard).as_deref(), Some("dictated text"));
        assert!(!board.has_marker());
    }

    #[test]
    fn a_newer_copy_during_the_paste_is_not_overwritten() {
        let board = FakeClipboard::with_text("what the user copied");
        let result = paste_with_receipt(&board, "dictated text", true, QUIET, NO_SETTLE, || {
            board.read(Selection::Clipboard);
            board.copy_elsewhere("copied during the paste");
            Ok(())
        });
        assert_eq!(result, Ok(InsertResult::Pasted { clipboard_restored: false }));
        assert_eq!(board.peek(Selection::Clipboard).as_deref(), Some("copied during the paste"));
    }

    #[test]
    fn a_paste_key_that_cannot_be_sent_leaves_the_text_on_the_clipboard() {
        let board = FakeClipboard::with_text("what the user copied");
        let result =
            paste_with_receipt(&board, "dictated text", true, QUIET, NO_SETTLE, || Err(InsertError::NotTrusted));
        assert_eq!(result, Err(InsertError::NotTrusted));
        assert_eq!(board.peek(Selection::Clipboard).as_deref(), Some("dictated text"));
        assert!(!board.has_marker());
    }

    #[test]
    fn the_marker_is_offered_only_for_a_restored_paste_and_its_reads_do_not_count() {
        assert!(text_content("x", true).iter().any(|(f, v)| f == PASSWORD_HINT && v == b"secret"));
        assert!(!text_content("x", false).iter().any(|(f, _)| f == PASSWORD_HINT));
        assert!(!read_counts(PASSWORD_HINT));
        assert!(!read_counts("TARGETS"));
        assert!(read_counts("UTF8_STRING"));
        assert!(read_counts("image/png"));
    }

    fn caps(virtual_keyboard: bool, portal: bool, portal_ready: bool, uinput: bool) -> InjectionCaps {
        InjectionCaps { virtual_keyboard, portal, portal_ready, uinput }
    }

    #[test]
    fn x11_always_uses_xtest_and_no_display_has_nothing() {
        assert_eq!(choose_injection(SessionKind::X11, InjectionCaps::default()), Some(Injection::XTest));
        assert_eq!(injection_state_for(SessionKind::X11, InjectionCaps::default()), PermissionState::Granted);
        assert_eq!(choose_injection(SessionKind::None, caps(true, true, true, true)), None);
        assert_eq!(injection_state_for(SessionKind::None, InjectionCaps::default()), PermissionState::Denied);
    }

    #[test]
    fn wayland_prefers_the_virtual_keyboard_then_a_ready_portal_then_uinput() {
        let w = SessionKind::Wayland;
        assert_eq!(choose_injection(w, caps(true, true, true, true)), Some(Injection::VirtualKeyboard));
        assert_eq!(choose_injection(w, caps(false, true, true, true)), Some(Injection::Portal));
        assert_eq!(choose_injection(w, caps(false, true, false, true)), Some(Injection::Uinput), "no surprise dialog");
        assert_eq!(choose_injection(w, caps(false, true, false, false)), Some(Injection::Portal));
        assert_eq!(choose_injection(w, caps(false, false, false, true)), Some(Injection::Uinput));
        assert_eq!(choose_injection(w, caps(false, false, false, false)), None);
    }

    #[test]
    fn a_portal_without_a_token_is_not_determined() {
        let w = SessionKind::Wayland;
        assert_eq!(injection_state_for(w, caps(false, true, false, false)), PermissionState::NotDetermined);
        assert_eq!(injection_state_for(w, caps(false, true, true, false)), PermissionState::Granted);
        assert_eq!(injection_state_for(w, caps(false, true, false, true)), PermissionState::Granted);
        assert_eq!(injection_state_for(w, caps(true, false, false, false)), PermissionState::Granted);
        assert_eq!(injection_state_for(w, caps(false, false, false, false)), PermissionState::Denied);
    }

    #[test]
    fn data_control_wins_on_wayland_and_xwayland_is_the_fallback() {
        let w = SessionKind::Wayland;
        assert_eq!(choose_clipboard(w, true, true, true), Some(ClipboardKind::ExtDataControl));
        assert_eq!(choose_clipboard(w, false, true, true), Some(ClipboardKind::WlrDataControl));
        assert_eq!(choose_clipboard(w, false, false, true), Some(ClipboardKind::X11), "GNOME");
        assert_eq!(choose_clipboard(w, false, false, false), None);
        assert_eq!(choose_clipboard(SessionKind::X11, false, false, true), Some(ClipboardKind::X11));
        assert_eq!(choose_clipboard(SessionKind::None, false, false, false), None);
    }
}
