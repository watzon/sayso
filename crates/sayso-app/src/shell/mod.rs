//! The app and window glue that GPUI does not cover, one backend per system.
//!
//! Every backend has the same items, so the rest of the app calls
//! `crate::shell::…` and never names a platform crate:
//!
//! - `hand_off`, `prepare`, `platform`: startup, before logging, before GPUI, and after GPUI starts.
//! - `NativeWindow`, `native`: the system's handle of a GPUI window.
//! - App: `become_accessory`, `set_dock_icon_visible`, `show_app_and_activate`, `deactivate_app`.
//! - Windows: `configure_overlay`, `make_never_key`, `set_frame_origin`, `set_frame`, `frame`,
//!   `make_clear`, `add_child_window`, `order_front_without_activating`, `order_out`, `show_and_focus`.
//! - Overlay: `overlay_kind`, `set_click_region`, `can_place_windows`.
//! - Screens and mouse: `screens`, `primary_screen_height`, `mouse_location`, `mouse_button_down`,
//!   `frontmost_app_is_fullscreen`, `focused_window_frame`, `install_global_click_monitor`, `MonitorToken`.
//! - Tray: `Tray` (with [`TrayEvent`]).
//! - Files and system: `open`, `reveal`, `audio_player`, `user_full_name`, `model_runs_here`,
//!   `open_requested` (a second start of Sayso asked to open the Hub). Windows plays
//!   audio in its own process, so it has `play_wav` and `stop_wav` and no `audio_player`.
//!
//! Coordinates are global screen points with the origin at the bottom left of
//! the primary display and y up (the Cocoa convention). A backend for a
//! top-left system converts.
//!
//! `fallback` is a backend that does nothing. It keeps the app compiling on a
//! system that has no backend yet.

/// The app id of the windows (the X11 window class and the Wayland app id on
/// Linux). The desktop entry `dev.sayso.Sayso.desktop` names it, so the dock
/// shows the Sayso icon. It is also the bundle id on macOS.
pub const APP_ID: &str = "dev.sayso.Sayso";

/// UI text with a macOS version and a version for the other systems:
/// `os_text!("on this Mac", "on this computer")`. With three versions, the
/// second is for Windows: `os_text!("on this Mac", "on this PC", "on this computer")`.
/// All arms must have the same type. Use it wherever text names the Mac,
/// macOS, or its settings.
macro_rules! os_text {
    ($mac:expr, $other:expr $(,)?) => {
        if cfg!(target_os = "macos") { $mac } else { $other }
    };
    ($mac:expr, $windows:expr, $other:expr $(,)?) => {
        if cfg!(target_os = "macos") {
            $mac
        } else if cfg!(windows) {
            $windows
        } else {
            $other
        }
    };
}
pub(crate) use os_text;

/// What the UI calls this computer: "your Mac", "this PC".
pub const COMPUTER: &str = os_text!("Mac", "PC", "computer");

pub const OS_NAME: &str = os_text!("macOS", "Windows", "Linux");

/// Where Sayso keeps API keys, with its article: "the Keychain".
pub const SECRET_STORE: &str = os_text!("the Keychain", "Windows Credential Manager", "the system keyring");

/// The settings page of the sound input.
pub const SOUND_SETTINGS: &str = os_text!("System Settings › Sound", "Settings › System › Sound", "the sound settings");

/// The model status while it loads for the first time. On macOS Core ML
/// compiles it for the Neural Engine.
pub const OPTIMIZING: &str = os_text!("Optimizing for your Mac", "Loading the model", "Preparing the model");

/// macOS asks the user for Accessibility (to paste) and Input Monitoring
/// (for push to talk), and Linux has its own grants for both. Windows has no
/// such grants, so the UI hides them there.
pub const HAS_INPUT_PERMISSIONS: bool = !cfg!(windows);

/// The names of the permissions in the UI. macOS uses its own names.
pub fn permission_name(permission: sayso_platform::Permission) -> &'static str {
    use sayso_platform::Permission::*;
    match permission {
        Microphone => "Microphone",
        Accessibility => os_text!("Accessibility", "Paste access"),
        InputMonitoring => os_text!("Input Monitoring", "Keyboard access"),
    }
}

/// The button that leads to the place where the user allows a permission.
pub const OPEN_PERMISSION_SETTINGS: &str = os_text!("Open System Settings", "Open Settings", "Open the setup guide");

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod fallback;
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub use fallback::*;

/// Gray out a tray icon image (RGBA): the sign that incognito is on.
#[cfg(any(target_os = "macos", target_os = "linux", windows))]
fn dim_icon(rgba: &mut [u8]) {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel[3] = (f32::from(pixel[3]) * 0.4) as u8;
    }
}

/// What happened at the menu bar or tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))] // The macOS and Windows icons have no menu, so they only send `Click`.
pub enum TrayEvent {
    /// A primary click: toggle the popover.
    Click,
    /// "Start or Stop Dictation" from the icon's own menu (systems with a native menu).
    ToggleDictation,
    /// "Open Sayso" from the icon's own menu.
    OpenHub,
    /// "Quit Sayso" from the icon's own menu.
    Quit,
}
