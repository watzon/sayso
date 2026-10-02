//! Raw C declarations for the Carbon, CoreGraphics, and ApplicationServices
//! calls that no safe crate in our dependency set covers.
//!
//! Everything here is `unsafe` to call. The safe wrappers live in the module
//! that uses them. The constants come from the spikes (`spikes/hotkeys`).
#![allow(non_snake_case, non_upper_case_globals, non_camel_case_types, dead_code)]
#![allow(clippy::duplicated_attributes)]

use std::ffi::c_void;

pub type CFTypeRef = *const c_void;
pub type OSStatus = i32;
pub type AXError = i32;

#[link(name = "Carbon", kind = "framework")]
#[link(name = "CoreFoundation", kind = "framework")]
#[link(name = "CoreGraphics", kind = "framework")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {}

// ---- Carbon ----

pub const noErr: OSStatus = 0;

unsafe extern "C" {
    pub fn IsSecureEventInputEnabled() -> bool;
    /// Returns a retained CFArray of CFDictionary.
    pub fn CopySymbolicHotKeys(out: *mut CFTypeRef) -> OSStatus;
}

// ---- CoreFoundation ----

unsafe extern "C" {
    pub static kCFRunLoopCommonModes: CFTypeRef;
    pub fn CFRunLoopRun();
    pub fn CFRunLoopStop(rl: CFTypeRef);
    pub fn CFRunLoopGetCurrent() -> CFTypeRef;
    pub fn CFRunLoopAddSource(rl: CFTypeRef, src: CFTypeRef, mode: CFTypeRef);
    pub fn CFMachPortCreateRunLoopSource(alloc: CFTypeRef, port: CFTypeRef, order: isize) -> CFTypeRef;
    pub fn CFRelease(cf: CFTypeRef);
}

// ---- CoreGraphics: event taps ----

pub const kCGSessionEventTap: u32 = 1;
pub const kCGHeadInsertEventTap: u32 = 0;
pub const kCGEventTapOptionListenOnly: u32 = 1;

pub const kCGEventKeyDown: u32 = 10;
pub const kCGEventKeyUp: u32 = 11;
pub const kCGEventFlagsChanged: u32 = 12;
pub const kCGEventTapDisabledByTimeout: u32 = 0xFFFF_FFFE;
pub const kCGEventTapDisabledByUserInput: u32 = 0xFFFF_FFFF;

pub const kCGKeyboardEventAutorepeat: u32 = 8;
pub const kCGKeyboardEventKeycode: u32 = 9;

/// `CGEventFlags` bits.
pub const FLAG_SHIFT: u64 = 0x2_0000;
pub const FLAG_CONTROL: u64 = 0x4_0000;
pub const FLAG_ALT: u64 = 0x8_0000;
pub const FLAG_COMMAND: u64 = 0x10_0000;
pub const FLAG_SECONDARY_FN: u64 = 0x80_0000;

pub type CGEventTapCallBack =
    extern "C" fn(proxy: *mut c_void, etype: u32, event: *mut c_void, user: *mut c_void) -> *mut c_void;

unsafe extern "C" {
    pub fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: CGEventTapCallBack,
        user: *mut c_void,
    ) -> CFTypeRef;
    pub fn CGEventTapEnable(tap: CFTypeRef, enable: bool);
    pub fn CGEventGetIntegerValueField(event: *mut c_void, field: u32) -> i64;
    pub fn CGEventGetFlags(event: *mut c_void) -> u64;
    pub fn CGPreflightListenEventAccess() -> bool;
    pub fn CGRequestListenEventAccess() -> bool;
}

// ---- CoreGraphics: event posting ----

pub const kCGHIDEventTap: u32 = 0;
/// `kCGEventSourceStatePrivate`: the event does not inherit held modifier keys.
pub const kCGEventSourceStatePrivate: i32 = -1;

unsafe extern "C" {
    pub fn CGEventSourceCreate(state: i32) -> CFTypeRef;
    pub fn CGEventCreateKeyboardEvent(source: CFTypeRef, keycode: u16, down: bool) -> CFTypeRef;
    pub fn CGEventSetFlags(event: CFTypeRef, flags: u64);
    pub fn CGEventKeyboardSetUnicodeString(event: CFTypeRef, len: usize, chars: *const u16);
    pub fn CGEventPost(tap: u32, event: CFTypeRef);
}

// ---- CoreGraphics: session and displays ----

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

pub const kCGWindowListOptionOnScreenOnly: u32 = 1;
pub const kCGWindowListExcludeDesktopElements: u32 = 1 << 4;
pub const kCGNullWindowID: u32 = 0;

unsafe extern "C" {
    pub fn CGSessionCopyCurrentDictionary() -> CFTypeRef;
    pub fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFTypeRef;
    pub fn CGRectMakeWithDictionaryRepresentation(dict: CFTypeRef, rect: *mut CGRect) -> bool;
    pub fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    pub fn CGDisplayBounds(display: u32) -> CGRect;
}

// ---- Accessibility ----

unsafe extern "C" {
    pub fn AXIsProcessTrusted() -> bool;
    pub fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> bool;
    pub fn AXUIElementCreateSystemWide() -> CFTypeRef;
    pub fn AXUIElementCopyAttributeValue(element: CFTypeRef, attribute: CFTypeRef, value: *mut CFTypeRef) -> AXError;
}

pub const kAXErrorSuccess: AXError = 0;
pub const kAXErrorNoValue: AXError = -25212;
