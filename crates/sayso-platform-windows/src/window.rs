//! Win32 helpers for the UI. The same functions as the macOS crate's
//! `window` module, so the app calls one API on both platforms.
//!
//! Window functions take the HWND from the `raw-window-handle` Win32 handle,
//! as a raw pointer. A null pointer or a handle that is no longer a window
//! makes the call do nothing.
//!
//! Coordinates are Cocoa-style, as on macOS: origin at the bottom left of the
//! primary display, y up. The unit is GPUI's logical pixel on Windows: a
//! physical pixel divided by the scale of the display it is on. On displays
//! with different scales this space has gaps, the same as GPUI's own window
//! bounds, so window positions from GPUI and from here agree.

use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, POINT, RECT, WAIT_OBJECT_0};
use windows::Win32::Graphics::Dwm::{
    DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_NCRENDERING_POLICY, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_DONOTROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MONITORINFOEXW,
    MonitorFromWindow,
};
use windows::Win32::System::Threading::{
    AttachThreadInput, CreateEventW, CreateMutexW, GetCurrentProcessId, GetCurrentThreadId, INFINITE, OpenEventW,
    SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GWL_EXSTYLE, GWLP_HWNDPARENT, GetClassNameW, GetCursorPos, GetForegroundWindow, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, HWND_TOPMOST, IsWindow, MONITORINFOF_PRIMARY, SM_SWAPBUTTON, SW_HIDE, SW_SHOW,
    SW_SHOWNOACTIVATE, SW_SHOWNORMAL, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_EX_APPWINDOW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};
use windows::core::{BOOL, PCWSTR};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenInfo {
    /// A hash of the display's device name (`\\.\DISPLAY1`). Stable while the
    /// display stays on the same connector.
    pub id: u32,
    pub frame: Rect,
    /// The frame minus the taskbar.
    pub visible_frame: Rect,
    /// Display scale (1.5 at 144 DPI).
    pub scale: f64,
    /// Always false: Windows displays have no notch.
    pub has_notch: bool,
}

/// One display in physical pixels, with its scale.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Monitor {
    id: u32,
    frame: RECT,
    work: RECT,
    scale: f64,
    primary: bool,
}

impl Monitor {
    fn contains_physical(&self, x: i32, y: i32) -> bool {
        x >= self.frame.left && x < self.frame.right && y >= self.frame.top && y < self.frame.bottom
    }

    /// A physical rectangle as a logical one with a top-left origin.
    fn logical(&self, r: RECT) -> (f64, f64, f64, f64) {
        let s = self.scale;
        (f64::from(r.left) / s, f64::from(r.top) / s, f64::from(r.right - r.left) / s, f64::from(r.bottom - r.top) / s)
    }
}

// ---------------------------------------------------------------------------
// Coordinates
// ---------------------------------------------------------------------------

/// Logical top-left based (x, top, width, height) to a Cocoa rectangle.
fn cocoa(primary_height: f64, (x, top, width, height): (f64, f64, f64, f64)) -> Rect {
    Rect { x, y: primary_height - top - height, width, height }
}

fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `Vec` passed to `EnumDisplayMonitors` below, alive for the call.
        let list = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
        if let Some(m) = monitor_info(monitor) {
            list.push(m);
        }
        BOOL(1)
    }
    let mut list: Vec<Monitor> = Vec::new();
    // SAFETY: the callback only runs during this call, while `list` is alive.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut list as *mut Vec<Monitor> as isize));
    }
    // Primary first, as on macOS.
    list.sort_by_key(|m| !m.primary);
    list
}

fn monitor_info(monitor: HMONITOR) -> Option<Monitor> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: `info` is a MONITORINFOEXW with its size set.
    unsafe { GetMonitorInfoW(monitor, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO).ok().ok()? };
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    // SAFETY: plain query with out-pointers to locals.
    let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    let name_len = info.szDevice.iter().position(|c| *c == 0).unwrap_or(info.szDevice.len());
    Some(Monitor {
        id: fnv1a(&info.szDevice[..name_len]),
        frame: info.monitorInfo.rcMonitor,
        work: info.monitorInfo.rcWork,
        scale: f64::from(dpi_x.max(1)) / 96.0,
        primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
    })
}

/// FNV-1a over UTF-16 units: a stable id from a display name.
fn fnv1a(units: &[u16]) -> u32 {
    units.iter().fold(0x811c_9dc5u32, |hash, unit| {
        let hash = (hash ^ u32::from(*unit & 0xff)).wrapping_mul(0x0100_0193);
        (hash ^ u32::from(*unit >> 8)).wrapping_mul(0x0100_0193)
    })
}

fn primary_height_of(monitors: &[Monitor]) -> f64 {
    monitors.iter().find(|m| m.primary).or(monitors.first()).map_or(1080.0, |m| m.logical(m.frame).3)
}

/// The display that holds a logical top-left based point, else the primary.
fn monitor_at_logical(monitors: &[Monitor], x: f64, top: f64) -> Option<Monitor> {
    monitors
        .iter()
        .find(|m| {
            let (mx, my, mw, mh) = m.logical(m.frame);
            x >= mx && x < mx + mw && top >= my && top < my + mh
        })
        .or(monitors.first())
        .copied()
}

/// A rectangle in physical pixels with a top-left origin (what tray-icon and
/// most Win32 calls report) in Cocoa coordinates.
pub fn rect_from_physical(x: f64, y: f64, width: f64, height: f64) -> Rect {
    let list = monitors();
    let h = primary_height_of(&list);
    let scale = list
        .iter()
        .find(|m| m.contains_physical((x + width / 2.0) as i32, (y + height / 2.0) as i32))
        .or(list.first())
        .map_or(1.0, |m| m.scale);
    cocoa(h, (x / scale, y / scale, width / scale, height / scale))
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

/// macOS hides the Dock icon here. A Windows tray app has nothing to hide:
/// only open windows show in the taskbar.
pub fn become_accessory() -> bool {
    true
}

/// The Hub always has a taskbar button on Windows, so this does nothing.
pub fn set_dock_icon_visible(_visible: bool) -> bool {
    true
}

/// GPUI activates the window it opens. Nothing else to do on Windows.
pub fn show_app_and_activate() {}

/// Windows activates the window below when the popover hides, which is the
/// app the user came from. Nothing to do.
pub fn deactivate_app() {}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

fn resolve(ptr: *mut c_void) -> Option<HWND> {
    if ptr.is_null() {
        return None;
    }
    let hwnd = HWND(ptr);
    // SAFETY: `IsWindow` accepts any value and says whether it is a window.
    unsafe { IsWindow(Some(hwnd)) }.as_bool().then_some(hwnd)
}

fn ex_style(hwnd: HWND) -> u32 {
    // SAFETY: `hwnd` is a window (checked by `resolve`).
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 }
}

fn set_ex_style(hwnd: HWND, style: u32) {
    if ex_style(hwnd) == style {
        return;
    }
    // SAFETY: `hwnd` is a window. SWP_FRAMECHANGED makes Windows read the new style.
    unsafe {
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style as isize);
        let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    }
}

/// No Windows 11 rounded corners and no one-pixel border: the window draws
/// its own sheet and shadow on a clear background.
fn no_frame(hwnd: HWND) {
    let corner = DWMWCP_DONOTROUND;
    let border = DWMWA_COLOR_NONE;
    // No non-client rendering: no system shadow around the clear window.
    let policy = DWMNCRP_DISABLED;
    // SAFETY: each pointer is to a local of the size given.
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const c_void, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &border as *const _ as *const c_void, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_NCRENDERING_POLICY, &policy as *const _ as *const c_void, 4);
    }
}

/// Make a window behave as a non-activating overlay: always on top, out of
/// the taskbar and Alt+Tab, never activated by a click, and without a frame.
/// Show it with [`order_front_without_activating`].
pub fn configure_overlay(window: *mut c_void) {
    let Some(hwnd) = resolve(window) else { return };
    let style = (ex_style(hwnd) | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0) & !WS_EX_APPWINDOW.0;
    set_ex_style(hwnd, style);
    no_frame(hwnd);
}

/// The overlay can never take the keyboard: [`configure_overlay`] already
/// gave it `WS_EX_NOACTIVATE`, so a click does not activate it either.
pub fn make_never_key(window: *mut c_void) {
    let Some(hwnd) = resolve(window) else { return };
    set_ex_style(hwnd, ex_style(hwnd) | WS_EX_NOACTIVATE.0);
}

/// Let mouse events pass through the window to what is behind it.
///
/// A click goes through a window only when it is both layered and
/// transparent. The layered style has no attributes set, so GPUI's
/// DirectComposition content still draws the window.
pub fn set_ignores_mouse(window: *mut c_void, ignores: bool) {
    let Some(hwnd) = resolve(window) else { return };
    let through = WS_EX_TRANSPARENT.0 | WS_EX_LAYERED.0;
    let style = ex_style(hwnd);
    set_ex_style(hwnd, if ignores { style | through } else { style & !through });
}

/// The window's size in logical pixels, and the scale of its display.
fn logical_size(hwnd: HWND) -> Option<(f64, f64)> {
    let mut r = RECT::default();
    // SAFETY: `hwnd` is a window and `r` an out-pointer.
    unsafe { GetWindowRect(hwnd, &mut r).ok()? };
    let scale = window_scale(hwnd);
    Some((f64::from(r.right - r.left) / scale, f64::from(r.bottom - r.top) / scale))
}

fn window_scale(hwnd: HWND) -> f64 {
    // SAFETY: `hwnd` is a window.
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    monitor_info(monitor).map_or(1.0, |m| m.scale)
}

/// Move and size the window to a Cocoa rectangle. The physical position uses
/// the scale of the display the top-left corner lands on.
fn place(hwnd: HWND, x: f64, y: f64, size: Option<(f64, f64)>) {
    let Some((current_w, current_h)) = logical_size(hwnd) else { return };
    let (w, h) = size.unwrap_or((current_w, current_h));
    let list = monitors();
    let top = primary_height_of(&list) - y - h;
    let scale = monitor_at_logical(&list, x, top).map_or(1.0, |m| m.scale);
    let flags = SWP_NOZORDER | SWP_NOACTIVATE | if size.is_none() { SWP_NOSIZE } else { Default::default() };
    // SAFETY: `hwnd` is a window.
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            (x * scale).round() as i32,
            (top * scale).round() as i32,
            (w * scale).round() as i32,
            (h * scale).round() as i32,
            flags,
        );
    }
}

/// Move the window's bottom-left corner to a point in Cocoa coordinates.
pub fn set_frame_origin(window: *mut c_void, x: f64, y: f64) {
    if let Some(hwnd) = resolve(window) {
        place(hwnd, x, y, None);
    }
}

/// Move and resize the window. `x` and `y` are its bottom-left corner in
/// Cocoa coordinates.
pub fn set_frame(window: *mut c_void, x: f64, y: f64, width: f64, height: f64) {
    if let Some(hwnd) = resolve(window) {
        place(hwnd, x, y, Some((width, height)));
    }
}

/// Make a window that draws its own sheet and shadow fully clear: no frame,
/// no rounded system corners, and no taskbar button.
pub fn make_clear(window: *mut c_void) {
    let Some(hwnd) = resolve(window) else { return };
    set_ex_style(hwnd, (ex_style(hwnd) | WS_EX_TOOLWINDOW.0) & !WS_EX_APPWINDOW.0);
    no_frame(hwnd);
}

/// Make `parent` the owner of `child`: the child stays above it and
/// minimizes with it.
pub fn add_child_window(parent: *mut c_void, child: *mut c_void) {
    let (Some(parent), Some(child)) = (resolve(parent), resolve(child)) else { return };
    // SAFETY: both are windows. GWLP_HWNDPARENT sets the owner of a top-level window.
    unsafe { SetWindowLongPtrW(child, GWLP_HWNDPARENT, parent.0 as isize) };
}

/// Show the window on top without activating it, so keystrokes stay in the
/// app the user is typing in.
pub fn order_front_without_activating(window: *mut c_void) {
    let Some(hwnd) = resolve(window) else { return };
    // SAFETY: `hwnd` is a window.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

/// Hide the window.
pub fn order_out(window: *mut c_void) {
    if let Some(hwnd) = resolve(window) {
        // SAFETY: `hwnd` is a window.
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

/// Show the window and give it the keyboard. For the Hub and the popover,
/// not for the overlay.
///
/// Windows lets only the foreground process take the foreground. Sayso
/// borrows the input state of the current foreground thread for the call,
/// the usual way for a tray app to bring up its own window.
pub fn show_and_focus(window: *mut c_void) {
    let Some(hwnd) = resolve(window) else { return };
    // SAFETY: `hwnd` is a window; the thread input is attached and detached in this block.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let foreground = GetForegroundWindow();
        let this_thread = GetCurrentThreadId();
        let other_thread = if foreground.is_invalid() { 0 } else { GetWindowThreadProcessId(foreground, None) };
        let attach = other_thread != 0 && other_thread != this_thread;
        if attach {
            let _ = AttachThreadInput(this_thread, other_thread, true);
        }
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        if attach {
            let _ = AttachThreadInput(this_thread, other_thread, false);
        }
    }
}

// ---------------------------------------------------------------------------
// Screens and mouse
// ---------------------------------------------------------------------------

/// All connected displays, primary first.
pub fn screens() -> Vec<ScreenInfo> {
    let list = monitors();
    let h = primary_height_of(&list);
    list.iter()
        .map(|m| ScreenInfo {
            id: m.id,
            frame: cocoa(h, m.logical(m.frame)),
            visible_frame: cocoa(h, m.logical(m.work)),
            scale: m.scale,
            has_notch: false,
        })
        .collect()
}

/// The mouse position in Cocoa coordinates.
pub fn mouse_location() -> Point {
    let mut p = POINT::default();
    // SAFETY: out-pointer to a local.
    if unsafe { GetCursorPos(&mut p) }.is_err() {
        return Point { x: 0.0, y: 0.0 };
    }
    let r = rect_from_physical(f64::from(p.x), f64::from(p.y), 0.0, 0.0);
    Point { x: r.x, y: r.y }
}

/// True while the primary mouse button is down (for dragging the pill).
pub fn mouse_button_down() -> bool {
    // SAFETY: plain queries.
    unsafe {
        let key = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { VK_RBUTTON } else { VK_LBUTTON };
        GetAsyncKeyState(i32::from(key.0)) as u16 & 0x8000 != 0
    }
}

/// Height of the primary display, for converting Cocoa (bottom-left) and
/// GPUI (top-left) coordinates.
pub fn primary_screen_height() -> f64 {
    primary_height_of(&monitors())
}

/// The window's content area in Cocoa coordinates. GPUI draws the content
/// into the client area, which on macOS fills the whole frame. A resizable
/// window here also has invisible resize borders outside it.
pub fn frame(window: *mut c_void) -> Option<Rect> {
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
    let hwnd = resolve(window)?;
    let mut c = RECT::default();
    let mut origin = POINT::default();
    // SAFETY: `hwnd` is a window; `c` and `origin` are out-pointers.
    unsafe {
        GetClientRect(hwnd, &mut c).ok()?;
        if !ClientToScreen(hwnd, &mut origin).as_bool() {
            return None;
        }
    }
    let r = RECT { left: origin.x, top: origin.y, right: origin.x + c.right, bottom: origin.y + c.bottom };
    let scale = window_scale(hwnd);
    let h = primary_screen_height();
    Some(cocoa(h, (f64::from(r.left) / scale, f64::from(r.top) / scale, f64::from(r.right - r.left) / scale, f64::from(r.bottom - r.top) / scale)))
}

/// macOS needs a global monitor to see clicks in other apps. On Windows such
/// a click deactivates the popover, and the popover closes on deactivation.
/// So the token is always inactive.
pub struct MonitorToken;

impl MonitorToken {
    pub fn is_active(&self) -> bool {
        false
    }
}

pub fn install_global_click_monitor(_callback: impl Fn(Point) + 'static) -> MonitorToken {
    MonitorToken
}

// ---------------------------------------------------------------------------
// Full-screen detection
// ---------------------------------------------------------------------------

/// Whether `bounds` covers a whole display (all values in the same
/// coordinate space, within one point).
pub fn covers_a_display(bounds: Rect, displays: &[Rect]) -> bool {
    const TOLERANCE: f64 = 1.0;
    displays.iter().any(|d| {
        (bounds.x - d.x).abs() <= TOLERANCE
            && (bounds.y - d.y).abs() <= TOLERANCE
            && (bounds.width - d.width).abs() <= TOLERANCE
            && (bounds.height - d.height).abs() <= TOLERANCE
    })
}

/// Window classes of the desktop and the taskbar. They fill a display without
/// being a full-screen app.
const SHELL_CLASSES: &[&str] = &["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"];

/// Best effort: the foreground window fills a display (a game, a video, a
/// presentation). Sayso's own windows never count.
pub fn frontmost_app_is_fullscreen() -> bool {
    // SAFETY: plain queries on the foreground window; `r` and `class` are out-buffers.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == GetCurrentProcessId() {
            return false;
        }
        let mut class = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut class) as usize;
        let class = String::from_utf16_lossy(&class[..len]);
        if SHELL_CLASSES.contains(&class.as_str()) {
            return false;
        }
        let mut r = RECT::default();
        if GetWindowRect(hwnd, &mut r).is_err() {
            return false;
        }
        let bounds = Rect { x: f64::from(r.left), y: f64::from(r.top), width: f64::from(r.right - r.left), height: f64::from(r.bottom - r.top) };
        let displays: Vec<Rect> = monitors()
            .iter()
            .map(|m| {
                let f = m.frame;
                Rect { x: f64::from(f.left), y: f64::from(f.top), width: f64::from(f.right - f.left), height: f64::from(f.bottom - f.top) }
            })
            .collect();
        covers_a_display(bounds, &displays)
    }
}

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

/// True when the taskbar uses the dark theme (Settings › Personalization ›
/// Colors › "Choose your default Windows mode"). Dark when the value is missing,
/// as on a new Windows 11 install.
pub fn taskbar_is_dark() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let key = wide(OsStr::new(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"));
    let value = wide(OsStr::new("SystemUsesLightTheme"));
    let mut data = 0u32;
    let mut size = 4u32;
    // SAFETY: the names are NUL-terminated and `data` holds the 4 bytes asked for.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    read.is_err() || data == 0
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Open a file, a folder, or a web address with its default program.
pub fn shell_open(target: &OsStr) {
    let target = wide(target);
    let verb = wide(OsStr::new("open"));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        ShellExecuteW(None, PCWSTR(verb.as_ptr()), PCWSTR(target.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Open File Explorer with `path` selected.
pub fn reveal_in_explorer(path: &Path) {
    use std::os::windows::process::CommandExt;
    // `/select,` takes the rest of the command line as the path, unquoted
    // splitting does not apply, so pass it raw.
    let _ = std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
}

/// Play a WAV file without waiting, for History playback. A new call or
/// [`stop_wav`] ends the sound that plays. False when Windows refused it.
pub fn play_wav(path: &Path) -> bool {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME, SND_NODEFAULT};
    let path = wide(path.as_os_str());
    // SAFETY: the path is NUL-terminated; with SND_ASYNC Windows copies what it needs.
    unsafe { PlaySoundW(PCWSTR(path.as_ptr()), None, SND_FILENAME | SND_ASYNC | SND_NODEFAULT) }.as_bool()
}

/// Stop the sound of [`play_wav`].
pub fn stop_wav() {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_FLAGS};
    // SAFETY: a null sound stops the current one.
    unsafe {
        let _ = PlaySoundW(PCWSTR::null(), None, SND_FLAGS(0));
    }
}

/// The user's display name ("Chris Watson"), else the account name.
pub fn user_display_name() -> Option<String> {
    use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, NameDisplay};
    let mut buf = [0u16; 256];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` units; on success `len` is the count written.
    let ok = unsafe { GetUserNameExW(NameDisplay, Some(windows::core::PWSTR(buf.as_mut_ptr())), &mut len) };
    let name = ok.then(|| String::from_utf16_lossy(&buf[..len as usize])).filter(|n| !n.trim().is_empty());
    name.or_else(|| std::env::var("USERNAME").ok().filter(|n| !n.is_empty()))
}

// ---------------------------------------------------------------------------
// Single instance
// ---------------------------------------------------------------------------

const INSTANCE_MUTEX: &str = "Local\\dev.sayso.Sayso.instance";
const SHOW_EVENT: &str = "Local\\dev.sayso.Sayso.show";

/// Make this process the only Sayso of the user session for its data.
///
/// Returns true for the first process. It then waits on a named event, and
/// calls `on_second_launch` on a helper thread each time a later launch sets
/// it. A later process sets the event and gets false: it should exit.
///
/// `scope` is empty for the default data folder, so the installer finds the
/// mutex by its fixed name. A run with other folders (`XDG_DATA_HOME`, for
/// development) passes the folder, and runs next to the installed Sayso.
pub fn claim_single_instance(scope: &str, on_second_launch: impl Fn() + Send + 'static) -> bool {
    let suffix = if scope.is_empty() {
        String::new()
    } else {
        let units: Vec<u16> = scope.to_lowercase().encode_utf16().collect();
        format!(".{:08x}", fnv1a(&units))
    };
    let mutex_name = wide(OsStr::new(&format!("{INSTANCE_MUTEX}{suffix}")));
    let event_name = wide(OsStr::new(&format!("{SHOW_EVENT}{suffix}")));
    // SAFETY: the names are NUL-terminated. The mutex handle stays open for
    // the life of the process on purpose: it marks the running instance.
    let event = unsafe {
        let Ok(_mutex) = CreateMutexW(None, false, PCWSTR(mutex_name.as_ptr())) else { return true };
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(event_name.as_ptr())) {
                let _ = SetEvent(event);
                let _ = CloseHandle(event);
            }
            return false;
        }
        let Ok(event) = CreateEventW(None, false, false, PCWSTR(event_name.as_ptr())) else { return true };
        event.0 as usize
    };
    let spawned = std::thread::Builder::new().name("sayso-instance".into()).spawn(move || {
        let event = windows::Win32::Foundation::HANDLE(event as *mut c_void);
        // SAFETY: the event handle is never closed while this thread runs.
        while unsafe { WaitForSingleObject(event, INFINITE) } == WAIT_OBJECT_0 {
            on_second_launch();
        }
    });
    if let Err(e) = spawned {
        log::warn!("cannot wait for later launches: {e}");
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: Rect = Rect { x: 0.0, y: 0.0, width: 2560.0, height: 1440.0 };

    #[test]
    fn a_window_equal_to_a_display_is_full_screen() {
        assert!(covers_a_display(MAIN, &[MAIN]));
        let maximized = Rect { x: 0.0, y: 0.0, width: 2560.0, height: 1392.0 };
        assert!(!covers_a_display(maximized, &[MAIN]));
        assert!(!covers_a_display(MAIN, &[]));
    }

    #[test]
    fn cocoa_flips_y_around_the_primary_height() {
        // A 100 x 50 window at the top left of a 1440-high primary display.
        assert_eq!(cocoa(1440.0, (0.0, 0.0, 100.0, 50.0)), Rect { x: 0.0, y: 1390.0, width: 100.0, height: 50.0 });
        // A display above the primary has a negative top and a y above 1440.
        assert_eq!(cocoa(1440.0, (0.0, -1080.0, 1920.0, 1080.0)).y, 1440.0);
    }

    #[test]
    fn logical_rects_divide_by_the_display_scale() {
        let m = Monitor {
            id: 1,
            frame: RECT { left: 0, top: 0, right: 3840, bottom: 2160 },
            work: RECT { left: 0, top: 0, right: 3840, bottom: 2064 },
            scale: 1.5,
            primary: true,
        };
        assert_eq!(m.logical(m.frame), (0.0, 0.0, 2560.0, 1440.0));
        assert_eq!(primary_height_of(&[m]), 1440.0);
        assert_eq!(monitor_at_logical(&[m], 100.0, 100.0), Some(m));
    }

    #[test]
    fn display_ids_are_stable_and_distinct() {
        let a: Vec<u16> = "\\\\.\\DISPLAY1".encode_utf16().collect();
        let b: Vec<u16> = "\\\\.\\DISPLAY2".encode_utf16().collect();
        assert_eq!(fnv1a(&a), fnv1a(&a));
        assert_ne!(fnv1a(&a), fnv1a(&b));
    }

    #[test]
    fn null_and_dead_handles_do_nothing() {
        configure_overlay(std::ptr::null_mut());
        order_out(std::ptr::null_mut());
        set_frame_origin(std::ptr::null_mut(), 0.0, 0.0);
        assert_eq!(frame(std::ptr::null_mut()), None);
        assert_eq!(frame(0x1234 as *mut c_void), None);
    }

    #[test]
    fn this_machine_has_a_primary_display() {
        let screens = screens();
        assert!(!screens.is_empty());
        assert!(screens[0].frame.width > 0.0 && screens[0].scale >= 1.0);
        assert!(primary_screen_height() > 0.0);
    }
}
