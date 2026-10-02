//! [`TextInserter`]: put text into the focused field of the frontmost app.
//!
//! Two methods:
//! - **Paste**: save the clipboard, publish our text as a promise, post
//!   Cmd+V, and wait until an app reads the text. Then restore the clipboard
//!   if nobody changed it meanwhile. With no read, the paste failed, and the
//!   text stays on the clipboard.
//! - **Type**: post synthetic key events that carry Unicode strings.
//!
//! Both need Accessibility (to post events). Secure Event Input blocks them,
//! and an app with no focused element and no focused window has nothing to
//! type into.

use crate::ffi::*;
use crate::paste_receipt::{Receipts, Verdict};
use crate::secure_input;
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeString, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSData, NSObject, NSObjectProtocol, NSString};
use parking_lot::Mutex;
use std::sync::Arc;
use sayso_platform::{InsertError, InsertMethod, InsertResult, TextInserter};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// How often the paste looks for a read of the text.
const RECEIPT_POLL: Duration = Duration::from_millis(10);
/// Virtual key code of `V`.
const KC_V: u16 = 9;
/// Largest number of UTF-16 units one synthetic key event can carry.
pub const MAX_UNITS_PER_EVENT: usize = 20;
const CHUNK_DELAY: Duration = Duration::from_millis(5);
const KEY_GAP: Duration = Duration::from_millis(8);
/// Marker that tells clipboard managers to skip an entry (nspasteboard.org).
const TRANSIENT_TYPE: &str = "org.nspasteboard.TransientType";

pub struct MacInserter;

/// Split text into UTF-16 chunks of at most `max_units` units without cutting
/// a surrogate pair. `max_units` must be at least 2, so every char fits.
pub fn utf16_chunks(text: &str, max_units: usize) -> Vec<Vec<u16>> {
    assert!(max_units >= 2, "a chunk must hold one surrogate pair");
    let mut chunks = Vec::new();
    let mut current: Vec<u16> = Vec::new();
    let mut buf = [0u16; 2];
    for ch in text.chars() {
        let units = ch.encode_utf16(&mut buf);
        if current.len() + units.len() > max_units {
            chunks.push(std::mem::take(&mut current));
        }
        current.extend_from_slice(units);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

// ---------------------------------------------------------------------------
// Preconditions
// ---------------------------------------------------------------------------

/// Check Accessibility, Secure Input, and focus, in that order.
fn preflight() -> Result<(), InsertError> {
    // SAFETY: plain query.
    if !unsafe { AXIsProcessTrusted() } {
        return Err(InsertError::NotTrusted);
    }
    if secure_input::is_enabled() {
        return Err(InsertError::SecureInput);
    }
    if !can_receive_text() {
        return Err(InsertError::NoFocusedField);
    }
    Ok(())
}

/// Read one Accessibility attribute and return only the status.
///
/// # Safety
/// `element` must be a valid `AXUIElementRef`.
unsafe fn attribute_status(element: CFTypeRef, name: &str) -> AXError {
    let attribute = CFString::new(name);
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: the caller gives a valid element. `value` is released when the call succeeded.
    unsafe {
        let status = AXUIElementCopyAttributeValue(element, attribute.as_concrete_TypeRef().cast(), &mut value);
        if status == kAXErrorSuccess && !value.is_null() {
            CFRelease(value);
        }
        status
    }
}

/// Whether the two Accessibility answers leave a place for text.
///
/// "No value" for the focused element does not prove that nothing is
/// focused: Electron and other web-content apps give that answer with the
/// cursor in a text field. Such an app still reports its focused window. So
/// only "no focused element" together with "no focused window" means that
/// there is nowhere to type. Every other error is unknown, and unknown must
/// not block the insertion.
fn focus_allows_text(focused_element: AXError, focused_window: impl FnOnce() -> AXError) -> bool {
    focused_element != kAXErrorNoValue || focused_window() != kAXErrorNoValue
}

/// Whether the frontmost app can take text now.
fn can_receive_text() -> bool {
    // SAFETY: each element is created and released here.
    unsafe {
        let system = AXUIElementCreateSystemWide();
        if system.is_null() {
            return true;
        }
        let element = attribute_status(system, "AXFocusedUIElement");
        CFRelease(system);
        if element != kAXErrorSuccess && element != kAXErrorNoValue {
            log::debug!("AXFocusedUIElement failed with {element}; assuming a field is focused");
        }
        focus_allows_text(element, || {
            let Some(app) = objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication() else {
                return kAXErrorNoValue;
            };
            let app = AXUIElementCreateApplication(app.processIdentifier());
            if app.is_null() {
                return kAXErrorSuccess;
            }
            let window = attribute_status(app, "AXFocusedWindow");
            CFRelease(app);
            log::debug!("AXFocusedUIElement has no value; AXFocusedWindow of the frontmost app gives {window}");
            window
        })
    }
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

/// Every item on the clipboard with every type and its bytes.
type Snapshot = Vec<Vec<(String, Vec<u8>)>>;

fn snapshot(pasteboard: &NSPasteboard) -> Snapshot {
    let Some(items) = pasteboard.pasteboardItems() else { return Vec::new() };
    items
        .iter()
        .map(|item| {
            item.types()
                .iter()
                .filter_map(|ty| item.dataForType(&ty).map(|data| (ty.to_string(), data.to_vec())))
                .collect()
        })
        .collect()
}

fn write_all(pasteboard: &NSPasteboard, items: Vec<Retained<NSPasteboardItem>>) -> bool {
    pasteboard.clearContents();
    if items.is_empty() {
        return true;
    }
    let writers: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
        items.into_iter().map(ProtocolObject::from_retained).collect();
    pasteboard.writeObjects(&NSArray::from_retained_slice(&writers))
}

fn restore(pasteboard: &NSPasteboard, saved: &Snapshot) -> bool {
    let items = saved
        .iter()
        .map(|types| {
            let item = NSPasteboardItem::new();
            for (ty, bytes) in types {
                item.setData_forType(&NSData::with_bytes(bytes), &NSString::from_str(ty));
            }
            item
        })
        .collect();
    write_all(pasteboard, items)
}

/// Replace the clipboard with `text`. Marked transient so clipboard
/// managers skip it. Returns the new change count.
fn write_text(pasteboard: &NSPasteboard, text: &str, transient: bool) -> isize {
    let item = NSPasteboardItem::new();
    // SAFETY: `NSPasteboardTypeString` is a constant provided by AppKit.
    item.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
    if transient {
        item.setData_forType(&NSData::new(), &NSString::from_str(TRANSIENT_TYPE));
    }
    write_all(pasteboard, vec![item]);
    pasteboard.changeCount()
}

// ---------------------------------------------------------------------------
// Paste with a read receipt
// ---------------------------------------------------------------------------

struct OwnerState {
    text: String,
    receipts: Arc<Mutex<Receipts>>,
}

define_class!(
    // The owner of the promised text. The pasteboard asks it for the text when
    // an app reads the clipboard. macOS sends both messages on the main thread,
    // while the main run loop runs.
    //
    // SAFETY: `NSObject` has no subclassing requirements, and the ivars are
    // plain Rust values behind a mutex.
    #[unsafe(super(NSObject))]
    #[name = "SaysoPasteOwner"]
    #[ivars = OwnerState]
    struct PasteOwner;

    impl PasteOwner {
        #[unsafe(method(pasteboard:provideDataForType:))]
        fn provide_data(&self, pasteboard: &NSPasteboard, data_type: &NSString) {
            // SAFETY: `NSPasteboardTypeString` is a constant provided by AppKit.
            if data_type.isEqualToString(unsafe { NSPasteboardTypeString }) {
                self.ivars().receipts.lock().record_read(Instant::now());
                pasteboard.setString_forType(&NSString::from_str(&self.ivars().text), data_type);
            }
        }

        #[unsafe(method(pasteboardChangedOwner:))]
        fn changed_owner(&self, _pasteboard: &NSPasteboard) {
            self.ivars().receipts.lock().mark_replaced();
        }
    }

    unsafe impl NSObjectProtocol for PasteOwner {}
);

impl PasteOwner {
    fn new(text: &str, receipts: Arc<Mutex<Receipts>>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(OwnerState { text: text.to_string(), receipts });
        // SAFETY: `init` of `NSObject` takes no arguments and cannot fail.
        unsafe { msg_send![super(this), init] }
    }
}

/// Put `text` on the pasteboard as a promise that `owner` fulfils when an app
/// reads it. Marked transient so clipboard managers skip it: fewer readers
/// means that a read is more surely the target app. Returns the change count.
fn publish(pasteboard: &NSPasteboard, owner: &PasteOwner, transient: bool) -> isize {
    // SAFETY: `NSPasteboardTypeString` is a constant provided by AppKit.
    let mut types = vec![unsafe { NSPasteboardTypeString }.to_owned()];
    if transient {
        types.push(NSString::from_str(TRANSIENT_TYPE));
    }
    let owner: &AnyObject = owner;
    // SAFETY: the owner answers `pasteboard:provideDataForType:`, and the
    // caller keeps it alive until the pasteboard has other content.
    unsafe { pasteboard.declareTypes_owner(&NSArray::from_retained_slice(&types), Some(owner)) };
    if transient {
        pasteboard.setData_forType(Some(&NSData::new()), &NSString::from_str(TRANSIENT_TYPE));
    }
    pasteboard.changeCount()
}

/// Paste `text` through `pasteboard` and wait for the receipt.
///
/// `send_paste` posts the paste key. `quiet` is how long the reads must have
/// stopped before the old clipboard comes back.
///
/// With no read, the app did not take the text. The old clipboard then stays
/// away and the text stays on the clipboard, for two reasons: the user can
/// paste it by hand, and an app that reads very late cannot get stale content.
fn paste_with_receipt(
    pasteboard: &NSPasteboard,
    text: &str,
    restore_clipboard: bool,
    quiet: Duration,
    send_paste: impl FnOnce() -> Result<(), InsertError>,
) -> Result<InsertResult, InsertError> {
    let saved = restore_clipboard.then(|| snapshot(pasteboard));
    let receipts = Arc::new(Mutex::new(Receipts::default()));
    let owner = PasteOwner::new(text, receipts.clone());
    let ours = publish(pasteboard, &owner, restore_clipboard);
    // Before the key: a fast app can read while the key is still down.
    receipts.lock().mark_sent(Instant::now());
    if let Err(e) = send_paste() {
        write_text(pasteboard, text, false);
        return Err(e);
    }
    let verdict = loop {
        match receipts.lock().verdict(Instant::now(), quiet) {
            Verdict::Wait => {}
            done => break done,
        }
        sleep(RECEIPT_POLL);
    };
    // Still our promise on the clipboard? If not, the user or another app
    // copied something newer, and that content must stay.
    let still_ours = pasteboard.changeCount() == ours;
    match verdict {
        Verdict::Taken => {
            let clipboard_restored = match &saved {
                Some(saved) => still_ours && restore(pasteboard, saved),
                None => {
                    if still_ours {
                        // Real text in place of the promise, which ends with `owner`.
                        write_text(pasteboard, text, false);
                    }
                    false
                }
            };
            Ok(InsertResult::Pasted { clipboard_restored })
        }
        Verdict::NotTaken | Verdict::Wait => {
            if still_ours {
                write_text(pasteboard, text, false);
            }
            Err(InsertError::NotTaken)
        }
    }
}

// ---------------------------------------------------------------------------
// Synthetic keys
// ---------------------------------------------------------------------------

/// Owns a `CGEventSource` that ignores keys the user holds, for example
/// the Right Option of a push-to-talk hotkey.
struct EventSource(CFTypeRef);

impl EventSource {
    fn new() -> Self {
        // SAFETY: returns a retained source or null (null is accepted by CGEventCreate*).
        Self(unsafe { CGEventSourceCreate(kCGEventSourceStatePrivate) })
    }

    /// Post one key event. `unicode` replaces the key's character when set.
    fn post(&self, keycode: u16, down: bool, flags: u64, unicode: Option<&[u16]>) -> Result<(), InsertError> {
        // SAFETY: the event is created, used, and released in this block.
        unsafe {
            let event = CGEventCreateKeyboardEvent(self.0, keycode, down);
            if event.is_null() {
                return Err(InsertError::Failed("cannot create a keyboard event".into()));
            }
            CGEventSetFlags(event, flags);
            if let Some(units) = unicode {
                CGEventKeyboardSetUnicodeString(event, units.len(), units.as_ptr());
            }
            CGEventPost(kCGHIDEventTap, event);
            CFRelease(event);
        }
        Ok(())
    }
}

impl Drop for EventSource {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we own one reference.
            unsafe { CFRelease(self.0) };
        }
    }
}

fn post_cmd_v() -> Result<(), InsertError> {
    let source = EventSource::new();
    source.post(KC_V, true, FLAG_COMMAND, None)?;
    sleep(KEY_GAP);
    source.post(KC_V, false, FLAG_COMMAND, None)
}

fn type_text(text: &str) -> Result<(), InsertError> {
    let source = EventSource::new();
    for chunk in utf16_chunks(text, MAX_UNITS_PER_EVENT) {
        source.post(0, true, 0, Some(&chunk))?;
        source.post(0, false, 0, Some(&chunk))?;
        sleep(CHUNK_DELAY);
    }
    Ok(())
}

impl TextInserter for MacInserter {
    fn insert(&self, text: &str, method: InsertMethod) -> Result<InsertResult, InsertError> {
        preflight()?;
        match method {
            InsertMethod::Type => {
                type_text(text)?;
                Ok(InsertResult::Typed)
            }
            InsertMethod::Paste { restore_clipboard, restore_delay } => {
                paste_with_receipt(&NSPasteboard::generalPasteboard(), text, restore_clipboard, restore_delay, post_cmd_v)
            }
        }
    }

    fn copy_to_clipboard(&self, text: &str) {
        write_text(&NSPasteboard::generalPasteboard(), text, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejoin(chunks: &[Vec<u16>]) -> String {
        String::from_utf16(&chunks.concat()).unwrap()
    }

    #[test]
    fn a_focused_element_allows_text_without_a_look_at_the_window() {
        assert!(focus_allows_text(kAXErrorSuccess, || panic!("the window is not needed")));
    }

    #[test]
    fn an_app_that_hides_its_focused_element_still_takes_text_in_its_window() {
        // What T3 Code (Electron) answers with the cursor in its text field.
        assert!(focus_allows_text(kAXErrorNoValue, || kAXErrorSuccess));
    }

    #[test]
    fn no_focused_element_and_no_focused_window_is_nowhere_to_type() {
        assert!(!focus_allows_text(kAXErrorNoValue, || kAXErrorNoValue));
    }

    #[test]
    fn an_accessibility_error_is_unknown_and_does_not_block() {
        const CANNOT_COMPLETE: AXError = -25204;
        assert!(focus_allows_text(CANNOT_COMPLETE, || kAXErrorNoValue));
        assert!(focus_allows_text(kAXErrorNoValue, || CANNOT_COMPLETE));
    }

    /// A private pasteboard, so the tests never touch the user's clipboard.
    struct TestBoard(Retained<NSPasteboard>);

    impl TestBoard {
        fn with_text(text: &str) -> Self {
            let board = Self(NSPasteboard::pasteboardWithUniqueName());
            write_text(&board.0, text, false);
            board
        }

        fn text(&self) -> Option<String> {
            // SAFETY: `NSPasteboardTypeString` is a constant provided by AppKit.
            self.0.stringForType(unsafe { NSPasteboardTypeString }).map(|s| s.to_string())
        }
    }

    impl Drop for TestBoard {
        fn drop(&mut self) {
            // SAFETY: the unique pasteboard belongs to this test.
            let _: () = unsafe { msg_send![&*self.0, releaseGlobally] };
        }
    }

    const QUIET: Duration = Duration::from_millis(30);

    #[test]
    fn a_paste_that_an_app_reads_succeeds_and_restores_the_clipboard() {
        let board = TestBoard::with_text("what the user copied");
        let mut read = None;
        let result = paste_with_receipt(&board.0, "dictated text", true, QUIET, || {
            // The target app reads the clipboard when it gets Cmd+V.
            read = board.text();
            Ok(())
        });
        assert_eq!(read.as_deref(), Some("dictated text"), "the promise gives the text to the reader");
        assert_eq!(result, Ok(InsertResult::Pasted { clipboard_restored: true }));
        assert_eq!(board.text().as_deref(), Some("what the user copied"));
    }

    #[test]
    fn a_paste_that_no_app_reads_fails_and_leaves_the_text_on_the_clipboard() {
        let board = TestBoard::with_text("what the user copied");
        let started = Instant::now();
        let result = paste_with_receipt(&board.0, "dictated text", true, QUIET, || Ok(()));
        assert_eq!(result, Err(InsertError::NotTaken));
        assert!(started.elapsed() >= crate::paste_receipt::NO_READ_TIMEOUT);
        assert_eq!(board.text().as_deref(), Some("dictated text"), "the user can paste it by hand");
    }

    #[test]
    fn without_restore_the_text_stays_on_the_clipboard_after_the_paste() {
        let board = TestBoard::with_text("what the user copied");
        let result = paste_with_receipt(&board.0, "dictated text", false, QUIET, || {
            board.text();
            Ok(())
        });
        assert_eq!(result, Ok(InsertResult::Pasted { clipboard_restored: false }));
        assert_eq!(board.text().as_deref(), Some("dictated text"));
    }

    #[test]
    fn a_newer_copy_during_the_paste_is_not_overwritten() {
        let board = TestBoard::with_text("what the user copied");
        let result = paste_with_receipt(&board.0, "dictated text", true, QUIET, || {
            board.text();
            write_text(&board.0, "copied during the paste", false);
            Ok(())
        });
        assert_eq!(result, Ok(InsertResult::Pasted { clipboard_restored: false }));
        assert_eq!(board.text().as_deref(), Some("copied during the paste"));
    }

    #[test]
    fn a_paste_key_that_cannot_be_sent_leaves_the_text_on_the_clipboard() {
        let board = TestBoard::with_text("what the user copied");
        let result = paste_with_receipt(&board.0, "dictated text", true, QUIET, || Err(InsertError::Failed("no key".into())));
        assert_eq!(result, Err(InsertError::Failed("no key".into())));
        assert_eq!(board.text().as_deref(), Some("dictated text"));
    }

    #[test]
    fn short_text_is_one_chunk() {
        let chunks = utf16_chunks("hello", 20);
        assert_eq!(chunks.len(), 1);
        assert_eq!(rejoin(&chunks), "hello");
    }

    #[test]
    fn empty_text_has_no_chunks() {
        assert!(utf16_chunks("", 20).is_empty());
    }

    #[test]
    fn long_ascii_splits_at_the_limit() {
        let text = "a".repeat(45);
        let chunks = utf16_chunks(&text, 20);
        assert_eq!(chunks.iter().map(Vec::len).collect::<Vec<_>>(), [20, 20, 5]);
        assert_eq!(rejoin(&chunks), text);
    }

    #[test]
    fn emoji_pairs_are_never_split() {
        // 19 units, then a 2-unit emoji: it must start a new chunk.
        let text = format!("{}😀tail", "a".repeat(19));
        let chunks = utf16_chunks(&text, 20);
        assert_eq!(chunks[0].len(), 19);
        assert_eq!(chunks[1][0], 0xD83D, "the second chunk starts with the high surrogate");
        assert_eq!(rejoin(&chunks), text);
    }

    #[test]
    fn every_chunk_is_valid_utf16_and_within_the_limit() {
        let text = "héllo 👩‍👩‍👧 wörld 😀😀😀😀😀😀😀😀😀😀😀 日本語のテキスト 🎉";
        for max in [2, 3, 7, 20] {
            let chunks = utf16_chunks(text, max);
            for chunk in &chunks {
                assert!(chunk.len() <= max);
                assert!(String::from_utf16(chunk).is_ok(), "chunk split a surrogate pair at max {max}");
            }
            assert_eq!(rejoin(&chunks), text);
        }
    }

    #[test]
    fn an_all_emoji_text_fills_chunks_exactly() {
        let chunks = utf16_chunks(&"😀".repeat(25), 20);
        assert_eq!(chunks.iter().map(Vec::len).collect::<Vec<_>>(), [20, 20, 10]);
    }

    #[test]
    #[should_panic(expected = "surrogate pair")]
    fn a_limit_below_two_is_rejected() {
        utf16_chunks("a", 1);
    }
}
