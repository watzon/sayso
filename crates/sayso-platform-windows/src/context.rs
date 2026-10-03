//! [`ContextProvider`]: frontmost app, running apps, and app icons.
//!
//! Windows has no bundle ids. An app is known by the lowercase file name of
//! its exe (for example `code.exe`), and that is its [`AppInfo::bundle_id`].
//! The name is the exe's "File description", or the file name without
//! `.exe` when there is none.
//!
//! Store apps run inside `ApplicationFrameHost.exe`. Their real process owns
//! a child window of the frame, so the frame is looked through.

use image::ImageEncoder;
use parking_lot::Mutex;
use sayso_platform::{AppInfo, ContextProvider};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, HWND, LPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    GetDIBits, GetObjectW, HBITMAP, HGDIOBJ,
};
use windows::Win32::Security::{GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenIntegrityLevel};
use windows::Win32::Storage::FileSystem::{FILE_FLAGS_AND_ATTRIBUTES, GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::Shell::{SHDefExtractIconW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGetFileInfoW};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, EnumChildWindows, EnumWindows, GW_OWNER, GWL_EXSTYLE, GetForegroundWindow, GetIconInfo, GetWindow,
    GetWindowLongW, GetWindowTextLengthW, GetWindowThreadProcessId, HICON, ICONINFO, IsWindowVisible,
    PrivateExtractIconsW, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PCWSTR, PWSTR};

use crate::win32::{OwnedHandle, from_wide, wide};

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
