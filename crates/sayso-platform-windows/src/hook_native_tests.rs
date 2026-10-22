//! Opt-in installed-hook regressions. Tagged input is consumed by this test
//! adapter even when production would leak it, so no key reaches another app.
//! Only the adapter removes LLKHF_INJECTED for its own tag; production still
//! rejects all injected input. This exercises real LL dispatch and async state,
//! not physical input or a replacement key-state provider.
use super::*;
use crate::keymap::{VK_ESCAPE, VK_RETURN};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{HHOOK, PM_REMOVE};

const TAG: usize = 0x5342_3354;
thread_local! {
    static RESULTS: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "system" fn adapter(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: Windows supplies a keyboard payload for HC_ACTION.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if info.dwExtraInfo == TAG {
            let mut physical = *info;
            physical.flags.0 &= !LLKHF_INJECTED;
            // SAFETY: a live copy of the native payload, forwarded unchanged
            // except for the test-only injected bit, to the real callback.
            let result = unsafe { hook_proc(code, wparam, LPARAM(&physical as *const _ as isize)) };
            RESULTS.with(|r| r.borrow_mut().push(result == LRESULT(1)));
            // Safety net: even a failing regression never sends Enter/Esc to
            // the foreground app. Production's decision is recorded above.
            return LRESULT(1);
        }
    }
    // SAFETY: pass unrelated input through, without observing or altering it.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

struct Installed;
impl Installed {
    fn new(shared: Arc<Shared>) -> (Self, HHOOK) {
        CONTEXT.with(|c| *c.borrow_mut() = Some(shared));
        RESULTS.with(|r| r.borrow_mut().clear());
        // SAFETY: this thread pumps messages until the hook is removed.
        let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(adapter), None, 0) }.unwrap();
        (Self, hook)
    }
}
impl Drop for Installed {
    fn drop(&mut self) {
        CONTEXT.with(|c| c.borrow_mut().take());
    }
}

fn native_event(vk: u16, down: bool) -> bool {
    let before = RESULTS.with(|r| r.borrow().len());
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT {
            wVk: VIRTUAL_KEY(vk), wScan: 0,
            dwFlags: if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP },
            time: 0, dwExtraInfo: TAG,
        } },
    };
    // SAFETY: one well-formed INPUT, consumed by our installed adapter.
    assert_eq!(unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) }, 1);
    let deadline = Instant::now() + Duration::from_secs(2);
    while RESULTS.with(|r| r.borrow().len()) == before && Instant::now() < deadline {
        let mut message = MSG::default();
        // SAFETY: pumps this thread's message queue and LL callbacks.
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            // Ignore thread-only mask/idle messages: this harness owns no
            // production hook thread and must not emit a mask into an app.
            if message.hwnd.0.is_null() { continue; }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    RESULTS.with(|r| {
        let results = r.borrow();
        assert_eq!(results.len(), before + 1, "native hook did not receive tagged input");
        results[before]
    })
}

fn installed_b3(enter: bool) {
    let vk = if enter { VK_RETURN } else { VK_ESCAPE };
    // Do not interfere with a real held key. No window/focus changes occur.
    for key in [vk, 0x7E] {
        assert!(unsafe { GetAsyncKeyState(key.into()) } >= 0, "test key is physically held");
    }
    let (tx, rx) = crossbeam_channel::unbounded();
    let controller = HookController::new(tx, Arc::default());
    {
        let mut logic = controller.shared.logic.lock();
        logic.set_enter_sends(enter);
        logic.set_take_escape(!enter);
        logic.set_recording(true);
    }
    let (_context, hook) = Installed::new(controller.shared.clone());
    // Always remove the native hook, including on assertion failure.
    struct Unhook(HHOOK);
    impl Drop for Unhook {
        fn drop(&mut self) { let _ = unsafe { UnhookWindowsHookEx(self.0) }; }
    }
    let _unhook = Unhook(hook);
    assert!(native_event(vk, true));
    assert!(unsafe { GetAsyncKeyState(vk.into()) } >= 0, "LL-swallowed down must remain absent from async state");
    if enter {
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Submit));
        controller.shared.logic.lock().set_recording(false);
    }
    // A tagged F15 press forces production pruning between held repeats.
    native_event(0x7E, true);
    native_event(0x7E, false);
    let repeated_taken = native_event(vk, true);
    let repeated_events: Vec<_> = rx.try_iter().collect();
    let released_taken = native_event(vk, false);
    assert!(repeated_taken, "held key leaked after native async-state pruning");
    assert!(released_taken, "taken release leaked");
    assert!(repeated_events.is_empty(), "held Esc became a second press: {repeated_events:?}");
    if !enter {
        assert!(native_event(vk, true));
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Cancel));
        assert!(native_event(vk, false));
    }
}

#[test]
#[ignore = "installs native LL hook and sends tagged input consumed by a safety adapter; run alone"]
fn installed_hook_taken_enter_survives_pruning() { installed_b3(true); }

#[test]
#[ignore = "installs native LL hook and sends tagged input consumed by a safety adapter; run alone"]
fn installed_hook_taken_esc_repeat_is_not_double_press() { installed_b3(false); }
