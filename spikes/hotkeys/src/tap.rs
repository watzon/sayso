//! Mode `tap`: listen-only CGEventTap, push-to-talk detection, double-Esc detector.
use crate::{chord::run_loop, ffi::*, keys, now};
use std::{ffi::c_void, ptr, time::{Duration, Instant}};

const KC_RIGHT_OPTION: i64 = 61;
const KC_FN: i64 = 63;
const KC_D: i64 = 2;
const KC_ESC: i64 = 53;
/// Device-dependent bit (IOKit NX_DEVICERALTKEYMASK): set only while the RIGHT option key is down.
const DEVICE_RALT: u64 = 0x40;
const MODS: u64 = FLAG_SHIFT | FLAG_CONTROL | FLAG_ALT | FLAG_COMMAND;

#[derive(Default)]
struct Ptt {
    name: &'static str,
    active: bool,
    /// Another key was pressed while this modifier-only PTT was held (it is part of a combo).
    tainted: bool,
}

struct State {
    tap: CFTypeRef,
    ptt: [Ptt; 3], // 0 = Right Option, 1 = Fn, 2 = Ctrl+Shift+D
    force_recording: bool,
    esc_window: Duration,
    single_esc: bool,
    last_esc: Option<Instant>,
}

impl State {
    fn recording(&self) -> bool { self.force_recording || self.ptt.iter().any(|p| p.active) }

    fn set(&mut self, i: usize, down: bool) {
        let p = &mut self.ptt[i];
        if p.active == down { return; }
        p.active = down;
        if down { p.tainted = false; }
        let tag = if down { "PTT-PRESS" } else { "PTT-RELEASE" };
        let note = if !down && p.tainted { " (was used as part of a combo)" } else { "" };
        println!("[{}] >>> {tag} {}{note}", now(), p.name);
    }

    fn esc(&mut self) {
        if !self.recording() { println!("[{}]     Esc ignored (not recording)", now()); return; }
        let t = Instant::now();
        let double = self.last_esc.is_some_and(|l| t - l <= self.esc_window);
        if self.single_esc || double {
            println!("[{}] >>> CANCEL ({})", now(), if self.single_esc { "single Esc" } else { "double Esc" });
            self.last_esc = None;
        } else {
            self.last_esc = Some(t);
        }
    }
}

extern "C" fn callback(_p: *mut c_void, etype: u32, ev: *mut c_void, user: *mut c_void) -> *mut c_void {
    let st = unsafe { &mut *(user as *mut State) };
    if etype == kCGEventTapDisabledByTimeout || etype == kCGEventTapDisabledByUserInput {
        println!("[{}] tap disabled by system (type {etype:#x}); re-enabling", now());
        unsafe { CGEventTapEnable(st.tap, true) };
        return ev;
    }
    let (code, flags, repeat) = unsafe {
        (CGEventGetIntegerValueField(ev, kCGKeyboardEventKeycode), CGEventGetFlags(ev),
         CGEventGetIntegerValueField(ev, kCGKeyboardEventAutorepeat) != 0)
    };
    let kind = match etype { kCGEventKeyDown => "keyDown", kCGEventKeyUp => "keyUp", _ => "flagsChanged" };
    println!("[{}] {kind:<12} code={code:<3} ({}) flags={flags:#010x}{}", now(), keys::keycode_name(code as u32),
        if repeat { " repeat" } else { "" });

    match etype {
        kCGEventFlagsChanged => match code {
            // Modifier-only PTT: keycode picks the physical key, flags say down or up.
            KC_RIGHT_OPTION => st.set(0, flags & DEVICE_RALT != 0),
            KC_FN => st.set(1, flags & FLAG_SECONDARY_FN != 0),
            _ => {}
        },
        kCGEventKeyDown if !repeat => {
            // Any normal key while a modifier-only PTT is held marks it as a combo.
            if code != KC_ESC {
                for p in st.ptt.iter_mut().take(2).filter(|p| p.active) { p.tainted = true; }
            }
            if code == KC_D && flags & MODS == FLAG_CONTROL | FLAG_SHIFT { st.set(2, true); }
            if code == KC_ESC { st.esc(); }
        }
        kCGEventKeyUp if code == KC_D => st.set(2, false),
        _ => {}
    }
    // Also release the chord PTT if its modifiers go away first.
    if etype == kCGEventFlagsChanged && flags & MODS != FLAG_CONTROL | FLAG_SHIFT { st.set(2, false); }
    ev
}

pub fn run(args: &[String]) {
    let secs = args.iter().find_map(|a| a.parse::<f64>().ok());
    let st = Box::into_raw(Box::new(State {
        tap: ptr::null(),
        ptt: [
            Ptt { name: "Right Option", ..Default::default() },
            Ptt { name: "Fn", ..Default::default() },
            Ptt { name: "Ctrl+Shift+D", ..Default::default() },
        ],
        force_recording: args.iter().any(|a| a == "--recording"),
        esc_window: Duration::from_millis(400),
        single_esc: args.iter().any(|a| a == "--single-esc"),
        last_esc: None,
    }));
    let mask = (1u64 << kCGEventKeyDown) | (1 << kCGEventKeyUp) | (1 << kCGEventFlagsChanged);
    unsafe {
        let tap = CGEventTapCreate(kCGSessionEventTap, kCGHeadInsertEventTap, kCGEventTapOptionListenOnly,
            mask, callback, st as *mut c_void);
        if tap.is_null() {
            println!("FAIL: CGEventTapCreate returned NULL. Input Monitoring is not granted to this process \
                (CGPreflightListenEventAccess={}). See RESULT.md for the human steps.", CGPreflightListenEventAccess());
            return;
        }
        (*st).tap = tap;
        let src = CFMachPortCreateRunLoopSource(ptr::null(), tap, 0);
        CFRunLoopAddSource(CFRunLoopGetCurrent(), src, kCFRunLoopCommonModes);
        CGEventTapEnable(tap, true);
    }
    println!("tap installed (listen-only). Try: hold Right Option, hold Fn, Ctrl+Shift+D, hold PTT then Esc Esc.");
    run_loop(secs, || {});
}

// ---- handy-keys evaluation ----
/// Same three PTT hotkeys through handy-keys 0.3.4, to compare behaviour and permissions.
pub fn run_handy(args: &[String]) {
    use handy_keys::{Hotkey, HotkeyManager, HotkeyState, Key, Modifiers};
    println!("handy-keys check_accessibility() = {}", handy_keys::check_accessibility());
    let mgr = match HotkeyManager::new() {
        Ok(m) => m,
        Err(e) => return println!("FAIL: HotkeyManager::new() -> {e}"),
    };
    let list = [
        Hotkey::new(Modifiers::OPT_RIGHT, None).unwrap(),
        Hotkey::new(Modifiers::FN, None).unwrap(),
        Hotkey::new(Modifiers::CTRL | Modifiers::SHIFT, Key::D).unwrap(),
    ];
    for h in list { println!("register {h} -> {:?}", mgr.register(h)); }
    let secs = args.iter().find_map(|a| a.parse::<f64>().ok());
    let end = secs.map(|s| Instant::now() + Duration::from_secs_f64(s));
    while end.is_none_or(|e| Instant::now() < e) {
        std::thread::sleep(Duration::from_millis(20));
        while let Some(ev) = mgr.try_recv() {
            let s = if ev.state == HotkeyState::Pressed { "PRESS" } else { "RELEASE" };
            println!("[{}] handy {s} {}", now(), mgr.get_hotkey(ev.id).map(|h| h.to_string()).unwrap_or_default());
        }
    }
}
