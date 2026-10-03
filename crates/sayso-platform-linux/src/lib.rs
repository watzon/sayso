//! Linux implementations of the `sayso-platform` traits, for X11 and Wayland.
//!
//! Call [`session::prepare`] first thing in `main`, then build everything with
//! [`LinuxPlatform::new`]. The `window` and `tray` modules hold the glue the
//! UI calls directly.
//!
//! The crate compiles to nothing on other systems.

#![cfg(target_os = "linux")]

pub mod context;
pub mod hotkeys;
pub mod inserter;
pub mod ipc;
pub mod launch;
pub mod login_item;
pub mod permissions;
pub mod prefs;
pub mod session;
pub mod tray;
pub mod window;

// The portable modules, under the same paths as in the macOS crate.
pub use sayso_platform_common::{audio, esc, paste_receipt, resample, sounds};

use sayso_platform::Platform;
use std::path::PathBuf;
use std::sync::Arc;

/// Entry point of this crate.
pub struct LinuxPlatform;

impl LinuxPlatform {
    /// Build every Linux service. Call it on the main thread after
    /// [`session::prepare`]. `sounds_dir` holds `start.wav`, `stop.wav`,
    /// `cancel.wav`, and `insert.wav`.
    #[allow(clippy::new_ret_no_self)] // The constructor returns the trait bundle on purpose.
    pub fn new(sounds_dir: PathBuf) -> Platform {
        let session = session::current();
        let hotkeys = hotkeys::LinuxHotkeys::new(session);
        ipc::forward_hotkeys(hotkeys.sender());
        Platform {
            hotkeys: Box::new(hotkeys),
            inserter: Arc::new(inserter::LinuxInserter::new(session)),
            context: Arc::new(context::LinuxContext::new(session)),
            permissions: Arc::new(permissions::LinuxPermissions::new(session)),
            audio: Arc::new(audio::CpalAudio::new(audio::always_granted)),
            sounds: Arc::new(sounds::RodioSounds::new(sounds_dir)),
            login_item: Arc::new(login_item::LinuxLoginItem),
            prefs: Arc::new(prefs::LinuxPrefs),
        }
    }
}
