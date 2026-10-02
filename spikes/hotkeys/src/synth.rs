//! Mode `synth`: post synthetic key events with CGEventPost, to test the listeners without hands.
use crate::ffi::*;
use std::{ptr, thread::sleep, time::Duration};

fn key(code: u16, down: bool, flags: u64) {
    unsafe {
        let e = CGEventCreateKeyboardEvent(ptr::null(), code, down);
        CGEventSetFlags(e, flags);
        // SYNTH_TAP=0 posts at the HID level, 1 (default) at the session level.
        let loc = std::env::var("SYNTH_TAP").ok().and_then(|v| v.parse().ok()).unwrap_or(kCGSessionEventTap);
        CGEventPost(loc, e);
        CFRelease(e);
    }
    sleep(Duration::from_millis(30));
}

pub fn run(args: &[String]) {
    println!("CGPreflightPostEventAccess = {}", unsafe { CGPreflightPostEventAccess() });
    match args.first().map(String::as_str) {
        Some("opt-space") => { key(49, true, FLAG_ALT); sleep(Duration::from_millis(150)); key(49, false, FLAG_ALT); }
        Some("right-option") => {
            key(61, true, FLAG_ALT | 0x40); sleep(Duration::from_millis(200)); key(61, false, 0);
        }
        Some("fn") => { key(63, true, FLAG_SECONDARY_FN); sleep(Duration::from_millis(200)); key(63, false, 0); }
        Some("ctrl-shift-d") => {
            let m = FLAG_CONTROL | FLAG_SHIFT;
            key(59, true, FLAG_CONTROL); key(56, true, m); key(2, true, m); sleep(Duration::from_millis(100));
            key(2, false, m); key(56, false, FLAG_CONTROL); key(59, false, 0);
        }
        Some("esc-esc") => { for _ in 0..2 { key(53, true, 0); key(53, false, 0); sleep(Duration::from_millis(120)); } }
        // Arrow keys carry the SecondaryFn flag on real hardware; must NOT look like an Fn press.
        Some("arrow-fn") => { key(123, true, FLAG_SECONDARY_FN); key(123, false, FLAG_SECONDARY_FN); }
        // Hold Right Option (recording), then double-Esc, then release.
        Some("ptt-esc-esc") => {
            key(61, true, FLAG_ALT | 0x40);
            for _ in 0..2 { key(53, true, 0); key(53, false, 0); sleep(Duration::from_millis(120)); }
            key(61, false, 0);
        }
        // Two Escs 700 ms apart while recording: must NOT cancel.
        Some("ptt-esc-slow") => {
            key(61, true, FLAG_ALT | 0x40);
            for _ in 0..2 { key(53, true, 0); key(53, false, 0); sleep(Duration::from_millis(700)); }
            key(61, false, 0);
        }
        _ => println!("synth <opt-space|right-option|fn|ctrl-shift-d|esc-esc|arrow-fn|ptt-esc-esc|ptt-esc-slow>"),
    }
}
