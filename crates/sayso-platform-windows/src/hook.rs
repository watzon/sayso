//! Low-level keyboard and mouse hooks on one thread with its own message loop.
//! Mouse buttons and wheels cancel modifier-only toggle candidates.
//!
//! The hook serves four jobs: push-to-talk (solo modifiers and chords), Esc
//! cancel while recording, the key recorder, and Send with Enter. It passes
//! each event on with `CallNextHookEx`, so it cannot break other shortcuts.
//! The exceptions are Send with Enter and taking Esc: with a setting on,
//! Enter or a plain Esc while recording goes to no app (see
//! [`crate::enter_send`] and [`crate::esc::EscTake`]). It needs no
//! permission. It is installed only while one of its jobs needs it, and
//! removed again after.
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
use sayso_core::hotkey::{Hotkey, ModifierSet};
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
use windows::Win32::UI::WindowsAndMessaging::HHOOK;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, MSG, PM_NOREMOVE, PeekMessageW,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP,
    WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN, WM_LBUTTONDOWN, WM_RBUTTONDOWN, WM_MBUTTONDOWN,
    WM_XBUTTONDOWN, WM_MOUSEWHEEL, WM_MOUSEHWHEEL,
};

/// Thread message: send the mask key.
const WM_APP_MASK: u32 = WM_APP + 1;
/// Thread message: check whether the hook is still needed, and stop if not.
const WM_APP_IDLE: u32 = WM_APP + 2;
/// Reconfigure the optional mouse hook on its owning thread.
const WM_APP_MOUSE: u32 = WM_APP + 3;

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
    mouse_refresh: Sender<Sender<Result<(), String>>>,
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
    #[cfg(test)]
    fail_mouse_install: AtomicBool,
    #[cfg(test)]
    fail_mouse_post: AtomicBool,
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
                #[cfg(test)]
                fail_mouse_install: AtomicBool::new(false),
                #[cfg(test)]
                fail_mouse_post: AtomicBool::new(false),
            }),
        }
    }

    /// Apply the modifier-only toggle, push-to-talk, Esc, and Enter settings. Installs or removes the hook.
    pub fn configure(
        &self,
        ptt: Option<Hotkey>,
        toggle_modifiers: Option<ModifierSet>,
        single_esc: bool,
        window: Duration,
        enter_sends: bool,
        take_escape: bool,
    ) -> Result<(), String> {
        let released = {
            let mut logic = self.shared.logic.lock();
            logic.configure_esc(single_esc, window);
            logic.set_toggle_modifiers(toggle_modifiers);
            // No tap can fire before the owning thread confirms visibility.
            logic.set_pointer_available(false);
            logic.set_enter_sends(enter_sends);
            logic.set_take_escape(take_escape);
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

    /// Modifier taps require pointer visibility; other keyboard jobs do not.
    pub(crate) fn modifier_toggle_available(&self) -> bool {
        self.shared.logic.lock().pointer_available()
    }

    #[cfg(test)]
    pub(crate) fn fail_mouse_install(&self) {
        self.shared.fail_mouse_install.store(true, Ordering::SeqCst);
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
        let thread = slot.as_ref().expect("hook just installed");
        let (tx, rx) = crossbeam_channel::bounded(1);
        thread.mouse_refresh.send(tx).map_err(|_| "keyboard hook stopped".to_string())?;
        let thread_id = thread.thread_id;
        // Force the real PostThreadMessageW API to reject a single request.
        #[cfg(test)]
        let thread_id = if shared.fail_mouse_post.swap(false, Ordering::SeqCst) { 0 } else { thread_id };
        // SAFETY: plain message to the hook thread's queue.
        if let Err(err) = unsafe { PostThreadMessageW(thread_id, WM_APP_MOUSE, WPARAM(0), LPARAM(0)) } {
            // The keyboard listener is already installed. A failed pointer
            // refresh cannot safely enable taps, but must not disable the
            // unrelated keyboard jobs or close a recorder.
            shared.logic.lock().set_pointer_available(false);
            log::warn!("modifier toggle unavailable: cannot refresh mouse hook: {err}");
            return Ok(());
        }
        // An earlier queued idle message may need the slot lock. Never wait
        // for the hook thread while owning it.
        drop(slot);
        if let Err(reason) = rx.recv().map_err(|_| "keyboard hook stopped".to_string())? {
            // sync_mouse_hook already disabled modifier taps. Do not turn a
            // pointer-only failure into a keyboard/recording/capture failure.
            log::warn!("modifier toggle unavailable: {reason}");
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
    let (mouse_refresh, requests) = crossbeam_channel::unbounded();
    let join = std::thread::Builder::new()
        .name("sayso-keyboard-hook".into())
        .spawn(move || run_hook_thread(shared, ready_tx, requests))
        .map_err(|e| format!("cannot start the keyboard hook thread: {e}"))?;
    match ready_rx.recv() {
        Ok(Ok(thread_id)) => Ok(HookThread { thread_id, join: Some(join), mouse_refresh }),
        Ok(Err(err)) => {
            let _ = join.join();
            Err(err)
        }
        Err(_) => Err("the keyboard hook thread stopped before it was ready".into()),
    }
}

/// Keep mouse-hook failure isolated from the keyboard hook. Only a modifier
/// toggle needs it; recording, PTT and capture keep their existing listener.
fn update_mouse_hook<T, E>(
    slot: &mut Option<T>,
    needed: bool,
    install: impl FnOnce() -> Result<T, E>,
    remove: impl FnOnce(T),
) -> Result<(), E> {
    if needed {
        if slot.is_none() {
            *slot = Some(install()?);
        }
    } else if let Some(hook) = slot.take() {
        remove(hook);
    }
    Ok(())
}

fn sync_mouse_hook(shared: &Shared, slot: &mut Option<HHOOK>) -> Result<(), String> {
    // Serialize the binding and availability with callbacks/configuration.
    let mut logic = shared.logic.lock();
    let result = update_mouse_hook(slot, logic.needs_mouse(), || {
        #[cfg(test)]
        if shared.fail_mouse_install.load(Ordering::SeqCst) {
            return Err("injected mouse installation failure".into());
        }
        // SAFETY: installed on this hook thread, which pumps its message loop.
        unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0) }
            .map_err(|e| format!("mouse SetWindowsHookExW failed: {e}"))
    }, |hook| {
        // SAFETY: our own installed mouse hook, on its owner thread.
        let _ = unsafe { UnhookWindowsHookEx(hook) };
    });
    logic.set_pointer_available(slot.is_some());
    result
}

fn run_hook_thread(shared: Arc<Shared>, ready: Sender<Result<u32, String>>, requests: Receiver<Sender<Result<(), String>>>) {
    let mut msg = MSG::default();
    // The previous hook may have stopped before receiving key-up events.
    if let Some(up) = shared.logic.lock().reset_keys() {
        let _ = shared.sink.events.send(up);
    }
    // SAFETY: plain query for this thread.
    let me = unsafe { GetCurrentThreadId() };
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
                let _ = ready.send(Err(format!("keyboard SetWindowsHookExW failed: {err}")));
                return;
            }
        };
        let mut mouse_hook = None;
        shared.logic.lock().set_pointer_available(false);
        let _ = ready.send(Ok(me));

        loop {
            let got = GetMessageW(&mut msg, None, 0, 0);
            if got.0 <= 0 {
                break;
            }
            match msg.message {
                WM_APP_MOUSE => {
                    // A failed post can leave an abandoned request queued.
                    // A wakeup drains all requests, so that stale reply cannot
                    // consume the only message belonging to the next caller.
                    while let Ok(reply) = requests.try_recv() {
                        let result = sync_mouse_hook(&shared, &mut mouse_hook);
                        let _ = reply.send(result);
                    }
                }
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

        if let Some(mouse_hook) = mouse_hook {
            let _ = UnhookWindowsHookEx(mouse_hook);
        }
        let _ = UnhookWindowsHookEx(hook);
        CONTEXT.with(|c| c.borrow_mut().take());
    }
    // Do not clear a replacement hook's gesture after stop_if_idle removed us.
    let slot = shared.thread.lock();
    if slot.as_ref().is_none_or(|thread| thread.thread_id == me)
        && let Some(up) = shared.logic.lock().release_held()
    {
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
    let mut swallow = false;
    if code == HC_ACTION as i32 {
        // A panic must not unwind into Windows.
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: for HC_ACTION, `lparam` points to a KBDLLHOOKSTRUCT.
            let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            CONTEXT.with(|c| {
                if let Some(shared) = c.borrow().as_ref() {
                    swallow = handle(shared, info, wparam.0 as u32);
                }
            });
        }));
    }
    if swallow {
        // Send with Enter or a taken Esc: no hook after us and no app gets this event.
        return LRESULT(1);
    }
    // Every other event goes on to the next hook and the app.
    // SAFETY: passes the arguments through unchanged.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && matches!(wparam.0 as u32,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_MOUSEWHEEL | WM_MOUSEHWHEEL
    ) {
        // Conservatively cancel even injected mouse gestures. They can change
        // focus or selection too; missing a tap is safer than opening the mic.
        // A panic must not unwind into Windows, and no mouse event is swallowed.
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
            CONTEXT.with(|c| {
                if let Some(shared) = c.borrow().as_ref() {
                    shared.logic.lock().cancel_modifier_tap();
                }
            });
        }));
    }
    // SAFETY: passes the original hook arguments through unchanged.
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

/// Handle one event. True when the app must not get it.
fn handle(shared: &Shared, info: &KBDLLHOOKSTRUCT, message: u32) -> bool {
    let flags = info.flags.0;
    if flags & LLKHF_INJECTED != 0 {
        #[cfg(test)]
        INJECTED_SEEN.fetch_add(1, Ordering::SeqCst);
        return false;
    }
    let vk = keymap::sided_vk(info.vkCode as u16, info.scanCode, flags & LLKHF_EXTENDED != 0);
    let event = RawEvent {
        vk,
        scan: info.scanCode,
        flags,
        down: message == WM_KEYDOWN || message == WM_SYSKEYDOWN,
        held: held_modifiers(vk),
    };
    let outcome = {
        let mut logic = shared.logic.lock();
        logic.prune_pressed(vk, |key| {
            // SAFETY: plain pre-event state query. Logic preserves swallowed
            // keys and the current regular key; modifiers can recover here.
            (unsafe { GetAsyncKeyState(i32::from(key)) }) < 0
        });
        logic.handle(event, Instant::now())
    };
    let swallow = outcome.swallow;
    if swallow && !event.down {
        // A taken Enter is up: it may have been the last reason for the hook.
        // SAFETY: posting to our own thread's queue.
        let _ = unsafe { PostThreadMessageW(GetCurrentThreadId(), WM_APP_IDLE, WPARAM(0), LPARAM(0)) };
    }
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
    swallow
}

#[cfg(test)]
#[path = "hook_native_tests.rs"]
mod native_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercise the production callback, including GetAsyncKeyState pruning.
    /// No OS input is emitted; all payloads live for the callback's duration.
    fn callback(shared: &Arc<Shared>, vk: u16, down: bool) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::WM_KEYUP;
        CONTEXT.with(|c| *c.borrow_mut() = Some(shared.clone()));
        let info = KBDLLHOOKSTRUCT { vkCode: u32::from(vk), ..Default::default() };
        // SAFETY: valid keyboard payload, no input or focus changes.
        let result = unsafe { hook_proc(HC_ACTION as i32, WPARAM(if down { WM_KEYDOWN } else { WM_KEYUP } as usize), LPARAM(&info as *const _ as isize)) };
        CONTEXT.with(|c| c.borrow_mut().take());
        result == LRESULT(1)
    }

    #[test]
    fn regression_callback_taken_enter_survives_intervening_key_and_stop() {
        use crate::keymap::{VK_RETURN, VK_LSHIFT};
        let (tx, rx) = crossbeam_channel::unbounded();
        let hook = HookController::new(tx, Arc::default());
        let mut logic = hook.shared.logic.lock();
        logic.set_enter_sends(true);
        logic.set_recording(true);
        drop(logic);
        assert!(callback(&hook.shared, VK_RETURN, true));
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Submit));
        hook.shared.logic.lock().set_recording(false);
        callback(&hook.shared, VK_LSHIFT, true);
        callback(&hook.shared, VK_LSHIFT, false);
        assert!(callback(&hook.shared, VK_RETURN, true), "held Enter must not leak after pruning");
        assert!(rx.try_recv().is_err(), "a repeat must not submit twice");
        assert!(callback(&hook.shared, VK_RETURN, false));
        assert!(!hook.shared.logic.lock().is_needed());
    }

    #[test]
    fn regression_callback_taken_esc_repeat_is_not_a_second_press() {
        use crate::keymap::{VK_ESCAPE, VK_LSHIFT};
        let (tx, rx) = crossbeam_channel::unbounded();
        let hook = HookController::new(tx, Arc::default());
        let mut logic = hook.shared.logic.lock();
        logic.set_take_escape(true);
        logic.set_recording(true);
        drop(logic);
        assert!(callback(&hook.shared, VK_ESCAPE, true));
        callback(&hook.shared, VK_LSHIFT, true);
        callback(&hook.shared, VK_LSHIFT, false);
        assert!(callback(&hook.shared, VK_ESCAPE, true));
        assert!(rx.try_recv().is_err(), "held Esc is not a double press");
        assert!(callback(&hook.shared, VK_ESCAPE, false));
        assert!(callback(&hook.shared, VK_ESCAPE, true));
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Cancel));
        hook.shared.logic.lock().set_recording(false);
        callback(&hook.shared, VK_LSHIFT, true);
        assert!(callback(&hook.shared, VK_ESCAPE, true), "repeat after cancel stays swallowed");
        assert!(callback(&hook.shared, VK_ESCAPE, false));
    }

    #[test]
    fn regression_mouse_callback_cancels_buttons_and_both_wheels_only() {
        use crate::keymap::VK_LSHIFT;
        use windows::Win32::UI::WindowsAndMessaging::{
            MSLLHOOKSTRUCT, WM_LBUTTONDOWN, WM_RBUTTONDOWN, WM_MBUTTONDOWN,
            WM_XBUTTONDOWN, WM_MOUSEWHEEL, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_LBUTTONUP,
        };
        let now = Instant::now();
        for (message, cancels) in [
            (WM_LBUTTONDOWN, true), (WM_RBUTTONDOWN, true), (WM_MBUTTONDOWN, true),
            (WM_XBUTTONDOWN, true), (WM_MOUSEWHEEL, true), (WM_MOUSEHWHEEL, true),
            (WM_MOUSEMOVE, false), (WM_LBUTTONUP, false),
        ] {
            let (tx, _rx) = crossbeam_channel::unbounded();
            let hook = HookController::new(tx, Arc::default());
            let mut logic = hook.shared.logic.lock();
            logic.set_toggle_modifiers("left_shift".parse::<Hotkey>().unwrap().modifier_keys());
            logic.handle(RawEvent { vk: VK_LSHIFT, scan: 0, flags: 0, down: true, held: ModKeys::of(&[]) }, now);
            drop(logic);
            CONTEXT.with(|c| *c.borrow_mut() = Some(hook.shared.clone()));
            // Injected mouse gestures also cancel: focus changes are unsafe
            // regardless of the input source. The callback never swallows them.
            let info = MSLLHOOKSTRUCT { flags: 1, ..Default::default() };
            // SAFETY: valid hook payload; no input is sent or window focused.
            unsafe { mouse_proc(HC_ACTION as i32, WPARAM(message as usize), LPARAM(&info as *const _ as isize)); }
            CONTEXT.with(|c| c.borrow_mut().take());
            let out = hook.shared.logic.lock().handle(RawEvent { vk: VK_LSHIFT, scan: 0, flags: 0, down: false, held: ModKeys::of(&[VK_LSHIFT]) }, now);
            assert_eq!(out.events, if cancels { vec![] } else { vec![HotkeyEvent::Toggle] }, "message={message:#x}");
        }
    }

    #[test]
    fn regression_mouse_hook_failure_pauses_only_modifier_toggle() {
        use crate::keymap::{VK_LSHIFT, VK_ESCAPE};
        let (tx, rx) = crossbeam_channel::unbounded();
        let hook = HookController::new(tx, Arc::default());
        let mut logic = hook.shared.logic.lock();
        logic.set_toggle_modifiers("left_shift".parse::<Hotkey>().unwrap().modifier_keys());
        logic.set_ptt(Some("right_shift".parse().unwrap()));
        logic.set_recording(true);
        logic.configure_esc(true, Duration::from_millis(400));
        logic.set_pointer_available(false);
        let now = Instant::now();
        for down in [true, false] {
            assert!(logic.handle(RawEvent { vk: VK_LSHIFT, scan: 0, flags: 0, down, held: ModKeys::of(&[]) }, now).events.is_empty());
        }
        assert!(logic.is_needed(), "keyboard jobs remain active");
        drop(logic);
        callback(&hook.shared, VK_ESCAPE, true);
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Cancel));
    }

    #[test]
    fn regression_mouse_install_failure_through_configuration_keeps_keyboard_jobs() {
        use crate::keymap::{VK_LSHIFT, VK_RSHIFT, VK_ESCAPE};
        let (tx, rx) = crossbeam_channel::unbounded();
        let hook = HookController::new(tx, Arc::default());
        hook.fail_mouse_install();
        assert!(hook.configure(Some("right_shift".parse().unwrap()),
            "left_shift".parse::<Hotkey>().unwrap().modifier_keys(), true,
            Duration::from_millis(400), false, false).is_ok(), "mouse failure must not fail keyboard configuration");
        assert!(hook.is_installed());
        assert!(!hook.shared.logic.lock().pointer_available());
        for down in [true, false] { callback(&hook.shared, VK_LSHIFT, down); }
        assert!(rx.try_recv().is_err(), "toggle must fail closed");
        callback(&hook.shared, VK_RSHIFT, true);
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::PushToTalkDown));
        callback(&hook.shared, VK_RSHIFT, false);
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::PushToTalkUp));
        assert!(hook.set_recording(true).is_ok(), "ordinary recording must not depend on mouse refresh");
        callback(&hook.shared, VK_ESCAPE, true);
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Cancel));
        callback(&hook.shared, VK_ESCAPE, false);
    }

    #[test]
    fn regression_mouse_install_failure_through_capture_keeps_recorder_open() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let capturing = Arc::new(AtomicBool::new(false));
        let hook = HookController::new(tx, capturing.clone());
        // Set up the binding before capture starts the real keyboard thread.
        hook.shared.logic.lock().set_toggle_modifiers("left_shift".parse::<Hotkey>().unwrap().modifier_keys());
        hook.fail_mouse_install();
        let captured = hook.capture();
        assert!(capturing.load(Ordering::SeqCst), "mouse failure ended the recorder");
        assert!(hook.is_installed());
        callback(&hook.shared, crate::keymap::VK_F1 + 5, true);
        assert_eq!(captured.recv_timeout(Duration::from_secs(2)), Ok("f6".parse().unwrap()));
        assert!(!capturing.load(Ordering::SeqCst));
    }

    #[test]
    fn regression_mouse_install_failure_through_recording_keeps_escape_active() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let hook = HookController::new(tx, Arc::default());
        {
            let mut logic = hook.shared.logic.lock();
            logic.configure_esc(true, Duration::from_millis(400));
            logic.set_toggle_modifiers("left_shift".parse::<Hotkey>().unwrap().modifier_keys());
        }
        hook.fail_mouse_install();
        assert!(hook.set_recording(true).is_ok(), "mouse refresh must not fail ordinary recording");
        assert!(hook.is_installed());
        callback(&hook.shared, crate::keymap::VK_ESCAPE, true);
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Cancel));
        callback(&hook.shared, crate::keymap::VK_ESCAPE, false);
    }

    #[test]
    fn regression_failed_mouse_post_does_not_strand_next_refresh() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let hook = HookController::new(tx, Arc::default());
        hook.configure(Some("right_shift".parse().unwrap()),
            "left_shift".parse::<Hotkey>().unwrap().modifier_keys(), false,
            Duration::from_millis(400), false, false).unwrap();
        assert!(hook.modifier_toggle_available());
        hook.shared.fail_mouse_post.store(true, Ordering::SeqCst);
        assert!(hook.set_recording(true).is_ok(), "a mouse refresh post must not fail the keyboard jobs");
        assert!(!hook.modifier_toggle_available(), "a failed refresh must fail closed");
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        // A detached test worker lets a broken implementation report a failure
        // instead of hanging the entire test runner on an unbounded recv.
        std::thread::spawn(move || {
            let _ = done_tx.send(hook.set_recording(true));
        });
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(3)), Ok(Ok(())),
            "a stale request from the failed post consumed the next wakeup");
    }

    #[test]
    fn regression_optional_mouse_hook_failure_keeps_keyboard_features() {
        let mut mouse = None;
        update_mouse_hook(&mut mouse, false, || -> Result<i32, &str> { panic!("PTT/recording/recorder must not install mouse") }, |_| panic!("nothing to remove")).unwrap();
        assert_eq!(mouse, None);
        assert_eq!(update_mouse_hook(&mut mouse, true, || Err("mouse unavailable"), |_| panic!("nothing installed")), Err("mouse unavailable"));
        assert_eq!(mouse, None);
        update_mouse_hook(&mut mouse, true, || Ok::<_, &str>(2), |_| panic!("keep new hook")).unwrap();
        assert_eq!(mouse, Some(2));
        update_mouse_hook(&mut mouse, true, || -> Result<i32, &str> { panic!("already installed") }, |_| panic!("keep hook")).unwrap();
        let removed = std::cell::Cell::new(None);
        update_mouse_hook(&mut mouse, false, || -> Result<i32, &str> { panic!("no toggle") }, |hook| removed.set(Some(hook))).unwrap();
        assert_eq!(removed.get(), Some(2));
        assert_eq!(mouse, None);
    }

    #[test]
    fn regression_mouse_hook_tracks_toggle_binding_on_existing_keyboard_thread() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let hook = HookController::new(tx, Arc::default());
        hook.set_recording(true).unwrap();
        let thread_id = hook.shared.thread.lock().as_ref().unwrap().thread_id;
        assert!(!hook.shared.logic.lock().pointer_available());
        hook.configure(None, "left_shift".parse::<Hotkey>().unwrap().modifier_keys(), false, Duration::from_millis(400), false, false).unwrap();
        assert_eq!(hook.shared.thread.lock().as_ref().unwrap().thread_id, thread_id);
        assert!(hook.shared.logic.lock().pointer_available());
        hook.configure(Some("right_shift".parse().unwrap()), None, false, Duration::from_millis(400), false, false).unwrap();
        assert_eq!(hook.shared.thread.lock().as_ref().unwrap().thread_id, thread_id);
        assert!(!hook.shared.logic.lock().pointer_available());
    }

    #[test]
    fn the_hook_is_installed_only_while_needed() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let hook = HookController::new(tx, Arc::default());
        assert!(!hook.is_installed());
        hook.set_recording(true).unwrap();
        assert!(hook.is_installed());
        hook.set_recording(false).unwrap();
        assert!(!hook.is_installed());
        hook.configure(Some("right_shift".parse().unwrap()), None, false, Duration::from_millis(400), false, false).unwrap();
        assert!(hook.is_installed());
        hook.configure(None, None, false, Duration::from_millis(400), false, false).unwrap();
        assert!(!hook.is_installed());
    }

    #[test]
    fn a_ptt_with_fn_does_not_install_the_hook() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let hook = HookController::new(tx, Arc::default());
        hook.configure(Some("fn".parse().unwrap()), None, false, Duration::from_millis(400), false, false).unwrap();
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
        hook.configure(Some("right_option".parse().unwrap()), None, false, Duration::from_millis(400), false, false).unwrap();
        let before = INJECTED_SEEN.load(Ordering::SeqCst);
        assert!(window.is_in_front());
        let right_alt = keymap::VK_RMENU;
        send(&[Stroke::Key { vk: right_alt, down: true }, Stroke::Key { vk: right_alt, down: false }]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while INJECTED_SEEN.load(Ordering::SeqCst) < before + 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        hook.configure(None, None, false, Duration::from_millis(400), false, false).unwrap();
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
