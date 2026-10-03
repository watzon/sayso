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
pub fn claim_single_instance(_on_second_launch: impl Fn() + Send + 'static) -> bool {
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

/// Only one Sayso runs at a time. Returns false when another one already
/// runs: it was told to open its Hub, and this process should exit.
/// `on_second_launch` runs on a helper thread each time a later launch asks.
#[cfg(windows)]
pub fn claim_single_instance(on_second_launch: impl Fn() + Send + 'static) -> bool {
    window::claim_single_instance(on_second_launch)
}
