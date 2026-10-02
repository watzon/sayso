//! Minimal raw FFI for Carbon hotkeys, CGEventTap, permission checks.
#![allow(non_snake_case, non_upper_case_globals, dead_code, clippy::duplicated_attributes)]
use std::ffi::c_void;

pub type OSStatus = i32;
pub type CFTypeRef = *const c_void;

#[link(name = "Carbon", kind = "framework")]
#[link(name = "CoreFoundation", kind = "framework")]
#[link(name = "CoreGraphics", kind = "framework")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {}

// ---- Carbon hotkeys ----
pub const noErr: OSStatus = 0;
pub const eventHotKeyExistsErr: OSStatus = -9878;
pub const eventHotKeyInvalidErr: OSStatus = -9879;

pub const kEventClassKeyboard: u32 = u32::from_be_bytes(*b"keyb");
pub const kEventHotKeyPressed: u32 = 5;
pub const kEventHotKeyReleased: u32 = 6;
pub const kEventParamDirectObject: u32 = u32::from_be_bytes(*b"----");
pub const typeEventHotKeyID: u32 = u32::from_be_bytes(*b"hkid");

pub const cmdKey: u32 = 1 << 8;
pub const shiftKey: u32 = 1 << 9;
pub const optionKey: u32 = 1 << 11;
pub const controlKey: u32 = 1 << 12;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EventHotKeyID {
    pub signature: u32,
    pub id: u32,
}
#[repr(C)]
pub struct EventTypeSpec {
    pub event_class: u32,
    pub event_kind: u32,
}
pub type EventHandlerProc =
    extern "C" fn(call_ref: *mut c_void, event: *mut c_void, user: *mut c_void) -> OSStatus;

unsafe extern "C" {
    pub fn GetApplicationEventTarget() -> *mut c_void;
    pub fn InstallEventHandler(
        target: *mut c_void,
        handler: EventHandlerProc,
        num_types: u32,
        types: *const EventTypeSpec,
        user: *mut c_void,
        out: *mut *mut c_void,
    ) -> OSStatus;
    pub fn RegisterEventHotKey(
        keycode: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: *mut c_void,
        options: u32,
        out: *mut *mut c_void,
    ) -> OSStatus;
    pub fn UnregisterEventHotKey(r: *mut c_void) -> OSStatus;
    pub fn GetEventKind(event: *mut c_void) -> u32;
    pub fn GetEventParameter(
        event: *mut c_void,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        buf_size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> OSStatus;
    pub fn IsSecureEventInputEnabled() -> bool;
    pub fn CopySymbolicHotKeys(out: *mut CFTypeRef) -> OSStatus;
}

// ---- CoreFoundation run loop ----
unsafe extern "C" {
    pub static kCFRunLoopCommonModes: CFTypeRef;
    pub fn CFRunLoopRun();
    pub fn CFRunLoopStop(rl: CFTypeRef);
    pub fn CFRunLoopGetCurrent() -> CFTypeRef;
    pub fn CFRunLoopAddSource(rl: CFTypeRef, src: CFTypeRef, mode: CFTypeRef);
    pub fn CFMachPortCreateRunLoopSource(alloc: CFTypeRef, port: CFTypeRef, order: isize) -> CFTypeRef;
    pub fn CFRunLoopRunInMode(mode: CFTypeRef, seconds: f64, return_after_source: bool) -> i32;
}

// ---- CoreGraphics event tap / posting / permissions ----
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
    pub fn CGEventCreateKeyboardEvent(source: CFTypeRef, keycode: u16, down: bool) -> CFTypeRef;
    pub fn CGEventSetFlags(event: CFTypeRef, flags: u64);
    pub fn CGEventPost(tap: u32, event: CFTypeRef);
    pub fn CFRelease(cf: CFTypeRef);
    pub fn CGPreflightListenEventAccess() -> bool;
    pub fn CGPreflightPostEventAccess() -> bool;
    pub fn AXIsProcessTrusted() -> bool;
}

unsafe extern "C" {
    pub static kCFRunLoopDefaultMode: CFTypeRef;
}

unsafe extern "C" {
    pub fn RunApplicationEventLoop();
}

unsafe extern "C" {
    pub fn EnableSecureEventInput() -> OSStatus;
    pub fn DisableSecureEventInput() -> OSStatus;
    pub fn CGSessionCopyCurrentDictionary() -> CFTypeRef;
}
