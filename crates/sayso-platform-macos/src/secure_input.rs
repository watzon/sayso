//! Secure Event Input detection (password fields, Terminal "Secure Keyboard
//! Entry", password managers).
//!
//! While it is on, macOS hides key events from event taps and synthetic
//! typing can fail. The session dictionary names the process that holds it.
//! From the spike: that PID is the "responsible app" (for example the host of
//! a terminal), not always the exact process.

use crate::context::app_name_for_pid;
use crate::ffi::{CGSessionCopyCurrentDictionary, IsSecureEventInputEnabled};
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;

/// True while some process holds Secure Event Input.
pub fn is_enabled() -> bool {
    // SAFETY: plain query, no arguments.
    unsafe { IsSecureEventInputEnabled() }
}

/// PID that holds Secure Event Input, from the session dictionary.
fn holder_pid() -> Option<i32> {
    // SAFETY: returns a retained dictionary or null.
    let raw = unsafe { CGSessionCopyCurrentDictionary() };
    if raw.is_null() {
        return None;
    }
    // SAFETY: "Copy" rule, so we own one reference.
    let dict: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_create_rule(raw.cast()) };
    dict.find(CFString::new("kCGSSessionSecureInputPID"))
        .and_then(|v| v.downcast::<CFNumber>())
        .and_then(|n| n.to_i32())
}

/// Name of the app holding Secure Event Input, or `None` when it is off.
pub fn holder() -> Option<String> {
    if !is_enabled() {
        return None;
    }
    let name = holder_pid().and_then(|pid| app_name_for_pid(pid).or_else(|| Some(format!("process {pid}"))));
    Some(name.unwrap_or_else(|| "an unknown app".to_string()))
}
