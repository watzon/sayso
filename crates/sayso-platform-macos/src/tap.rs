//! The `CGEventTap`s, each on its own thread.
//!
//! The listen-only tap handles modifier-only toggle, push-to-talk, Esc cancel
//! while recording, and the key recorder. It never
//! swallows events, so it cannot break system shortcuts such as double-Fn
//! dictation. It needs Input Monitoring.
//!
//! The active tap takes keys from the app, and that needs Accessibility. It
//! runs while "Send with Enter" (see [`crate::enter_send`]) or "take Esc"
//! (see [`crate::esc::EscTake`]) is on. It takes Return, keypad Enter, and
//! plain Esc only, and only while a dictation records, or while a key that it
//! took is still down. While it takes Esc, it also sends the Esc cancel, as
//! the listen-only tap may not see an Esc that it took.
//!
//! Spike result: with Secure Event Input on, the tap still gets
//! `flagsChanged`, but no `keyDown` or `keyUp`. Solo-modifier push-to-talk
//! keeps working. Modifier-only toggles are disabled because an intervening
//! regular key cannot be detected. Chord push-to-talk, Esc, the key recorder,
//! and the active tap do not work.

use crate::enter_send::EnterSend;
use crate::esc::EscDetector;
use crate::ffi::*;
use crate::keymap::{KC_ESCAPE, KC_KEYPAD_ENTER, KC_RETURN};
use crate::tap_logic::{RawEvent, TapLogic};
use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use sayso_core::hotkey::{Hotkey, ModifierSet};
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

/// What a tap does with its events.
enum Job {
    /// The listen-only tap.
    Listen { logic: Arc<Mutex<TapLogic>>, sink: Arc<TapSink> },
    /// The active tap: Enter, and Esc while Sayso takes it.
    Active { enter: Arc<Mutex<EnterSend>>, logic: Arc<Mutex<TapLogic>>, events: Sender<HotkeyEvent> },
}

/// Everything the C callback needs. Lives from thread start to thread end.
struct CallbackContext {
    job: Job,
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

/// Owns the tap threads and the logic they run.
pub struct TapController {
    logic: Arc<Mutex<TapLogic>>,
    sink: Arc<TapSink>,
    thread: Mutex<Option<TapThread>>,
    /// The state of Send with Enter, shared with the active tap.
    enter_logic: Arc<Mutex<EnterSend>>,
    /// The active tap, while Send with Enter or "take Esc" is on.
    active: Mutex<Option<TapThread>>,
}

impl TapController {
    pub fn new(events: Sender<HotkeyEvent>, capturing: Arc<AtomicBool>) -> Self {
        let esc = EscDetector::new(false, Duration::from_millis(400));
        Self {
            logic: Arc::new(Mutex::new(TapLogic::new(esc))),
            sink: Arc::new(TapSink { events, capture: Mutex::new(None), capturing }),
            thread: Mutex::new(None),
            enter_logic: Arc::default(),
            active: Mutex::new(None),
        }
    }

    /// Apply the modifier-only toggle, push-to-talk, Esc, and Enter settings. Starts the taps when something needs them.
    pub fn configure(
        &self,
        ptt: Option<Hotkey>,
        toggle_modifiers: Option<ModifierSet>,
        single_esc: bool,
        window: Duration,
        enter_sends: bool,
        take_escape: bool,
    ) -> Result<(), String> {
        self.enter_logic.lock().set_on(enter_sends);
        self.logic.lock().set_take_escape(take_escape);
        if enter_sends || take_escape {
            self.ensure_active_tap();
        } else {
            self.active.lock().take();
            self.logic.lock().set_active_tap(false);
        }
        let (released, needed) = {
            let mut logic = self.logic.lock();
            logic.configure_esc(single_esc, window);
            logic.set_toggle_modifiers(toggle_modifiers);
            let released = logic.set_ptt(ptt);
            (released, logic.is_needed())
        };
        if let Some(event) = released {
            let _ = self.sink.events.send(event);
        }
        if !needed {
            return Ok(());
        }
        // Creating the tap makes the system ask for Input Monitoring. Sayso asks
        // when the user chooses a key that needs it, or in onboarding.
        // SAFETY: plain permission queries. Accessibility also lets a tap listen.
        if !unsafe { CGPreflightListenEventAccess() || AXIsProcessTrusted() } {
            return Err("Input Monitoring is not granted".into());
        }
        self.ensure_running()
    }

    pub fn set_recording(&self, recording: bool) -> Result<(), String> {
        let (needed, take_on) = {
            let mut logic = self.logic.lock();
            logic.set_recording(recording);
            (logic.is_needed(), logic.takes_escape())
        };
        let enter_on = {
            let mut enter = self.enter_logic.lock();
            enter.set_recording(recording);
            enter.is_on()
        };
        // Accessibility may have come since the last try.
        if recording && (enter_on || take_on) {
            self.ensure_active_tap();
        }
        if recording && needed { self.ensure_running() } else { Ok(()) }
    }

    /// Start the active tap if it does not run. Without it, Enter and Esc go to the app as usual.
    fn ensure_active_tap(&self) {
        let mut slot = self.active.lock();
        if slot.is_some() {
            return;
        }
        // Creating the tap makes the system ask for Accessibility. Sayso asks in
        // onboarding, after it says why. `set_recording` tries again.
        // SAFETY: plain permission query.
        if !unsafe { AXIsProcessTrusted() } {
            return;
        }
        let job = Job::Active { enter: self.enter_logic.clone(), logic: self.logic.clone(), events: self.sink.events.clone() };
        match spawn_tap(job) {
            Ok(thread) => {
                *slot = Some(thread);
                self.logic.lock().set_active_tap(true);
            }
            Err(err) => log::warn!("Send with Enter and taking Esc are unavailable: {err}"),
        }
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
        *slot = Some(spawn_tap(Job::Listen { logic: self.logic.clone(), sink: self.sink.clone() })?);
        Ok(())
    }
}

fn spawn_tap(job: Job) -> Result<TapThread, String> {
    let name = match job {
        Job::Listen { .. } => "sayso-event-tap",
        Job::Active { .. } => "sayso-active-tap",
    };
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<usize, String>>(1);
    let join = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || run_tap_thread(job, ready_tx))
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

fn listen_mask() -> u64 {
    (1u64 << kCGEventKeyDown) | (1 << kCGEventKeyUp) | (1 << kCGEventFlagsChanged)
        | (1 << kCGEventLeftMouseDown) | (1 << kCGEventRightMouseDown)
        | (1 << kCGEventOtherMouseDown) | (1 << kCGEventScrollWheel)
}

fn run_tap_thread(job: Job, ready: Sender<Result<usize, String>>) {
    let keys = (1u64 << kCGEventKeyDown) | (1 << kCGEventKeyUp);
    let (option, mask) = match job {
        Job::Listen { .. } => (kCGEventTapOptionListenOnly, listen_mask()),
        Job::Active { .. } => (kCGEventTapOptionDefault, keys),
    };
    let listen = matches!(job, Job::Listen { .. });
    let ctx = Box::into_raw(Box::new(CallbackContext { job, tap: AtomicPtr::new(ptr::null_mut()) }));
    // SAFETY: `ctx` stays alive until after the run loop returns, below. The
    // callback only reads it through a shared reference.
    unsafe {
        let tap = CGEventTapCreate(kCGSessionEventTap, kCGHeadInsertEventTap, option, mask, callback, ctx.cast());
        if tap.is_null() {
            drop(Box::from_raw(ctx));
            let _ = ready.send(Err(if listen {
                format!(
                    "CGEventTapCreate failed. Input Monitoring is probably not granted (preflight: {})",
                    CGPreflightListenEventAccess()
                )
            } else {
                format!("CGEventTapCreate failed. Accessibility is probably not granted (trusted: {})", AXIsProcessTrusted())
            }));
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
    let swallow = std::panic::catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `user` is the `CallbackContext` pointer from `run_tap_thread`.
        let ctx = unsafe { &*(user as *const CallbackContext) };
        handle(ctx, etype, event)
    }));
    // A null return takes the event from the app. A listen-only tap never does.
    if swallow.unwrap_or(false) { ptr::null_mut() } else { event }
}

/// Handle one event. True when the app must not get it.
fn handle(ctx: &CallbackContext, etype: u32, event: *mut c_void) -> bool {
    if etype == kCGEventTapDisabledByTimeout || etype == kCGEventTapDisabledByUserInput {
        log::warn!("event tap disabled by the system (type {etype:#x}); re-enabling");
        // SAFETY: the stored pointer is the live tap port.
        unsafe { CGEventTapEnable(ctx.tap.load(Ordering::SeqCst), true) };
        // Key-up events may have been missed while the tap was off.
        if let Job::Listen { logic, sink } = &ctx.job
            && let Some(up) = logic.lock().release_held()
        {
            let _ = sink.events.send(up);
        }
        return false;
    }
    match &ctx.job {
        Job::Listen { logic, sink } => {
            listen(logic, sink, etype, event);
            false
        }
        Job::Active { enter, logic, events } => {
            if etype != kCGEventKeyDown && etype != kCGEventKeyUp {
                return false;
            }
            // SAFETY: `event` is a live CGEvent for the duration of the callback.
            let (code, flags, repeat) = unsafe {
                (
                    CGEventGetIntegerValueField(event, kCGKeyboardEventKeycode) as u16,
                    CGEventGetFlags(event),
                    CGEventGetIntegerValueField(event, kCGKeyboardEventAutorepeat) != 0,
                )
            };
            let down = etype == kCGEventKeyDown;
            match code {
                KC_RETURN | KC_KEYPAD_ENTER => {
                    let decision = enter.lock().key(down, repeat);
                    if decision.submit {
                        let _ = events.send(HotkeyEvent::Submit);
                    }
                    decision.swallow
                }
                KC_ESCAPE => {
                    let (cancel, swallow) = logic.lock().active_esc(down, repeat, flags, Instant::now());
                    if let Some(cancel) = cancel {
                        let _ = events.send(cancel);
                    }
                    swallow
                }
                _ => false,
            }
        }
    }
}

fn listen(logic: &Mutex<TapLogic>, sink: &TapSink, etype: u32, event: *mut c_void) {
    let pointer = [kCGEventLeftMouseDown, kCGEventRightMouseDown,
        kCGEventOtherMouseDown, kCGEventScrollWheel].contains(&etype);
    // SAFETY: `event` is a live CGEvent for the duration of the callback.
    let (code, flags, repeat) = unsafe {
        (
            CGEventGetIntegerValueField(event, kCGKeyboardEventKeycode) as u16,
            CGEventGetFlags(event),
            CGEventGetIntegerValueField(event, kCGKeyboardEventAutorepeat) != 0,
        )
    };
    let raw = if pointer {
        RawEvent::PointerActivity
    } else if etype == kCGEventKeyDown {
        RawEvent::KeyDown { code, flags, repeat }
    } else if etype == kCGEventKeyUp {
        RawEvent::KeyUp { code, flags }
    } else if etype == kCGEventFlagsChanged {
        // SAFETY: this Carbon query takes no pointers and reads the current system state.
        let secure_input = unsafe { IsSecureEventInputEnabled() };
        RawEvent::FlagsChanged { code, flags, secure_input }
    } else {
        return;
    };
    let outcome = {
        let mut logic = logic.lock();
        if etype == kCGEventFlagsChanged {
            // SAFETY: this query takes only scalar values and reads HID state.
            // Reconcile before gating the gesture, including the first event
            // after secure input ended without any intervening flagsChanged.
            logic.reconcile_pressed_keys(|code| unsafe {
                CGEventSourceKeyState(kCGEventSourceStateHIDSystemState, code)
            });
        }
        logic.handle(raw, Instant::now())
    };
    for event in outcome.events {
        let _ = sink.events.send(event);
    }
    if let Some(hotkey) = outcome.captured {
        sink.capturing.store(false, Ordering::SeqCst);
        if let Some(tx) = sink.capture.lock().take() {
            let _ = tx.send(hotkey);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_mask_includes_buttons_and_scroll_without_mouse_motion() {
        let mask = listen_mask();
        for etype in [kCGEventKeyDown, kCGEventKeyUp, kCGEventFlagsChanged,
            kCGEventLeftMouseDown, kCGEventRightMouseDown, kCGEventOtherMouseDown, kCGEventScrollWheel] {
            assert_ne!(mask & (1u64 << etype), 0);
        }
        // Pointer motion alone must not cancel a keyboard gesture.
        assert_eq!(mask & (1u64 << 5), 0);
    }

    #[test]
    #[ignore = "needs Accessibility for the test binary; sends no keys"]
    fn the_active_tap_runs_only_while_a_setting_needs_it() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let taps = TapController::new(tx, Arc::default());
        let window = Duration::from_millis(400);
        taps.configure(None, None, false, window, true, false).unwrap();
        assert!(taps.active.lock().is_some(), "the active tap could not start");
        let _ = taps.set_recording(true);
        let _ = taps.set_recording(false);
        assert!(taps.active.lock().is_some(), "it stays for the next recording");
        taps.configure(None, None, false, window, false, false).unwrap();
        assert!(taps.active.lock().is_none(), "both settings are off");
        let _ = taps.set_recording(true);
        assert!(taps.active.lock().is_none());
        let _ = taps.set_recording(false);
        taps.configure(None, None, false, window, false, true).unwrap();
        assert!(taps.active.lock().is_some(), "taking Esc needs it too");
        taps.configure(None, None, false, window, false, false).unwrap();
    }
}
