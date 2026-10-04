//! Windows backend: the Win32 helpers of `sayso-platform-windows`.
//!
//! Instead of `audio_player`, it has `play_wav` and `stop_wav`: `PlaySound`
//! plays in this process.

use super::TrayEvent;
use gpui_kit::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use sayso_core::models::ModelInfo;
use sayso_core::paths::{PathSource, Paths, SystemEnv};
use sayso_platform::Platform;
use sayso_platform_windows::window as win;
use std::ffi::{OsStr, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub use win::{MonitorToken, Point, Rect, ScreenInfo, play_wav, stop_wav};

/// The `HWND` of a GPUI window.
pub type NativeWindow = *mut c_void;

/// A later start of Sayso asked this one to open its Hub.
static OPEN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// The first startup step, before logging starts. Only one Sayso runs for
/// each data folder. A later start tells the running Sayso to open its Hub
/// and exits here, so it never touches that Sayso's log.
pub fn hand_off() {
    let paths = Paths::resolve(&SystemEnv);
    let scope = match paths.data_source {
        PathSource::PlatformDefault => String::new(),
        _ => paths.data_dir.display().to_string(),
    };
    if !win::claim_single_instance(&scope, || OPEN_REQUESTED.store(true, Ordering::SeqCst)) {
        std::process::exit(0);
    }
}

/// Startup work before GPUI starts. Nothing on Windows.
pub fn prepare() {}

/// True once for each start of Sayso that asked the running Sayso to open.
pub fn open_requested() -> bool {
    OPEN_REQUESTED.swap(false, Ordering::SeqCst)
}

pub fn platform(sounds_dir: PathBuf) -> Platform {
    sayso_platform_windows::WinPlatform::new(sounds_dir)
}

pub fn native(window: &Window) -> Option<NativeWindow> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as *mut c_void),
        _ => None,
    }
}

// App -----------------------------------------------------------------------

pub fn become_accessory() {
    win::become_accessory();
}

pub fn set_dock_icon_visible(visible: bool) {
    win::set_dock_icon_visible(visible);
}

pub fn show_app_and_activate() {
    win::show_app_and_activate();
}

pub fn deactivate_app() {
    win::deactivate_app();
}

// Windows -------------------------------------------------------------------

pub fn configure_overlay(window: NativeWindow) {
    win::configure_overlay(window);
}

pub fn make_never_key(window: NativeWindow) {
    win::make_never_key(window);
}

pub fn set_frame_origin(window: NativeWindow, x: f64, y: f64) {
    win::set_frame_origin(window, x, y);
}

pub fn set_frame(window: NativeWindow, x: f64, y: f64, width: f64, height: f64) {
    win::set_frame(window, x, y, width, height);
}

pub fn frame(window: NativeWindow) -> Option<Rect> {
    win::frame(window)
}

pub fn make_clear(window: NativeWindow) {
    win::make_clear(window);
}

pub fn add_child_window(parent: NativeWindow, child: NativeWindow) {
    win::add_child_window(parent, child);
}

pub fn order_front_without_activating(window: NativeWindow) {
    win::order_front_without_activating(window);
}

pub fn order_out(window: NativeWindow) {
    win::order_out(window);
}

pub fn show_and_focus(window: NativeWindow) {
    win::show_and_focus(window);
}

// Overlay -------------------------------------------------------------------

/// The window kind of the overlay.
pub fn overlay_kind() -> WindowKind {
    WindowKind::PopUp
}

/// The window kind of the tray popover.
pub fn popover_kind() -> WindowKind {
    WindowKind::PopUp
}

/// The window kind of a list that opens from a trigger, when the system
/// places it. None: the app places it.
pub fn popup_kind(_parent: AnyWindowHandle, _anchor: Bounds<Pixels>, _offset: gpui_kit::Point<Pixels>) -> Option<WindowKind> {
    None
}

/// Let the mouse reach the overlay only while it is over the card, so the
/// empty part of the window never blocks the app underneath.
pub fn set_click_region(_window: &Window, native: NativeWindow, _card: Bounds<Pixels>, inside: bool) {
    win::set_ignores_mouse(native, !inside);
}

/// True when the app can move its windows to a screen position.
pub fn can_place_windows() -> bool {
    true
}

/// True when [`order_out`] hides a window and [`show_and_focus`] shows it again.
pub fn can_hide_windows() -> bool {
    true
}

// Screens and mouse ---------------------------------------------------------

pub fn screens() -> Vec<ScreenInfo> {
    win::screens()
}

pub fn primary_screen_height() -> f64 {
    win::primary_screen_height()
}

pub fn mouse_location() -> Point {
    win::mouse_location()
}

pub fn mouse_button_down() -> bool {
    win::mouse_button_down()
}

pub fn frontmost_app_is_fullscreen() -> bool {
    win::frontmost_app_is_fullscreen()
}

/// The frame of the window that has the keyboard focus.
pub fn focused_window_frame() -> Option<Rect> {
    win::frontmost_window_frame()
}

pub fn install_global_click_monitor(callback: impl Fn(Point) + 'static) -> MonitorToken {
    win::install_global_click_monitor(callback)
}

// Tray ----------------------------------------------------------------------

/// The icon in the notification area. It has no menu: a click shows the popover.
pub struct Tray {
    icon: tray_icon::TrayIcon,
}

impl Tray {
    /// Show the icon. `on_event` runs on any thread.
    pub fn install(on_event: impl Fn(TrayEvent) + Send + Sync + 'static) -> Option<Tray> {
        let mut builder = tray_icon::TrayIconBuilder::new().with_tooltip("Sayso");
        if let Some(icon) = tray_icon_image(false) {
            builder = builder.with_icon(icon);
        } else {
            builder = builder.with_title("Sayso");
        }
        let icon = match builder.build() {
            Ok(i) => i,
            Err(e) => {
                log::error!("could not create the tray icon: {e}");
                return None;
            }
        };
        // A taskbar icon opens on the release of the button, and a right
        // click opens it too.
        tray_icon::TrayIconEvent::set_event_handler(Some(move |e: tray_icon::TrayIconEvent| {
            if let tray_icon::TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left | tray_icon::MouseButton::Right,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            } = e
            {
                on_event(TrayEvent::Click);
            }
        }));
        Some(Tray { icon })
    }

    /// Gray out the icon while incognito is on.
    pub fn set_incognito(&self, on: bool) {
        if let Err(e) = self.icon.set_icon(tray_icon_image(on)) {
            log::warn!("could not change the tray icon: {e}");
        }
    }

    /// The icon's screen frame. tray-icon gives physical pixels with the
    /// origin at the top left.
    pub fn icon_rect(&self) -> Option<Rect> {
        let r = self.icon.rect()?;
        Some(win::rect_from_physical(r.position.x, r.position.y, f64::from(r.size.width), f64::from(r.size.height)))
    }
}

fn tray_icon_image(incognito: bool) -> Option<tray_icon::Icon> {
    let bytes = sayso_ui::assets::bytes("app/tray.png")?;
    let mut img = image::load_from_memory(&bytes).ok()?.into_rgba8();
    // The icon is a black template. On a dark taskbar it must be white to show.
    if win::taskbar_is_dark() {
        for p in img.pixels_mut() {
            p.0[..3].copy_from_slice(&[255, 255, 255]);
        }
    }
    if incognito {
        super::dim_icon(&mut img);
    }
    let (w, h) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), w, h).ok()
}

// Files and system ----------------------------------------------------------

/// Open a file, a folder, or a web page with its default app.
pub fn open(target: impl AsRef<OsStr>) {
    win::shell_open(target.as_ref());
}

/// Show a file in File Explorer.
pub fn reveal(path: &Path) {
    win::reveal_in_explorer(path);
}

/// The account's display name, for the greeting.
pub fn user_full_name() -> Option<String> {
    win::user_display_name()
}

/// Every model of the Windows catalog runs on every Windows version Sayso supports.
pub fn model_runs_here(_info: &ModelInfo) -> bool {
    true
}
