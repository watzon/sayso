//! macOS backend: the AppKit helpers of `sayso-platform-macos`.

use super::TrayEvent;
use gpui_kit::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use sayso_core::models::ModelInfo;
use sayso_platform::Platform;
use sayso_platform_macos::window as mac;
use std::ffi::{OsStr, c_void};
use std::path::{Path, PathBuf};

pub use mac::{MonitorToken, Point, Rect, ScreenInfo};

/// The `NSView` of a GPUI window.
pub type NativeWindow = *mut c_void;

/// The first startup step, before logging starts. macOS starts one copy of
/// the app on its own, so there is nothing to hand off.
pub fn hand_off() {}

/// Startup work before GPUI starts. Nothing on macOS.
pub fn prepare() {}

/// macOS starts one copy of the app on its own, so nobody asks.
pub fn open_requested() -> bool {
    false
}

pub fn platform(sounds_dir: PathBuf) -> Platform {
    sayso_platform_macos::MacPlatform::new(sounds_dir)
}

pub fn native(window: &Window) -> Option<NativeWindow> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr()),
        _ => None,
    }
}

// App -----------------------------------------------------------------------

pub fn become_accessory() {
    mac::become_accessory();
}

pub fn set_dock_icon_visible(visible: bool) {
    mac::set_dock_icon_visible(visible);
}

pub fn show_app_and_activate() {
    mac::show_app_and_activate();
}

pub fn deactivate_app() {
    mac::deactivate_app();
}

// Windows -------------------------------------------------------------------

pub fn configure_overlay(window: NativeWindow) {
    mac::configure_overlay(window);
}

pub fn make_never_key(window: NativeWindow) {
    mac::make_never_key(window);
}

pub fn set_frame_origin(window: NativeWindow, x: f64, y: f64) {
    mac::set_frame_origin(window, x, y);
}

pub fn set_frame(window: NativeWindow, x: f64, y: f64, width: f64, height: f64) {
    mac::set_frame(window, x, y, width, height);
}

pub fn frame(window: NativeWindow) -> Option<Rect> {
    mac::frame(window)
}

pub fn make_clear(window: NativeWindow) {
    mac::make_clear(window);
}

pub fn add_child_window(parent: NativeWindow, child: NativeWindow) {
    mac::add_child_window(parent, child);
}

pub fn order_front_without_activating(window: NativeWindow) {
    mac::order_front_without_activating(window);
}

pub fn order_out(window: NativeWindow) {
    mac::order_out(window);
}

pub fn show_and_focus(window: NativeWindow) {
    mac::show_and_focus(window);
}

// Overlay -------------------------------------------------------------------

/// The window kind of the overlay.
pub fn overlay_kind() -> WindowKind {
    WindowKind::PopUp
}

/// The window kind of the menu bar popover.
pub fn popover_kind() -> WindowKind {
    WindowKind::PopUp
}

/// The window kind of a list that opens from a trigger, when the system
/// places it. None: the app places it (macOS places every window itself).
pub fn popup_kind(_parent: AnyWindowHandle, _anchor: Bounds<Pixels>, _offset: gpui_kit::Point<Pixels>) -> Option<WindowKind> {
    None
}

/// Let the mouse reach the overlay only while it is over the card, so the
/// empty part of the window never blocks the app underneath. macOS switches
/// the whole window, because the mouse position is known everywhere.
pub fn set_click_region(_window: &Window, native: NativeWindow, _card: Bounds<Pixels>, inside: bool) {
    mac::set_ignores_mouse(native, !inside);
}

/// True when the app can move its windows to a screen position.
pub fn can_place_windows() -> bool {
    true
}

/// True when [`order_out`] hides a window and [`show_and_focus`] shows it
/// again. Without it, close a window to hide it.
pub fn can_hide_windows() -> bool {
    true
}

// Screens and mouse ---------------------------------------------------------

pub fn screens() -> Vec<ScreenInfo> {
    mac::screens()
}

pub fn primary_screen_height() -> f64 {
    mac::primary_screen_height()
}

pub fn mouse_location() -> Point {
    mac::mouse_location()
}

pub fn mouse_button_down() -> bool {
    mac::mouse_button_down()
}

pub fn frontmost_app_is_fullscreen() -> bool {
    mac::frontmost_app_is_fullscreen()
}

pub fn install_global_click_monitor(callback: impl Fn(Point) + 'static) -> MonitorToken {
    mac::install_global_click_monitor(callback)
}

// Tray ----------------------------------------------------------------------

/// The menu bar icon. It has no NSMenu: a click shows the popover.
pub struct Tray {
    icon: tray_icon::TrayIcon,
}

impl Tray {
    /// Show the icon. `on_event` runs on any thread.
    pub fn install(on_event: impl Fn(TrayEvent) + Send + Sync + 'static) -> Option<Tray> {
        let mut builder = tray_icon::TrayIconBuilder::new().with_tooltip("Sayso");
        if let Some(icon) = tray_icon_image() {
            builder = builder.with_icon_templated(icon);
        } else {
            builder = builder.with_title("Sayso");
        }
        let icon = match builder.build() {
            Ok(i) => i,
            Err(e) => {
                log::error!("could not create the menu bar icon: {e}");
                return None;
            }
        };
        tray_icon::TrayIconEvent::set_event_handler(Some(move |e: tray_icon::TrayIconEvent| {
            if let tray_icon::TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Down,
                ..
            } = e
            {
                on_event(TrayEvent::Click);
            }
        }));
        Some(Tray { icon })
    }

    /// The icon's screen frame. Read it at click time: right after creation
    /// the frame is still empty.
    pub fn icon_rect(&self) -> Option<Rect> {
        let item = self.icon.ns_status_item()?;
        mac::status_item_screen_rect(objc2::rc::Retained::as_ptr(&item) as *mut c_void)
    }
}

fn tray_icon_image() -> Option<tray_icon::Icon> {
    let bytes = sayso_ui::assets::bytes("app/tray.png")?;
    let img = image::load_from_memory(&bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), w, h).ok()
}

// Files and system ----------------------------------------------------------

/// Open a file, a folder, or a web page with its default app.
pub fn open(target: impl AsRef<OsStr>) {
    let _ = std::process::Command::new("open").arg(target).spawn();
}

/// Show a file in the file manager.
pub fn reveal(path: &Path) {
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
}

/// A command that plays a WAV file given as its last argument.
pub fn audio_player() -> Option<std::process::Command> {
    Some(std::process::Command::new("afplay"))
}

/// The user's full name, for the greeting.
pub fn user_full_name() -> Option<String> {
    let out = std::process::Command::new("id").arg("-F").output().ok()?;
    String::from_utf8(out.stdout).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// True when this Mac's macOS version runs the model.
pub fn model_runs_here(info: &ModelInfo) -> bool {
    info.min_macos <= macos_major()
}

/// The major version of macOS ("15" from "15.6.1"). 14, the oldest macOS Sayso
/// runs on, when the version cannot be read.
fn macos_major() -> u32 {
    static VERSION: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *VERSION.get_or_init(|| {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|v| v.trim().split('.').next().and_then(|major| major.parse().ok()))
            .unwrap_or(14)
    })
}
