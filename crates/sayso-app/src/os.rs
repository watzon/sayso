//! The platform crate of this OS, behind one set of names.
//!
//! The rest of the app uses `crate::os::window` for window glue and
//! `NativePlatform` for the services. Each platform crate's `window` module
//! has the same functions, in Cocoa-style screen coordinates: origin at the
//! bottom left of the primary display, y up, in logical pixels.

use std::path::Path;

#[cfg(target_os = "macos")]
pub use sayso_platform_macos::{MacPlatform as NativePlatform, window};
#[cfg(windows)]
pub use sayso_platform_windows::{WinPlatform as NativePlatform, window};

/// Show a file in Finder.
#[cfg(target_os = "macos")]
pub fn reveal(path: &Path) {
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
}

/// Open a file or folder with its default app.
#[cfg(target_os = "macos")]
pub fn open_path(path: &Path) {
    let _ = std::process::Command::new("open").arg(path).spawn();
}

/// Open a web address in the default browser.
#[cfg(target_os = "macos")]
pub fn open_url(url: &str) {
    let _ = std::process::Command::new("open").arg(url).spawn();
}

/// Only one Sayso runs at a time. macOS does this for an app bundle.
#[cfg(target_os = "macos")]
pub fn claim_single_instance(_paths: &sayso_core::paths::Paths, _on_second_launch: impl Fn() + Send + 'static) -> bool {
    true
}

/// Show a file in File Explorer.
#[cfg(windows)]
pub fn reveal(path: &Path) {
    window::reveal_in_explorer(path);
}

/// Open a file or folder with its default app.
#[cfg(windows)]
pub fn open_path(path: &Path) {
    window::shell_open(path.as_os_str());
}

/// Open a web address in the default browser.
#[cfg(windows)]
pub fn open_url(url: &str) {
    window::shell_open(std::ffi::OsStr::new(url));
}

/// Only one Sayso runs at a time for the same data folder. Returns false
/// when another one already runs: it was told to open its Hub, and this
/// process should exit. `on_second_launch` runs on a helper thread each time
/// a later launch asks.
#[cfg(windows)]
pub fn claim_single_instance(paths: &sayso_core::paths::Paths, on_second_launch: impl Fn() + Send + 'static) -> bool {
    let scope = match paths.data_source {
        sayso_core::paths::PathSource::PlatformDefault => String::new(),
        _ => paths.data_dir.display().to_string(),
    };
    window::claim_single_instance(&scope, on_second_launch)
}

// ---------------------------------------------------------------------------
// Words for the UI
// ---------------------------------------------------------------------------

/// What the UI calls this computer: "your Mac", "this PC".
pub const COMPUTER: &str = if cfg!(target_os = "macos") {
    "Mac"
} else if cfg!(windows) {
    "PC"
} else {
    "computer"
};

pub const OS_NAME: &str = if cfg!(target_os = "macos") {
    "macOS"
} else if cfg!(windows) {
    "Windows"
} else {
    "Linux"
};

/// The app where the user changes system settings.
pub const SETTINGS_APP: &str = if cfg!(target_os = "macos") { "System Settings" } else { "Settings" };

/// Where Sayso keeps API keys, with its article: "the Keychain".
pub const SECRET_STORE: &str = if cfg!(target_os = "macos") {
    "the Keychain"
} else if cfg!(windows) {
    "Windows Credential Manager"
} else {
    "the system keyring"
};

/// The settings page of the sound input.
pub const SOUND_SETTINGS: &str = if cfg!(target_os = "macos") {
    "System Settings › Sound"
} else if cfg!(windows) {
    "Settings › System › Sound"
} else {
    "the sound settings"
};

/// The model status while it loads for the first time. On macOS Core ML
/// compiles it for the Neural Engine.
pub const OPTIMIZING: &str = if cfg!(target_os = "macos") { "Optimizing for your Mac" } else { "Loading the model" };

/// macOS asks the user for Accessibility (to paste) and Input Monitoring
/// (for push to talk). Other platforms have no such grants, so the UI hides them.
pub const HAS_INPUT_PERMISSIONS: bool = cfg!(target_os = "macos");
