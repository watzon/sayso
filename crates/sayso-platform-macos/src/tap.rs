//! The listen-only `CGEventTap` on its own thread.
//!
//! The tap serves three jobs: push-to-talk (solo modifiers and chords), Esc
//! cancel while recording, and the key recorder. It never swallows events, so
//! it cannot break system shortcuts such as double-Fn dictation. It needs
//! Input Monitoring.
//!
//! Spike result: with Secure Event Input on, the tap still gets
//! `flagsChanged`, but no `keyDown` or `keyUp`. Solo-modifier push-to-talk
//! keeps working. Chord push-to-talk, Esc, and the key recorder do not.

use crate::esc::EscDetector;
use crate::ffi::*;
use crate::tap_logic::{RawEvent, TapLogic};
use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use sayso_core::hotkey::Hotkey;
use sayso_platform::HotkeyEvent;
use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Where the tap delivers its results.
pub struct TapSink {
    events: Sender<HotkeyEvent>,
    capture: Mutex<Option<Sender<Hotkey>>>,
    /// True while the key recorder is open. The Carbon forwarder reads it to
    /// drop chord events during recording.
    pub capturing: Arc<AtomicBool>,
}

/// Everything the C callback needs. Lives from thread start to thread end.
struct CallbackContext {
    logic: Arc<Mutex<TapLogic>>,
    sink: Arc<TapSink>,
    tap: AtomicPtr<c_void>,
}

struct TapThread {
    run_loop: usize,
    join: Option<JoinHandle<()>>,
}

impl Drop for TapThread {
    fn drop(&mut self) {
        // SAFETY: the run loop reference stays valid until the thread exits,
        // and CFRunLoopStop is safe to call from any thread.
        unsafe { CFRunLoopStop(self.run_loop as CFTypeRef) };
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Owns the tap thread and the logic it runs.
pub struct TapController {
    logic: Arc<Mutex<TapLogic>>,
    sink: Arc<TapSink>,
    thread: Mutex<Option<TapThread>>,
}

impl TapController {
    pub fn new(events: Sender<HotkeyEvent>, capturing: Arc<AtomicBool>) -> Self {
        let esc = EscDetector::new(false, Duration::from_millis(400));
        Self {
            logic: Arc::new(Mutex::new(TapLogic::new(esc))),
            sink: Arc::new(TapSink { events, capture: Mutex::new(None), capturing }),
            thread: Mutex::new(None),
        }
    }

    /// Apply push-to-talk and Esc settings. Starts the tap when something needs it.
    pub fn configure(&self, ptt: Option<Hotkey>, single_esc: bool, window: Duration) -> Result<(), String> {
        let (released, needed) = {
            let mut logic = self.logic.lock();
            logic.configure_esc(single_esc, window);
            let released = logic.set_ptt(ptt);
            (released, logic.is_needed())
        };
        if let Some(event) = released {
            let _ = self.sink.events.send(event);
        }
        if needed { self.ensure_running() } else { Ok(()) }
    }

    pub fn set_recording(&self, recording: bool) -> Result<(), String> {
        let needed = {
            let mut logic = self.logic.lock();
            logic.set_recording(recording);
            logic.is_needed()
        };
        if recording && needed { self.ensure_running() } else { Ok(()) }
    }

    /// Open the key recorder. The next completed combination arrives once on the channel.
    /// When the tap cannot start, the channel closes without a value.
    pub fn capture(&self) -> Receiver<Hotkey> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mine = tx.clone();
        *self.sink.capture.lock() = Some(tx);
        self.sink.capturing.store(true, Ordering::SeqCst);
        self.logic.lock().set_capturing(true);
        if let Err(err) = self.ensure_running() {
            log::warn!("key recorder cannot listen: {err}");
            self.sink.capture.lock().take();
            self.sink.capturing.store(false, Ordering::SeqCst);
            self.logic.lock().set_capturing(false);
        }
        // An abandoned recorder must not keep the next key press: expire after 15 s.
        let (sink, logic) = (self.sink.clone(), self.logic.clone());
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(15));
            let mut slot = sink.capture.lock();
            if slot.as_ref().is_some_and(|tx| tx.same_channel(&mine)) {
                slot.take();
                sink.capturing.store(false, Ordering::SeqCst);
                logic.lock().set_capturing(false);
            }
        });
        rx
    }

    fn ensure_running(&self) -> Result<(), String> {
        let mut slot = self.thread.lock();
        if slot.is_some() {
            return Ok(());
        }
        *slot = Some(spawn_tap(self.logic.clone(), self.sink.clone())?);
        Ok(())
    }
}

fn spawn_tap(logic: Arc<Mutex<TapLogic>>, sink: Arc<TapSink>) -> Result<TapThread, String> {
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<usize, String>>(1);
    let join = std::thread::Builder::new()
        .name("sayso-event-tap".into())
        .spawn(move || run_tap_thread(logic, sink, ready_tx))
        .map_err(|e| format!("cannot start the event tap thread: {e}"))?;
    match ready_rx.recv() {
        Ok(Ok(run_loop)) => Ok(TapThread { run_loop, join: Some(join) }),
        Ok(Err(err)) => {
            let _ = join.join();
            Err(err)
        }
        Err(_) => Err("the event tap thread stopped before it was ready".into()),
    }
}

fn run_tap_thread(logic: Arc<Mutex<TapLogic>>, sink: Arc<TapSink>, ready: Sender<Result<usize, String>>) {
    let ctx = Box::into_raw(Box::new(CallbackContext { logic, sink, tap: AtomicPtr::new(ptr::null_mut()) }));
    let mask = (1u64 << kCGEventKeyDown) | (1 << kCGEventKeyUp) | (1 << kCGEventFlagsChanged);
    // SAFETY: `ctx` stays alive until after the run loop returns, below. The
    // callback only reads it through a shared reference.
    unsafe {
        let tap = CGEventTapCreate(
            kCGSessionEventTap,
            kCGHeadInsertEventTap,
            kCGEventTapOptionListenOnly,
            mask,
            callback,
            ctx.cast(),
        );
        if tap.is_null() {
            drop(Box::from_raw(ctx));
            let _ = ready.send(Err(format!(
                "CGEventTapCreate failed. Input Monitoring is probably not granted (preflight: {})",
                CGPreflightListenEventAccess()
            )));
            return;
        }
        (*ctx).tap.store(tap.cast_mut(), Ordering::SeqCst);
        let source = CFMachPortCreateRunLoopSource(ptr::null(), tap, 0);
        let run_loop = CFRunLoopGetCurrent();
        CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
        CGEventTapEnable(tap, true);
        let _ = ready.send(Ok(run_loop as usize));

        CFRunLoopRun();

        CGEventTapEnable(tap, false);
        CFRelease(source);
        CFRelease(tap);
        drop(Box::from_raw(ctx));
    }
}

extern "C" fn callback(_proxy: *mut c_void, etype: u32, event: *mut c_void, user: *mut c_void) -> *mut c_void {
    // A panic must not unwind into C.
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `user` is the `CallbackContext` pointer from `run_tap_thread`.
        let ctx = unsafe { &*(user as *const CallbackContext) };
        handle(ctx, etype, event);
    }));
    event
}

fn handle(ctx: &CallbackContext, etype: u32, event: *mut c_void) {
    if etype == kCGEventTapDisabledByTimeout || etype == kCGEventTapDisabledByUserInput {
        log::warn!("event tap disabled by the system (type {etype:#x}); re-enabling");
        // SAFETY: the stored pointer is the live tap port.
        unsafe { CGEventTapEnable(ctx.tap.load(Ordering::SeqCst), true) };
        // Key-up events may have been missed while the tap was off.
        if let Some(up) = ctx.logic.lock().release_held() {
            let _ = ctx.sink.events.send(up);
        }
        return;
    }
    // SAFETY: `event` is a live CGEvent for the duration of the callback.
    let (code, flags, repeat) = unsafe {
        (
            CGEventGetIntegerValueField(event, kCGKeyboardEventKeycode) as u16,
            CGEventGetFlags(event),
            CGEventGetIntegerValueField(event, kCGKeyboardEventAutorepeat) != 0,
        )
    };
    let raw = if etype == kCGEventKeyDown {
        RawEvent::KeyDown { code, flags, repeat }
    } else if etype == kCGEventKeyUp {
        RawEvent::KeyUp { code, flags }
    } else if etype == kCGEventFlagsChanged {
        RawEvent::FlagsChanged { code, flags }
    } else {
        return;
    };
    let outcome = ctx.logic.lock().handle(raw, Instant::now());
    for event in outcome.events {
        let _ = ctx.sink.events.send(event);
    }
    if let Some(hotkey) = outcome.captured {
        ctx.sink.capturing.store(false, Ordering::SeqCst);
        if let Some(tx) = ctx.sink.capture.lock().take() {
            let _ = tx.send(hotkey);
        }
    }
}
