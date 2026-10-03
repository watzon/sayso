//! The listen-only low-level keyboard hook (`WH_KEYBOARD_LL`) on its own
//! thread with its own message loop.
//!
//! The hook serves three jobs: push-to-talk (solo modifiers and chords), Esc
//! cancel while recording, and the key recorder. It always passes each event
//! on with `CallNextHookEx` and never swallows one, so it cannot break other
//! shortcuts. It needs no permission. It is installed only while one of the
//! three jobs needs it, and removed again after.
//!
//! Windows removes a hook without notice when its callback is slow (the
//! `LowLevelHooksTimeout`, about one second). So the callback only reads the
//! key state, runs the pure [`HookLogic`], and sends on channels. Everything
//! else (the mask key, stopping the thread) goes through thread messages.
//!
//! The hook does not see keys while an app that runs as administrator is in
//! front. Windows keeps those keys from programs at a lower level.

use crate::esc::EscDetector;
use crate::hook_logic::{HookLogic, LLKHF_EXTENDED, LLKHF_INJECTED, RawEvent};
use crate::input;
use crate::keymap::{self, ModKeys};
use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use sayso_core::hotkey::Hotkey;
use sayso_platform::HotkeyEvent;
use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, MSG, PM_NOREMOVE, PeekMessageW,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP,
    WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
};

/// Thread message: send the mask key.
const WM_APP_MASK: u32 = WM_APP + 1;
/// Thread message: check whether the hook is still needed, and stop if not.
const WM_APP_IDLE: u32 = WM_APP + 2;

/// How long an abandoned key recorder keeps listening.
const CAPTURE_EXPIRY: Duration = Duration::from_secs(15);

/// Where the hook delivers its results.
pub struct HookSink {
    events: Sender<HotkeyEvent>,
    capture: Mutex<Option<Sender<Hotkey>>>,
    /// True while the key recorder is open. The chord forwarder reads it to
    /// drop chord events during recording.
    pub capturing: Arc<AtomicBool>,
}

struct HookThread {
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

impl Drop for HookThread {
    fn drop(&mut self) {
        // SAFETY: plain thread id query.
        if self.thread_id == unsafe { GetCurrentThreadId() } {
            // The hook thread is stopping itself; it returns on its own.
            return;
        }
        // SAFETY: posting to a thread id is safe even after the thread ended.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// State shared by the controller, the hook thread, and the expiry timer.
struct Shared {
    logic: Mutex<HookLogic>,
    sink: HookSink,
    thread: Mutex<Option<HookThread>>,
}

thread_local! {
    /// The state the hook callback of this thread works with. A low-level
    /// hook runs on the thread that installed it, so each hook thread has its own.
    static CONTEXT: RefCell<Option<Arc<Shared>>> = const { RefCell::new(None) };
}

/// Owns the hook thread and the logic it runs.
pub struct HookController {
    shared: Arc<Shared>,
}

impl HookController {
    pub fn new(events: Sender<HotkeyEvent>, capturing: Arc<AtomicBool>) -> Self {
        let esc = EscDetector::new(false, Duration::from_millis(400));
        Self {
            shared: Arc::new(Shared {
                logic: Mutex::new(HookLogic::new(esc)),
                sink: HookSink { events, capture: Mutex::new(None), capturing },
                thread: Mutex::new(None),
            }),
        }
    }

    /// Apply push-to-talk and Esc settings. Installs or removes the hook.
    pub fn configure(&self, ptt: Option<Hotkey>, single_esc: bool, window: Duration) -> Result<(), String> {
        let released = {
            let mut logic = self.shared.logic.lock();
            logic.configure_esc(single_esc, window);
            logic.set_ptt(ptt)
        };
        if let Some(event) = released {
            let _ = self.shared.sink.events.send(event);
        }
        refresh(&self.shared)
    }

    pub fn set_recording(&self, recording: bool) -> Result<(), String> {
        self.shared.logic.lock().set_recording(recording);
        refresh(&self.shared)
    }

    /// Open the key recorder. The next completed combination arrives once on the channel.
    /// When the hook cannot start, the channel closes without a value.
    pub fn capture(&self) -> Receiver<Hotkey> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mine = tx.clone();
        let shared = &self.shared;
        *shared.sink.capture.lock() = Some(tx);
        shared.sink.capturing.store(true, Ordering::SeqCst);
        shared.logic.lock().set_capturing(true);
        if let Err(err) = refresh(shared) {
            log::warn!("key recorder cannot listen: {err}");
            end_capture(shared);
        }
        // An abandoned recorder must not keep the next key press: expire after 15 s.
        let shared = shared.clone();
        std::thread::spawn(move || {
            std::thread::sleep(CAPTURE_EXPIRY);
            let expired = shared.sink.capture.lock().as_ref().is_some_and(|tx| tx.same_channel(&mine));
            if expired {
                end_capture(&shared);
                if let Err(err) = refresh(&shared) {
                    log::warn!("keyboard hook: {err}");
                }
            }
        });
        rx
    }

    /// True while the hook is installed. For tests and diagnostics.
    pub fn is_installed(&self) -> bool {
        self.shared.thread.lock().as_ref().is_some_and(|t| t.join.as_ref().is_some_and(|j| !j.is_finished()))
    }
}

impl Drop for HookController {
    fn drop(&mut self) {
        let thread = self.shared.thread.lock().take();
        drop(thread);
    }
}

fn end_capture(shared: &Shared) {
    shared.sink.capture.lock().take();
    shared.sink.capturing.store(false, Ordering::SeqCst);
    shared.logic.lock().set_capturing(false);
}

/// Install the hook when the logic needs it, remove it when not.
fn refresh(shared: &Arc<Shared>) -> Result<(), String> {
    let needed = shared.logic.lock().is_needed();
    let mut slot = shared.thread.lock();
    // A thread that ended on its own (or failed) leaves a dead entry.
    if slot.as_ref().is_some_and(|t| t.join.as_ref().is_none_or(|j| j.is_finished())) {
        slot.take();
    }
    if needed {
        if slot.is_none() {
            *slot = Some(spawn_hook(shared.clone())?);
        }
        Ok(())
    } else {
        let thread = slot.take();
        // Join outside the lock: the hook thread may want it to stop itself.
        drop(slot);
        drop(thread);
        Ok(())
    }
}

fn spawn_hook(shared: Arc<Shared>) -> Result<HookThread, String> {
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<u32, String>>(1);
    let join = std::thread::Builder::new()
        .name("sayso-keyboard-hook".into())
        .spawn(move || run_hook_thread(shared, ready_tx))
        .map_err(|e| format!("cannot start the keyboard hook thread: {e}"))?;
    match ready_rx.recv() {
        Ok(Ok(thread_id)) => Ok(HookThread { thread_id, join: Some(join) }),
        Ok(Err(err)) => {
            let _ = join.join();
            Err(err)
        }
        Err(_) => Err("the keyboard hook thread stopped before it was ready".into()),
    }
}

fn run_hook_thread(shared: Arc<Shared>, ready: Sender<Result<u32, String>>) {
    let mut msg = MSG::default();
    // SAFETY: all calls run on this thread with valid out-pointers. The
    // callback reads CONTEXT, which is set before the hook and cleared after it.
    unsafe {
        // Create the message queue before anyone posts to it.
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        CONTEXT.with(|c| *c.borrow_mut() = Some(shared.clone()));
        let module = GetModuleHandleW(None).ok().map(|m| m.into());
        let hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module, 0) {
            Ok(hook) => hook,
            Err(err) => {
                CONTEXT.with(|c| c.borrow_mut().take());
                let _ = ready.send(Err(format!("SetWindowsHookExW failed: {err}")));
                return;
            }
        };
        let me = GetCurrentThreadId();
        let _ = ready.send(Ok(me));

        loop {
            let got = GetMessageW(&mut msg, None, 0, 0);
            if got.0 <= 0 {
                break;
            }
            match msg.message {
                WM_APP_MASK => {
                    if let Err(err) = input::send(&input::mask_strokes()) {
                        log::debug!("mask key: {err}");
                    }
                }
                WM_APP_IDLE => {
                    if stop_if_idle(&shared, me) {
                        break;
                    }
                }
                _ => {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }

        let _ = UnhookWindowsHookEx(hook);
        CONTEXT.with(|c| c.borrow_mut().take());
    }
    // Key-up events are lost once the hook is gone.
    if let Some(up) = shared.logic.lock().release_held() {
        let _ = shared.sink.events.send(up);
    }
}

/// Called on the hook thread: stop when nothing needs the hook any more.
/// Decided under the thread lock, so a concurrent start cannot get lost.
fn stop_if_idle(shared: &Shared, me: u32) -> bool {
    let mut slot = shared.thread.lock();
    let mine = slot.as_ref().is_some_and(|t| t.thread_id == me);
    if mine && !shared.logic.lock().is_needed() {
        // Dropping our own entry does not join (see `HookThread::drop`).
        slot.take();
        return true;
    }
    false
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // A panic must not unwind into Windows.
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: for HC_ACTION, `lparam` points to a KBDLLHOOKSTRUCT.
            let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            CONTEXT.with(|c| {
                if let Some(shared) = c.borrow().as_ref() {
                    handle(shared, info, wparam.0 as u32);
                }
            });
        }));
    }
    // Listen only: every event goes on to the next hook and the app.
    // SAFETY: passes the arguments through unchanged.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// The modifier keys that are down now, except `skip`.
fn held_modifiers(skip: u16) -> ModKeys {
    let down: Vec<u16> = keymap::MODIFIER_VKS
        .into_iter()
        .filter(|vk| *vk != skip)
        // SAFETY: plain key state query.
        .filter(|vk| unsafe { GetAsyncKeyState(i32::from(*vk)) } < 0)
        .collect();
    ModKeys::of(&down)
}

/// Injected events the hook has seen, so a test can tell "ignored" from "never called".
#[cfg(test)]
static INJECTED_SEEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn handle(shared: &Shared, info: &KBDLLHOOKSTRUCT, message: u32) {
    let flags = info.flags.0;
    if flags & LLKHF_INJECTED != 0 {
        #[cfg(test)]
        INJECTED_SEEN.fetch_add(1, Ordering::SeqCst);
        return;
    }
    let vk = keymap::sided_vk(info.vkCode as u16, info.scanCode, flags & LLKHF_EXTENDED != 0);
    let event = RawEvent {
        vk,
        scan: info.scanCode,
        flags,
        down: message == WM_KEYDOWN || message == WM_SYSKEYDOWN,
        held: held_modifiers(vk),
    };
    let outcome = shared.logic.lock().handle(event, Instant::now());
    for event in outcome.events {
        let _ = shared.sink.events.send(event);
    }
    // SAFETY: posting to our own thread's queue.
    let post = |msg| unsafe { PostThreadMessageW(GetCurrentThreadId(), msg, WPARAM(0), LPARAM(0)) };
    if outcome.mask {
        let _ = post(WM_APP_MASK);
    }
    if let Some(hotkey) = outcome.captured {
        shared.sink.capturing.store(false, Ordering::SeqCst);
        if let Some(tx) = shared.sink.capture.lock().take() {
            let _ = tx.send(hotkey);
        }
        // The recorder may have been the only reason for the hook.
        let _ = post(WM_APP_IDLE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hook_is_installed_only_while_needed() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let hook = HookController::new(tx, Arc::default());
        assert!(!hook.is_installed());
        hook.set_recording(true).unwrap();
        assert!(hook.is_installed());
        hook.set_recording(false).unwrap();
        assert!(!hook.is_installed());
        hook.configure(Some("right_shift".parse().unwrap()), false, Duration::from_millis(400)).unwrap();
        assert!(hook.is_installed());
        hook.configure(None, false, Duration::from_millis(400)).unwrap();
        assert!(!hook.is_installed());
    }

    #[test]
    fn a_ptt_with_fn_does_not_install_the_hook() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let hook = HookController::new(tx, Arc::default());
        hook.configure(Some("fn".parse().unwrap()), false, Duration::from_millis(400)).unwrap();
        assert!(!hook.is_installed());
    }

    #[test]
    #[ignore = "sends Right Alt to a window of its own"]
    fn injected_keys_reach_the_hook_but_never_trigger_push_to_talk() {
        use crate::input::{Stroke, send};
        let window = crate::test_window::TestWindow::open();
        assert!(window.wait_in_front(), "the test window could not come to the front; no keys were sent");
        let (tx, rx) = crossbeam_channel::unbounded();
        let hook = HookController::new(tx, Arc::default());
        hook.configure(Some("right_option".parse().unwrap()), false, Duration::from_millis(400)).unwrap();
        let before = INJECTED_SEEN.load(Ordering::SeqCst);
        assert!(window.is_in_front());
        let right_alt = keymap::VK_RMENU;
        send(&[Stroke::Key { vk: right_alt, down: true }, Stroke::Key { vk: right_alt, down: false }]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while INJECTED_SEEN.load(Ordering::SeqCst) < before + 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        hook.configure(None, false, Duration::from_millis(400)).unwrap();
        assert!(INJECTED_SEEN.load(Ordering::SeqCst) >= before + 2, "the hook saw both events");
        assert!(rx.try_recv().is_err(), "Sayso's own keys never trigger push-to-talk");
    }

    #[test]
    fn opening_the_recorder_installs_the_hook() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let capturing = Arc::new(AtomicBool::new(false));
        let hook = HookController::new(tx, capturing.clone());
        let _rx = hook.capture();
        assert!(hook.is_installed());
        assert!(capturing.load(Ordering::SeqCst));
    }
}
