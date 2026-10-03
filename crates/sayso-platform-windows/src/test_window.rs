//! A window of the tests' own, so tests that send real keys never send them
//! to the user's apps. Test builds only.

use crate::win32::{from_wide, wide};
use std::thread::sleep;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, ES_MULTILINE, ES_WANTRETURN,
    GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
    SW_SHOW, SendMessageW, SetForegroundWindow, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE,
    WM_DESTROY, WM_GETTEXT, WM_GETTEXTLENGTH, WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows::core::PCWSTR;

unsafe extern "system" fn frame_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CLOSE => {
            // SAFETY: our own window, on its thread.
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: ends the loop of this thread.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// A window with a multi-line EDIT control on a thread with its own message
/// loop, brought to the front.
pub struct TestWindow {
    pub frame: usize,
    pub edit: usize,
    join: Option<std::thread::JoinHandle<()>>,
}

impl TestWindow {
    pub fn open() -> Self {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let join = std::thread::spawn(move || {
            // SAFETY: the windows are created and used on this thread only;
            // other threads only send messages to them.
            unsafe {
                let instance = GetModuleHandleW(None).unwrap();
                let class = wide("SaysoTestFrame");
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(frame_proc),
                    hInstance: instance.into(),
                    lpszClassName: PCWSTR(class.as_ptr()),
                    ..Default::default()
                };
                RegisterClassW(&wc);
                let title = wide("Sayso platform test");
                let frame = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    PCWSTR(class.as_ptr()),
                    PCWSTR(title.as_ptr()),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    100,
                    100,
                    480,
                    240,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
                .unwrap();
                let edit_class = wide("EDIT");
                let edit = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    PCWSTR(edit_class.as_ptr()),
                    PCWSTR::null(),
                    WS_CHILD | WS_VISIBLE | WINDOW_STYLE((ES_MULTILINE | ES_WANTRETURN) as u32),
                    0,
                    0,
                    460,
                    200,
                    Some(frame),
                    None,
                    Some(instance.into()),
                    None,
                )
                .unwrap();
                let _ = ShowWindow(frame, SW_SHOW);
                if !SetForegroundWindow(frame).as_bool() {
                    // Windows lets a thread that shares input with the
                    // foreground thread take the foreground.
                    let fg_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
                    let me = GetCurrentThreadId();
                    if fg_thread != 0 && fg_thread != me {
                        let _ = AttachThreadInput(fg_thread, me, true);
                        let _ = SetForegroundWindow(frame);
                        let _ = BringWindowToTop(frame);
                        let _ = AttachThreadInput(fg_thread, me, false);
                    }
                }
                let _ = SetFocus(Some(edit));
                tx.send((frame.0 as usize, edit.0 as usize)).unwrap();
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        });
        let (frame, edit) = rx.recv().unwrap();
        Self { frame, edit, join: Some(join) }
    }

    pub fn is_in_front(&self) -> bool {
        // SAFETY: plain query.
        unsafe { GetForegroundWindow() }.0 as usize == self.frame
    }

    /// Wait up to 2 s for the window to come to the front. Tests must not
    /// send keys when this is false.
    pub fn wait_in_front(&self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.is_in_front() && Instant::now() < deadline {
            sleep(Duration::from_millis(20));
        }
        self.is_in_front()
    }

    pub fn text(&self) -> String {
        let edit = HWND(self.edit as *mut _);
        // SAFETY: WM_GETTEXT writes at most the given number of units.
        unsafe {
            let len = SendMessageW(edit, WM_GETTEXTLENGTH, None, None).0 as usize;
            let mut buf = vec![0u16; len + 1];
            SendMessageW(edit, WM_GETTEXT, Some(WPARAM(buf.len())), Some(LPARAM(buf.as_mut_ptr() as isize)));
            from_wide(&buf)
        }
    }

    /// Wait until the EDIT control shows `want`.
    pub fn wait_for(&self, want: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let text = self.text();
            if text == want || Instant::now() > deadline {
                return text;
            }
            sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        // SAFETY: posts to our own window.
        let _ = unsafe { PostMessageW(Some(HWND(self.frame as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
