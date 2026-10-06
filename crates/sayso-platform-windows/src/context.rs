//! [`ContextProvider`]: frontmost app, running apps, installed apps, app
//! icons, and the address of the page in a browser.
//!
//! Windows has no bundle ids. An app is known by the lowercase file name of
//! its exe (for example `code.exe`), and that is its [`AppInfo::bundle_id`].
//! The name is the exe's "File description", or the file name without
//! `.exe` when there is none.
//!
//! Store apps run inside `ApplicationFrameHost.exe`. Their real process owns
//! a child window of the frame, so the frame is looked through.
//!
//! The page address comes from the address bar of the browser, read with UI
//! Automation. The walk goes through the browser's own controls in the order
//! of the window and stops at the address bar. It never enters the web page.

use image::ImageEncoder;
use parking_lot::Mutex;
use sayso_platform::{AppInfo, ContextProvider};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, HWND, LPARAM, RPC_E_CHANGED_MODE};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    GetDIBits, GetObjectW, HBITMAP, HGDIOBJ,
};
use windows::Win32::Security::{GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenIntegrityLevel};
use windows::Win32::Storage::FileSystem::{FILE_FLAGS_AND_ATTRIBUTES, GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationCacheRequest, IUIAutomationElement,
    IUIAutomationTreeWalker, IUIAutomationValuePattern, UIA_AutomationIdPropertyId, UIA_ButtonControlTypeId,
    UIA_ControlTypePropertyId, UIA_DocumentControlTypeId, UIA_EditControlTypeId, UIA_ImageControlTypeId,
    UIA_MenuBarControlTypeId, UIA_MenuItemControlTypeId, UIA_TabControlTypeId, UIA_TextControlTypeId,
    UIA_ValuePatternId,
};
use windows::Win32::UI::Shell::{SHDefExtractIconW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, EnumChildWindows, EnumWindows, GW_OWNER, GWL_EXSTYLE, GetForegroundWindow, GetIconInfo, GetWindow,
    GetWindowLongW, GetWindowTextLengthW, GetWindowThreadProcessId, HICON, ICONINFO, IsWindowVisible,
    PrivateExtractIconsW, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, Interface, PCWSTR, PWSTR};

use crate::win32::{OwnedHandle, expand_env, from_wide, read_string, subkey_names, wide};

/// The host process of Store app windows.
const FRAME_HOST: &str = "applicationframehost.exe";

#[derive(Default)]
pub struct WinContext {
    /// Exe paths seen in `frontmost_app` and `running_apps`, by app id.
    paths: Mutex<HashMap<String, PathBuf>>,
}

/// The app id of an exe: its lowercase file name.
pub fn app_id(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// The name to show when the exe has no description: the file name without `.exe`.
pub fn fallback_name(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Processes and windows
// ---------------------------------------------------------------------------

pub(crate) fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: plain query with an out-pointer to a local.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn open_process(pid: u32) -> windows::core::Result<OwnedHandle> {
    // SAFETY: plain call; the handle is owned by the guard.
    unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.map(OwnedHandle)
}

/// Full path of the exe of a process.
pub(crate) fn process_path(pid: u32) -> Option<PathBuf> {
    let process = open_process(pid).ok()?;
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` units, and the call writes at most that many.
    unsafe { QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) }.ok()?;
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

/// Every process as (pid, lowercase exe file name), from a Toolhelp snapshot.
/// Unlike [`process_path`], this needs no access to the process.
pub(crate) fn processes() -> Vec<(u32, String)> {
    // SAFETY: the snapshot handle is owned by the guard; the entry has its size set.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map(OwnedHandle) else {
            return Vec::new();
        };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut out = Vec::new();
        let mut more = Process32FirstW(snapshot.0, &mut entry).is_ok();
        while more {
            out.push((entry.th32ProcessID, from_wide(&entry.szExeFile).to_lowercase()));
            more = Process32NextW(snapshot.0, &mut entry).is_ok();
        }
        out
    }
}

/// Lowercase exe names of every running process, for hotkey conflict hints.
/// Launchers such as PowerToys Run have no visible window, so
/// [`running_apps`] would miss them.
pub fn process_exe_names() -> Vec<String> {
    let mut names: Vec<String> = processes().into_iter().map(|(_, name)| name).collect();
    names.sort();
    names.dedup();
    names
}

/// For a Store app frame: the process of the child window that belongs to
/// another process. That is the real app.
fn frame_child_pid(frame: HWND, host_pid: u32) -> Option<u32> {
    struct Search {
        host: u32,
        found: Option<u32>,
    }
    unsafe extern "system" fn visit(child: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `Search` below, alive for the whole enumeration.
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        let pid = window_pid(child);
        if pid != 0 && pid != search.host {
            search.found = Some(pid);
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut search = Search { host: host_pid, found: None };
    // SAFETY: the callback only touches `search`, which outlives the call.
    unsafe {
        let _ = EnumChildWindows(Some(frame), Some(visit), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}

/// The process behind a top-level window, looking through Store app frames.
pub(crate) fn window_process(hwnd: HWND) -> Option<(u32, PathBuf)> {
    let pid = window_pid(hwnd);
    if pid == 0 {
        return None;
    }
    let path = process_path(pid)?;
    if app_id(&path) == FRAME_HOST
        && let Some(child) = frame_child_pid(hwnd, pid)
        && let Some(child_path) = process_path(child)
    {
        return Some((child, child_path));
    }
    Some((pid, path))
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// The language and code page pairs to look up in a version resource: the
/// ones the file lists, then US English in Unicode and in Windows-1252.
pub fn version_lookups(translations: &[(u16, u16)]) -> Vec<String> {
    let mut keys: Vec<String> = translations.iter().map(|(lang, cp)| format!("{lang:04x}{cp:04x}")).collect();
    for fallback in ["040904b0", "040904e4"] {
        if !keys.iter().any(|k| k == fallback) {
            keys.push(fallback.to_string());
        }
    }
    keys
}

/// The "File description" from the version resource of an exe.
fn file_description(path: &Path) -> Option<String> {
    let file = wide(&path.to_string_lossy());
    // SAFETY: the buffer is as large as Windows asked for, and every pointer
    // VerQueryValueW returns points into it while it lives.
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(file.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(file.as_ptr()), None, size, data.as_mut_ptr().cast()).ok()?;
        let query = |sub: &str| -> Option<(*const u8, u32)> {
            let sub = wide(sub);
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            VerQueryValueW(data.as_ptr().cast(), PCWSTR(sub.as_ptr()), &mut ptr, &mut len)
                .as_bool()
                .then_some((ptr as *const u8, len))
                .filter(|(p, l)| !p.is_null() && *l > 0)
        };
        let translations: Vec<(u16, u16)> = query(r"\VarFileInfo\Translation")
            .map(|(ptr, len)| {
                let units = std::slice::from_raw_parts(ptr.cast::<u16>(), len as usize / 2);
                units.as_chunks::<2>().0.iter().map(|[lang, cp]| (*lang, *cp)).collect()
            })
            .unwrap_or_default();
        version_lookups(&translations).into_iter().find_map(|key| {
            let (ptr, len) = query(&format!(r"\StringFileInfo\{key}\FileDescription"))?;
            // For strings, `len` counts UTF-16 units.
            let text = from_wide(std::slice::from_raw_parts(ptr.cast::<u16>(), len as usize));
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        })
    }
}

static NAMES: LazyLock<Mutex<HashMap<PathBuf, String>>> = LazyLock::new(Mutex::default);

/// The display name of an exe. Cached, because the version resource is read from disk.
pub fn display_name(path: &Path) -> String {
    if let Some(name) = NAMES.lock().get(path) {
        return name.clone();
    }
    let name = file_description(path).unwrap_or_else(|| fallback_name(path));
    NAMES.lock().insert(path.to_path_buf(), name.clone());
    name
}

/// Display name of the process with this PID.
pub fn app_name_for_pid(pid: u32) -> Option<String> {
    process_path(pid).map(|path| display_name(&path))
}

fn app_info(pid: u32, path: &Path) -> AppInfo {
    AppInfo { bundle_id: app_id(path), name: display_name(path), pid: pid as i32 }
}

// ---------------------------------------------------------------------------
// Frontmost and running apps
// ---------------------------------------------------------------------------

fn frontmost() -> Option<(AppInfo, PathBuf)> {
    // SAFETY: plain query.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return None;
    }
    let (pid, path) = window_process(hwnd)?;
    Some((app_info(pid, &path), path))
}

pub fn frontmost_app() -> Option<AppInfo> {
    frontmost().map(|(info, _)| info)
}

/// Whether a top-level window is one the user sees as an app window: visible,
/// not owned by another window, not a tool window, not cloaked (a hidden
/// Store app or a window on another virtual desktop), and with a title.
fn is_app_window(hwnd: HWND) -> bool {
    // SAFETY: plain queries on a window handle from EnumWindows.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
            return false;
        }
        if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
            return false;
        }
        let mut cloaked = 0u32;
        let cloak = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4);
        if cloak.is_ok() && cloaked != 0 {
            return false;
        }
        GetWindowTextLengthW(hwnd) > 0
    }
}

fn top_level_windows() -> Vec<HWND> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the vector below, alive for the whole enumeration.
        unsafe { (*(lparam.0 as *mut Vec<HWND>)).push(hwnd) };
        BOOL(1)
    }
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes into `windows`, which outlives the call.
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut windows as *mut Vec<HWND> as isize));
    }
    windows
}

/// Apps with a visible window, one entry per exe.
fn running() -> Vec<(AppInfo, PathBuf)> {
    let mut out: Vec<(AppInfo, PathBuf)> = Vec::new();
    for hwnd in top_level_windows().into_iter().filter(|h| is_app_window(*h)) {
        let Some((pid, path)) = window_process(hwnd) else { continue };
        let id = app_id(&path);
        if !out.iter().any(|(info, _)| info.bundle_id == id) {
            out.push((app_info(pid, &path), path));
        }
    }
    out
}

pub fn running_apps() -> Vec<AppInfo> {
    running().into_iter().map(|(info, _)| info).collect()
}

/// The exe path of a running process with this app id.
fn find_process_path(app: &str) -> Option<PathBuf> {
    processes().into_iter().filter(|(_, name)| name == app).find_map(|(pid, _)| process_path(pid))
}

// ---------------------------------------------------------------------------
// Installed apps
// ---------------------------------------------------------------------------

/// The registry key where an installer registers an exe by its file name.
const APP_PATHS: &str = r"Software\Microsoft\Windows\CurrentVersion\App Paths";

/// The exe path in the default value of an App Paths key, without quotes and
/// without the text after a quoted path. `None` when nothing is left.
pub fn clean_app_path(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let path = match raw.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or_default(),
        None => raw,
    };
    let path = path.trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// The exe files that App Paths lists, for the current user and then for
/// the computer. Entries whose file does not exist are skipped.
fn app_path_exes() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for name in subkey_names(root, APP_PATHS) {
            let raw = read_string(root, &format!(r"{APP_PATHS}\{name}"), "");
            let Some(path) = raw.and_then(|raw| clean_app_path(&raw)) else { continue };
            // Windows expands a `REG_EXPAND_SZ` value on read. Some installers write `%VAR%` into a plain string.
            let path = if path.contains('%') { expand_env(&path) } else { path };
            let path = PathBuf::from(path);
            if path.is_file() {
                found.push(path);
            }
        }
    }
    found
}

/// Remove duplicate ids (the first one wins) and sort by lowercase name.
/// Apps with an empty id or an empty name are dropped.
pub fn dedup_and_sort(apps: Vec<AppInfo>) -> Vec<AppInfo> {
    let mut seen = HashSet::new();
    let mut apps: Vec<AppInfo> = apps
        .into_iter()
        .filter(|a| !a.bundle_id.is_empty() && !a.name.is_empty() && seen.insert(a.bundle_id.clone()))
        .collect();
    apps.sort_by_cached_key(|a| a.name.to_lowercase());
    apps
}

/// The apps in App Paths plus the apps that run now, each with its exe path.
/// The id is the file name of the exe the entry points to, as for a running
/// app. The key name can be an alias (`pbrush.exe` starts `mspaint.exe`).
fn installed() -> Vec<(AppInfo, PathBuf)> {
    let mut apps: Vec<(AppInfo, PathBuf)> =
        app_path_exes().into_iter().map(|path| (app_info(0, &path), path)).collect();
    // Store apps and apps with no App Paths entry show up when they run.
    apps.extend(running().into_iter().map(|(info, path)| (AppInfo { pid: 0, ..info }, path)));
    apps
}

pub fn installed_apps() -> Vec<AppInfo> {
    dedup_and_sort(installed().into_iter().map(|(info, _)| info).collect())
}

// ---------------------------------------------------------------------------
// Page address
// ---------------------------------------------------------------------------

/// How a browser shows its address bar to UI Automation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserKind {
    /// The address bar is the first Edit control of the window. Its name is
    /// translated, so the name is not used.
    Chromium,
    /// The address bar is the Edit control with the id `urlbar-input`.
    Firefox,
}

/// The id Firefox gives its address bar.
const FIREFOX_ADDRESS_BAR: &str = "urlbar-input";

/// The browsers whose address bar `page_url` reads, by app id.
pub fn browser_kind(bundle_id: &str) -> Option<BrowserKind> {
    match bundle_id.to_lowercase().as_str() {
        "chrome.exe" | "msedge.exe" | "brave.exe" | "vivaldi.exe" | "opera.exe" | "arc.exe" | "chromium.exe"
        | "thorium.exe" => Some(BrowserKind::Chromium),
        "firefox.exe" | "zen.exe" | "librewolf.exe" | "waterfox.exe" => Some(BrowserKind::Firefox),
        _ => None,
    }
}

/// What the walk does with one control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkStep {
    /// This is the address bar.
    Take,
    /// Go on with the controls inside this one.
    Descend,
    /// Go on with the next control and leave the inside of this one alone.
    Skip,
}

/// Decide the step for a control from its UI Automation control type and id.
///
/// A Document is a web page and is never entered. A Tab is the tab strip,
/// which can hold hundreds of tabs. The other skipped types have nothing
/// inside that can be the address bar.
pub fn walk_step(kind: BrowserKind, control_type: i32, automation_id: &str) -> WalkStep {
    const SKIPPED: [i32; 8] = [
        UIA_DocumentControlTypeId.0,
        UIA_TabControlTypeId.0,
        UIA_EditControlTypeId.0,
        UIA_ButtonControlTypeId.0,
        UIA_MenuBarControlTypeId.0,
        UIA_MenuItemControlTypeId.0,
        UIA_TextControlTypeId.0,
        UIA_ImageControlTypeId.0,
    ];
    if control_type == UIA_EditControlTypeId.0 {
        let is_address_bar = match kind {
            BrowserKind::Chromium => true,
            BrowserKind::Firefox => automation_id == FIREFOX_ADDRESS_BAR,
        };
        if is_address_bar {
            return WalkStep::Take;
        }
    }
    if SKIPPED.contains(&control_type) { WalkStep::Skip } else { WalkStep::Descend }
}

/// The text of the address bar as a page address: trimmed, and only when it
/// is not empty and has no white space. Text with a space is a search the
/// user types. The scheme can be missing (`mail.google.com/mail`).
pub fn clean_address(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && !value.contains(char::is_whitespace)).then(|| value.to_string())
}

/// The longest time one UI Automation call waits for the browser.
const UIA_TIMEOUT_MS: u32 = 250;
/// The time after which the walk starts no new call.
const WALK_BUDGET: Duration = Duration::from_millis(250);
/// The most controls the walk looks at.
const WALK_MAX_CONTROLS: usize = 300;

/// COM on the calling thread, for as long as this lives.
struct ComInit {
    /// False when the thread already ran COM in the other mode. COM works
    /// then, and the count is not ours to give back.
    uninit: bool,
}

impl ComInit {
    fn new() -> Option<Self> {
        // SAFETY: plain call. Each success (also "already initialized") is
        // paired with CoUninitialize in `drop`.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_ok() {
            Some(Self { uninit: true })
        } else if hr == RPC_E_CHANGED_MODE {
            Some(Self { uninit: false })
        } else {
            None
        }
    }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        if self.uninit {
            // SAFETY: pairs with the CoInitializeEx that succeeded on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// Walks the controls of one window in the order of the window.
struct ControlWalk {
    walker: IUIAutomationTreeWalker,
    /// Asks for the control type (and for Firefox the id) with each control,
    /// so that reading them needs no more calls to the browser.
    cache: IUIAutomationCacheRequest,
}

impl ControlWalk {
    fn first_child(&self, of: &IUIAutomationElement) -> Option<IUIAutomationElement> {
        // SAFETY: COM call on live interfaces. No child and a timeout are both an error here.
        unsafe { self.walker.GetFirstChildElementBuildCache(of, &self.cache) }.ok()
    }

    fn next_sibling(&self, of: &IUIAutomationElement) -> Option<IUIAutomationElement> {
        // SAFETY: as in `first_child`.
        unsafe { self.walker.GetNextSiblingElementBuildCache(of, &self.cache) }.ok()
    }

    /// The address bar under `root`. Goes depth first, so it never touches a
    /// control that comes after the address bar (the web page does).
    fn find_address_bar(
        &self,
        root: &IUIAutomationElement,
        kind: BrowserKind,
        deadline: Instant,
    ) -> Option<IUIAutomationElement> {
        // The controls whose inside is being walked, outermost first.
        let mut open: Vec<IUIAutomationElement> = Vec::new();
        let mut current = self.first_child(root);
        let mut seen = 0usize;
        loop {
            if Instant::now() >= deadline {
                return None;
            }
            let Some(element) = current else {
                // This level is done. Go on after the control that holds it.
                current = self.next_sibling(&open.pop()?);
                continue;
            };
            seen += 1;
            if seen > WALK_MAX_CONTROLS {
                return None;
            }
            // SAFETY: reads from the cache that came with the element.
            let control_type = unsafe { element.CachedControlType() }.map(|t| t.0).unwrap_or_default();
            let id = match kind {
                // SAFETY: as above. The id is in the cache for Firefox only.
                BrowserKind::Firefox => {
                    unsafe { element.CachedAutomationId() }.map(|s| s.to_string()).unwrap_or_default()
                }
                BrowserKind::Chromium => String::new(),
            };
            match walk_step(kind, control_type, &id) {
                WalkStep::Take => return Some(element),
                WalkStep::Descend => {
                    current = self.first_child(&element);
                    open.push(element);
                }
                WalkStep::Skip => current = self.next_sibling(&element),
            }
        }
    }
}

/// The text in the address bar of a browser window. Waits for the browser
/// for about [`WALK_BUDGET`] at most, plus one call that is under way.
fn address_bar_text(hwnd: HWND, kind: BrowserKind) -> Option<String> {
    // Declared first, so every COM interface below is released before COM is.
    let _com = ComInit::new()?;
    let deadline = Instant::now() + WALK_BUDGET;
    // SAFETY: COM calls on this thread, where COM is initialized. Every
    // interface is owned by a local and released when it drops.
    let (walk, root) = unsafe {
        let automation: IUIAutomation = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).ok()?;
        // Without these, a browser that hangs holds a call for many seconds.
        if let Ok(automation) = automation.cast::<IUIAutomation2>() {
            let _ = automation.SetConnectionTimeout(UIA_TIMEOUT_MS);
            let _ = automation.SetTransactionTimeout(UIA_TIMEOUT_MS);
        }
        let cache = automation.CreateCacheRequest().ok()?;
        cache.AddProperty(UIA_ControlTypePropertyId).ok()?;
        if kind == BrowserKind::Firefox {
            cache.AddProperty(UIA_AutomationIdPropertyId).ok()?;
        }
        let walker = automation.ControlViewWalker().ok()?;
        (ControlWalk { walker, cache }, automation.ElementFromHandle(hwnd).ok()?)
    };
    let bar = walk.find_address_bar(&root, kind, deadline)?;
    // SAFETY: COM calls on a live element. The value is read now, from the browser.
    let value = unsafe {
        bar.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId).ok()?.CurrentValue().ok()?
    };
    Some(value.to_string())
}

/// The address in the address bar of `app`, when `app` is a known browser
/// and owns the foreground window. Every other app costs no system call.
pub fn page_url(app: &AppInfo) -> Option<String> {
    let kind = browser_kind(&app.bundle_id)?;
    // SAFETY: plain query.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() || app.pid <= 0 || window_pid(hwnd) != app.pid as u32 {
        return None;
    }
    clean_address(&address_bar_text(hwnd, kind)?)
}

// ---------------------------------------------------------------------------
// Integrity level
// ---------------------------------------------------------------------------

/// The integrity level of a process, as Windows uses it to keep programs at
/// a lower level from sending keys to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Integrity {
    /// The level RID, for example `0x2000` (medium) or `0x3000` (high).
    Level(u32),
    /// Windows refused to show the level. A process of the same user at the
    /// same level never refuses, so it runs at a higher level.
    Hidden,
    /// The process is gone or could not be checked.
    Unknown,
}

fn token_integrity(process: windows::Win32::Foundation::HANDLE) -> Integrity {
    let mut token = windows::Win32::Foundation::HANDLE::default();
    // SAFETY: out-pointer to a local; the token is owned by the guard below.
    if let Err(err) = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } {
        return if err.code() == ERROR_ACCESS_DENIED.to_hresult() { Integrity::Hidden } else { Integrity::Unknown };
    }
    let token = OwnedHandle(token);
    let mut buf = vec![0u8; 256];
    let mut len = 0u32;
    // SAFETY: the buffer is large enough for a label with one SID (Windows
    // says so with an error otherwise), and the SID points into the buffer.
    unsafe {
        if GetTokenInformation(token.0, TokenIntegrityLevel, Some(buf.as_mut_ptr().cast()), buf.len() as u32, &mut len)
            .is_err()
        {
            return Integrity::Unknown;
        }
        let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = label.Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return Integrity::Unknown;
        }
        Integrity::Level(*GetSidSubAuthority(sid, u32::from(count) - 1))
    }
}

/// Integrity level of the process with this PID.
pub fn process_integrity(pid: u32) -> Integrity {
    match open_process(pid) {
        Ok(process) => token_integrity(process.0),
        Err(err) if err.code() == ERROR_ACCESS_DENIED.to_hresult() => Integrity::Hidden,
        Err(_) => Integrity::Unknown,
    }
}

/// Integrity level of Sayso itself.
pub fn own_integrity() -> Integrity {
    // SAFETY: the pseudo handle of the current process needs no closing.
    token_integrity(unsafe { GetCurrentProcess() })
}

// ---------------------------------------------------------------------------
// Icons
// ---------------------------------------------------------------------------

/// Turn 32-bit BGRA rows (as `GetDIBits` gives them) into RGBA.
///
/// Old icons have no alpha channel: every alpha byte is 0. Then `mask`, the
/// icon's AND mask in the same layout, says which pixels are transparent
/// (white in the mask) and which are opaque (black).
pub fn bgra_to_rgba(color: &[u8], mask: Option<&[u8]>) -> Vec<u8> {
    let pixels = color.as_chunks::<4>().0;
    let has_alpha = pixels.iter().any(|px| px[3] != 0);
    let mut out = Vec::with_capacity(color.len());
    for (i, px) in pixels.iter().enumerate() {
        let alpha = if has_alpha {
            px[3]
        } else {
            match mask.and_then(|m| m.get(i * 4..i * 4 + 3)) {
                Some(m) if m.iter().any(|b| *b != 0) => 0,
                _ => 255,
            }
        };
        out.extend_from_slice(&[px[2], px[1], px[0], alpha]);
    }
    out
}

/// RGBA for a monochrome icon: its mask bitmap is twice as high, with the AND
/// mask on top and the XOR mask below (both as 32-bit rows). AND 0 means
/// opaque in the XOR color. AND 1 with XOR 1 inverts the screen, which a PNG
/// cannot show, so it becomes opaque black. AND 1 with XOR 0 is transparent.
pub fn monochrome_to_rgba(and: &[u8], xor: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(and.len());
    for (a, x) in and.as_chunks::<4>().0.iter().zip(xor.as_chunks::<4>().0) {
        let and_set = a[..3].iter().any(|b| *b != 0);
        let xor_set = x[..3].iter().any(|b| *b != 0);
        out.extend_from_slice(match (and_set, xor_set) {
            (false, true) => &[255, 255, 255, 255],
            (false, false) => &[0, 0, 0, 255],
            (true, true) => &[0, 0, 0, 255],
            (true, false) => &[0, 0, 0, 0],
        });
    }
    out
}

/// Deletes a GDI bitmap when dropped.
struct Bitmap(HBITMAP);

impl Drop for Bitmap {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: GetIconInfo gave us these bitmaps to delete.
            let _ = unsafe { DeleteObject(HGDIOBJ(self.0.0)) };
        }
    }
}

/// Size of a bitmap in pixels.
fn bitmap_size(bitmap: HBITMAP) -> Option<(i32, i32)> {
    let mut bm = BITMAP::default();
    // SAFETY: `bm` is a BITMAP and the size says so.
    let got = unsafe { GetObjectW(HGDIOBJ(bitmap.0), std::mem::size_of::<BITMAP>() as i32, Some((&mut bm as *mut BITMAP).cast())) };
    (got != 0 && bm.bmWidth > 0 && bm.bmHeight > 0).then_some((bm.bmWidth, bm.bmHeight))
}

/// The pixels of a bitmap as 32-bit BGRA rows, top row first.
fn bitmap_pixels(bitmap: HBITMAP, width: i32, height: i32) -> Option<Vec<u8>> {
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Negative height: top-down rows.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    // SAFETY: the buffer holds `height` rows of `width` 32-bit pixels, which is
    // what the header asks for. The DC is created and deleted here.
    unsafe {
        let dc = CreateCompatibleDC(None);
        let lines = GetDIBits(dc, bitmap, 0, height as u32, Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        let _ = DeleteDC(dc);
        (lines == height).then_some(pixels)
    }
}

/// The image of an icon as (width, height, RGBA).
fn icon_rgba(icon: HICON) -> Option<(u32, u32, Vec<u8>)> {
    let mut info = ICONINFO::default();
    // SAFETY: out-pointer to a local. We own the two bitmaps it returns.
    unsafe { GetIconInfo(icon, &mut info) }.ok()?;
    let (color, mask) = (Bitmap(info.hbmColor), Bitmap(info.hbmMask));
    if !color.0.is_invalid() {
        let (w, h) = bitmap_size(color.0)?;
        let pixels = bitmap_pixels(color.0, w, h)?;
        let mask_pixels = bitmap_pixels(mask.0, w, h);
        Some((w as u32, h as u32, bgra_to_rgba(&pixels, mask_pixels.as_deref())))
    } else {
        // A monochrome icon: AND and XOR masks stacked in one bitmap.
        let (w, h2) = bitmap_size(mask.0)?;
        let h = h2 / 2;
        let pixels = bitmap_pixels(mask.0, w, h2)?;
        let (and, xor) = pixels.split_at(w as usize * h as usize * 4);
        Some((w as u32, h as u32, monochrome_to_rgba(and, xor)))
    }
}

/// Load the icon of an exe at about `size` pixels. The caller destroys it.
fn load_icon(path: &Path, size: u32) -> Option<HICON> {
    let file = wide(&path.to_string_lossy());
    // PrivateExtractIconsW scales the best icon in the file to the exact size.
    if file.len() <= 260 {
        let mut name = [0u16; 260];
        name[..file.len()].copy_from_slice(&file);
        let mut icons = [HICON::default()];
        // SAFETY: `name` is a NUL-terminated MAX_PATH buffer; one icon slot.
        let n = unsafe { PrivateExtractIconsW(&name, 0, size as i32, size as i32, Some(&mut icons), None, 0) };
        if n != 0 && n != u32::MAX && !icons[0].is_invalid() {
            return Some(icons[0]);
        }
    }
    let mut large = HICON::default();
    // SAFETY: out-pointer to a local. LOWORD of the size is the large icon size.
    let hr = unsafe { SHDefExtractIconW(PCWSTR(file.as_ptr()), 0, 0, Some(&mut large), None, size & 0xFFFF) };
    if hr.is_ok() && !large.is_invalid() {
        return Some(large);
    }
    // The shell's icon for the file: a generic one when the exe has none.
    let mut sfi = SHFILEINFOW::default();
    // SAFETY: `sfi` is a SHFILEINFOW and the size says so.
    let ok = unsafe {
        SHGetFileInfoW(
            PCWSTR(file.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut sfi),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    (ok != 0 && !sfi.hIcon.is_invalid()).then_some(sfi.hIcon)
}

/// Encode RGBA pixels as PNG, scaled to `size` by `size` when needed.
pub fn encode_png(width: u32, height: u32, rgba: Vec<u8>, size: u32) -> Option<Vec<u8>> {
    let mut image = image::RgbaImage::from_raw(width, height, rgba)?;
    if (width, height) != (size, size) {
        image = image::imageops::resize(&image, size, size, image::imageops::FilterType::Lanczos3);
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(image.as_raw(), size, size, image::ExtendedColorType::Rgba8)
        .ok()?;
    Some(png)
}

/// The icon of an exe as PNG bytes, `size` by `size` pixels.
pub fn icon_png(path: &Path, size: u32) -> Option<Vec<u8>> {
    let size = size.clamp(1, 256);
    let icon = load_icon(path, size)?;
    let rgba = icon_rgba(icon);
    // SAFETY: we own the icon from `load_icon`.
    let _ = unsafe { DestroyIcon(icon) };
    let (w, h, pixels) = rgba?;
    encode_png(w, h, pixels, size)
}

impl WinContext {
    fn remember(&self, app: &AppInfo, path: &Path) {
        self.paths.lock().insert(app.bundle_id.clone(), path.to_path_buf());
    }
}

impl ContextProvider for WinContext {
    fn frontmost_app(&self) -> Option<AppInfo> {
        let (info, path) = frontmost()?;
        self.remember(&info, &path);
        Some(info)
    }

    fn running_apps(&self) -> Vec<AppInfo> {
        running()
            .into_iter()
            .map(|(info, path)| {
                self.remember(&info, &path);
                info
            })
            .collect()
    }

    fn app_icon_png(&self, bundle_id: &str, size: u32) -> Option<Vec<u8>> {
        let id = bundle_id.to_lowercase();
        let known = self.paths.lock().get(&id).cloned();
        let path = known.or_else(|| find_process_path(&id))?;
        let png = icon_png(&path, size);
        if png.is_some() {
            self.paths.lock().insert(id, path);
        }
        png
    }

    fn installed_apps(&self) -> Vec<AppInfo> {
        let apps = installed();
        // Keep the paths, so `app_icon_png` works for an app that does not run.
        let mut paths = self.paths.lock();
        for (info, path) in &apps {
            paths.entry(info.bundle_id.clone()).or_insert_with(|| path.clone());
        }
        drop(paths);
        dedup_and_sort(apps.into_iter().map(|(info, _)| info).collect())
    }

    fn reads_page_url(&self) -> bool {
        true
    }

    fn page_url(&self, app: &AppInfo) -> Option<String> {
        page_url(app)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_MAGIC: [u8; 4] = [0x89, b'P', b'N', b'G'];

    #[test]
    fn app_ids_are_lowercase_exe_names() {
        assert_eq!(app_id(Path::new(r"C:\Program Files\Microsoft VS Code\Code.exe")), "code.exe");
        assert_eq!(fallback_name(Path::new(r"C:\Tools\MyTool.exe")), "MyTool");
    }

    fn info(bundle_id: &str, name: &str) -> AppInfo {
        AppInfo { bundle_id: bundle_id.into(), name: name.into(), pid: 0 }
    }

    #[test]
    fn app_paths_lose_quotes_and_arguments() {
        assert_eq!(clean_app_path(r"C:\Apps\a.exe").as_deref(), Some(r"C:\Apps\a.exe"));
        assert_eq!(clean_app_path(r#" "C:\Program Files\A\a.exe" "#).as_deref(), Some(r"C:\Program Files\A\a.exe"));
        assert_eq!(clean_app_path(r#""C:\Program Files\A\a.exe" --flag"#).as_deref(), Some(r"C:\Program Files\A\a.exe"));
        assert_eq!(clean_app_path(r"%ProgramFiles%\A\a.exe").as_deref(), Some(r"%ProgramFiles%\A\a.exe"));
        assert_eq!(clean_app_path(""), None);
        assert_eq!(clean_app_path(r#" "" "#), None);
    }

    #[test]
    fn dedup_keeps_first_drops_empty_and_sorts_by_lowercase_name() {
        let apps = vec![
            info("zed.exe", "zed"),
            info("code.exe", "Visual Studio Code"),
            info("", "No id"),
            info("noname.exe", ""),
            info("code.exe", "Code again"),
            info("chrome.exe", "Google Chrome"),
        ];
        let out = dedup_and_sort(apps);
        let names: Vec<&str> = out.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Google Chrome", "Visual Studio Code", "zed"]);
    }

    #[test]
    fn only_known_browsers_have_a_kind() {
        assert_eq!(browser_kind("chrome.exe"), Some(BrowserKind::Chromium));
        assert_eq!(browser_kind("MSEdge.exe"), Some(BrowserKind::Chromium));
        assert_eq!(browser_kind("firefox.exe"), Some(BrowserKind::Firefox));
        assert_eq!(browser_kind("zen.exe"), Some(BrowserKind::Firefox));
        for other in ["code.exe", "explorer.exe", "chrome", "notchrome.exe", ""] {
            assert_eq!(browser_kind(other), None, "{other}");
        }
    }

    #[test]
    fn the_walk_takes_the_address_bar_and_stays_out_of_pages() {
        use BrowserKind::{Chromium, Firefox};
        let edit = UIA_EditControlTypeId.0;
        assert_eq!(walk_step(Chromium, edit, ""), WalkStep::Take);
        assert_eq!(walk_step(Firefox, edit, "urlbar-input"), WalkStep::Take);
        assert_eq!(walk_step(Firefox, edit, ""), WalkStep::Skip);
        assert_eq!(walk_step(Firefox, edit, "searchbar"), WalkStep::Skip);
        for kind in [Chromium, Firefox] {
            assert_eq!(walk_step(kind, UIA_DocumentControlTypeId.0, ""), WalkStep::Skip);
            assert_eq!(walk_step(kind, UIA_TabControlTypeId.0, ""), WalkStep::Skip);
            assert_eq!(walk_step(kind, UIA_ButtonControlTypeId.0, ""), WalkStep::Skip);
            // Pane, ToolBar, and Group hold the address bar.
            for container in [50033, 50021, 50026] {
                assert_eq!(walk_step(kind, container, ""), WalkStep::Descend);
            }
        }
    }

    #[test]
    fn addresses_have_no_white_space() {
        assert_eq!(clean_address(" mail.google.com/mail/u/0/#inbox ").as_deref(), Some("mail.google.com/mail/u/0/#inbox"));
        assert_eq!(clean_address("https://example.com/a?b=c").as_deref(), Some("https://example.com/a?b=c"));
        assert_eq!(clean_address("how to bake bread"), None);
        assert_eq!(clean_address("two\tparts"), None);
        assert_eq!(clean_address("   "), None);
        assert_eq!(clean_address(""), None);
    }

    #[test]
    fn other_apps_have_no_page_address() {
        let app = AppInfo { bundle_id: "explorer.exe".into(), name: "Windows Explorer".into(), pid: std::process::id() as i32 };
        assert_eq!(page_url(&app), None);
    }

    #[test]
    fn installed_apps_are_unique_and_sorted() {
        let apps = installed_apps();
        let ids: HashSet<&str> = apps.iter().map(|a| a.bundle_id.as_str()).collect();
        assert_eq!(ids.len(), apps.len());
        assert!(apps.iter().all(|a| !a.name.is_empty() && a.pid == 0));
        assert!(apps.is_sorted_by_key(|a| a.name.to_lowercase()));
    }

    #[test]
    fn version_lookups_try_the_listed_languages_first() {
        assert_eq!(version_lookups(&[]), ["040904b0", "040904e4"]);
        assert_eq!(version_lookups(&[(0x0407, 0x04b0)]), ["040704b0", "040904b0", "040904e4"]);
        assert_eq!(version_lookups(&[(0x0409, 0x04b0)]), ["040904b0", "040904e4"]);
    }

    #[test]
    fn alpha_icons_keep_their_alpha() {
        // Two pixels in BGRA: half-transparent red, opaque blue.
        let color = [0, 0, 255, 128, 255, 0, 0, 255];
        assert_eq!(bgra_to_rgba(&color, None), [255, 0, 0, 128, 0, 0, 255, 255]);
    }

    #[test]
    fn icons_without_alpha_use_their_mask() {
        let color = [10, 20, 30, 0, 40, 50, 60, 0];
        // First pixel transparent (white in the mask), second opaque.
        let mask = [255, 255, 255, 0, 0, 0, 0, 0];
        assert_eq!(bgra_to_rgba(&color, Some(&mask)), [30, 20, 10, 0, 60, 50, 40, 255]);
        // No mask at all: opaque.
        assert_eq!(bgra_to_rgba(&color, None), [30, 20, 10, 255, 60, 50, 40, 255]);
    }

    #[test]
    fn monochrome_icons_map_their_four_cases() {
        let white = [255u8, 255, 255, 0];
        let black = [0u8, 0, 0, 0];
        let and = [black, black, white, white].concat();
        let xor = [white, black, white, black].concat();
        assert_eq!(
            monochrome_to_rgba(&and, &xor),
            [[255, 255, 255, 255], [0, 0, 0, 255], [0, 0, 0, 255], [0, 0, 0, 0]].concat()
        );
    }

    #[test]
    fn png_encoding_scales_to_the_requested_size() {
        let png = encode_png(2, 2, vec![255; 16], 4).unwrap();
        assert!(png.starts_with(&PNG_MAGIC));
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (4, 4));
    }

    #[test]
    fn extracts_the_explorer_icon() {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let explorer = Path::new(&windir).join("explorer.exe");
        for size in [16, 32, 64] {
            let png = icon_png(&explorer, size).expect("explorer.exe has an icon");
            assert!(png.starts_with(&PNG_MAGIC));
            let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap().to_rgba8();
            assert_eq!(decoded.dimensions(), (size, size));
            assert!(decoded.pixels().any(|p| p[3] == 255), "some pixels are opaque");
            assert!(decoded.pixels().any(|p| p[3] == 0), "the corners are transparent");
        }
    }

    #[test]
    fn explorer_has_a_display_name() {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let name = display_name(&Path::new(&windir).join("explorer.exe"));
        assert_eq!(name, "Windows Explorer");
    }

    #[test]
    fn this_process_is_found_by_pid_and_in_the_process_list() {
        let pid = std::process::id();
        let path = process_path(pid).unwrap();
        let id = app_id(&path);
        assert!(id.ends_with(".exe"));
        assert!(processes().iter().any(|(p, name)| *p == pid && *name == id));
        assert!(process_exe_names().contains(&id));
        assert!(matches!(process_integrity(pid), Integrity::Level(_)));
        assert_eq!(process_integrity(pid), own_integrity());
    }

    #[test]
    fn running_apps_are_unique_by_exe() {
        let apps = running_apps();
        let mut ids: Vec<&str> = apps.iter().map(|a| a.bundle_id.as_str()).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n);
        assert!(apps.iter().all(|a| !a.name.is_empty() && a.bundle_id.ends_with(".exe")));
    }
}
