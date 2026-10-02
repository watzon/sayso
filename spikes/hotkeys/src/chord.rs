//! Mode `chord`: Carbon RegisterEventHotKey (raw FFI) and the global-hotkey crate.
use crate::{ffi::*, keys, now};
use std::{ffi::c_void, ptr, time::{Duration, Instant}};

extern "C" fn on_hotkey(_c: *mut c_void, ev: *mut c_void, _u: *mut c_void) -> OSStatus {
    unsafe {
        let mut id = EventHotKeyID { signature: 0, id: 0 };
        GetEventParameter(ev, kEventParamDirectObject, typeEventHotKeyID, ptr::null_mut(),
            size_of::<EventHotKeyID>(), ptr::null_mut(), &mut id as *mut _ as *mut c_void);
        let kind = match GetEventKind(ev) {
            kEventHotKeyPressed => "PRESS",
            kEventHotKeyReleased => "RELEASE",
            _ => "?",
        };
        println!("[{}] chord {kind} id={}", now(), id.id);
    }
    noErr
}

/// Registers a chord and returns (status, ref). Installs the handler once.
pub fn register(c: &keys::Chord, id: u32) -> (OSStatus, *mut c_void) {
    unsafe {
        let specs = [
            EventTypeSpec { event_class: kEventClassKeyboard, event_kind: kEventHotKeyPressed },
            EventTypeSpec { event_class: kEventClassKeyboard, event_kind: kEventHotKeyReleased },
        ];
        let mut h = ptr::null_mut();
        InstallEventHandler(GetApplicationEventTarget(), on_hotkey, 2, specs.as_ptr(), ptr::null_mut(), &mut h);
        let mut r = ptr::null_mut();
        let st = RegisterEventHotKey(c.keycode, c.carbon,
            EventHotKeyID { signature: u32::from_be_bytes(*b"sayo"), id }, GetApplicationEventTarget(), 0, &mut r);
        (st, r)
    }
}

/// Spin the main event loop. `secs` = None runs forever.
pub fn run_loop(secs: Option<f64>, mut poll: impl FnMut()) {
    // Carbon hotkey events are NOT delivered to a bare CFRunLoop in a CLI (verified); they need the
    // Carbon application event loop (or NSApplication). CHORD_LOOP=cf keeps the broken variant for comparison.
    if std::env::var("CHORD_LOOP").as_deref() != Ok("cf") {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs_f64(secs.unwrap_or(1e9)));
            std::process::exit(0);
        });
        unsafe { RunApplicationEventLoop() };
        return;
    }
    let end = secs.map(|s| Instant::now() + Duration::from_secs_f64(s));
    while end.is_none_or(|e| Instant::now() < e) {
        unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, false) };
        poll();
    }
}

pub fn run(args: &[String]) {
    let chord = keys::parse(args.first().map(String::as_str).unwrap_or("opt+space")).expect("chord");
    let secs = args.get(1).and_then(|s| s.parse().ok());
    let (st, _r) = register(&chord, 1);
    println!("[{}] RegisterEventHotKey({}) -> OSStatus {st}", now(), keys::describe(&chord));
    if st != noErr { return; }
    println!("listening{}; press the chord (Ctrl-C to quit)", secs.map(|s| format!(" for {s}s")).unwrap_or_default());
    run_loop(secs, || {});
}

/// Same thing through the global-hotkey crate, for comparison.
pub fn run_gh(args: &[String]) {
    use global_hotkey::{hotkey::{Code, HotKey, Modifiers}, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    let secs = args.first().and_then(|s| s.parse().ok());
    let mgr = GlobalHotKeyManager::new().expect("manager");
    let hk = HotKey::new(Some(Modifiers::ALT), Code::Space);
    println!("[{}] global-hotkey register(Alt+Space) -> {:?}", now(), mgr.register(hk));
    // Events arrive on a channel; drain it from a thread so the main thread can run the event loop.
    std::thread::spawn(|| {
        while let Ok(e) = GlobalHotKeyEvent::receiver().recv() {
            let s = if e.state == HotKeyState::Pressed { "PRESS" } else { "RELEASE" };
            println!("[{}] global-hotkey {s} id={}", now(), e.id);
        }
    });
    run_loop(secs, || {});
}
