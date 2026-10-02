//! [`TextInserter`]: put text into the focused field of the frontmost app.
//!
//! Two methods:
//! - **Paste**: save the clipboard, write our text, post Cmd+V, wait, then
//!   restore the clipboard if nobody changed it meanwhile.
//! - **Type**: post synthetic key events that carry Unicode strings.
//!
//! Both need Accessibility (to post events). Secure Event Input blocks them,
//! and with no focused element there is nothing to type into.

use crate::ffi::*;
use crate::secure_input;
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeString, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSData, NSString};
use sayso_platform::{InsertError, InsertMethod, InsertResult, TextInserter};
use std::thread::sleep;
use std::time::Duration;

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
    if !has_focused_element() {
        return Err(InsertError::NoFocusedField);
    }
    Ok(())
}

/// Whether the system-wide Accessibility focus points at an element.
/// Only "no value" counts as missing. Other errors (for example an Electron
/// app that has not enabled Accessibility yet) do not prove that nothing is focused.
fn has_focused_element() -> bool {
    let attribute = CFString::new("AXFocusedUIElement");
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: the system-wide element is created and released here. `value`
    // is released when the call succeeded.
    unsafe {
        let system = AXUIElementCreateSystemWide();
        if system.is_null() {
            return true;
        }
        let status = AXUIElementCopyAttributeValue(system, attribute.as_concrete_TypeRef().cast(), &mut value);
        CFRelease(system);
        if status == kAXErrorSuccess && !value.is_null() {
            CFRelease(value);
        }
        if status != kAXErrorSuccess && status != kAXErrorNoValue {
            log::debug!("AXFocusedUIElement failed with {status}; assuming a field is focused");
        }
        status != kAXErrorNoValue
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
                let pasteboard = NSPasteboard::generalPasteboard();
                let saved = restore_clipboard.then(|| snapshot(&pasteboard));
                let ours = write_text(&pasteboard, text, restore_clipboard);
                post_cmd_v()?;
                let Some(saved) = saved else {
                    return Ok(InsertResult::Pasted { clipboard_restored: false });
                };
                sleep(restore_delay);
                // Restore only when nobody wrote to the clipboard since our write.
                let clipboard_restored = pasteboard.changeCount() == ours && restore(&pasteboard, &saved);
                Ok(InsertResult::Pasted { clipboard_restored })
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
