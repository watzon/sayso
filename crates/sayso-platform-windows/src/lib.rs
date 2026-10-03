//! Windows implementations of the `sayso-platform` traits.
//!
//! Build everything with [`WinPlatform::new`].
//!
//! The crate is empty on other platforms, so `cargo test --workspace` works there.
#![cfg(windows)]

pub mod audio;
pub mod conflicts;
pub mod context;
pub mod esc;
pub mod hook;
pub mod hook_logic;
pub mod hotkeys;
pub mod input;
pub mod inserter;
pub mod keymap;
pub mod login_item;
pub mod paste_receipt;
pub mod permissions;
pub mod prefs;
pub mod resample;
pub mod sounds;
#[cfg(test)]
mod test_window;
pub mod win32;

use sayso_platform::Platform;
use std::path::PathBuf;
use std::sync::Arc;

/// Entry point of this crate.
pub struct WinPlatform;

impl WinPlatform {
    /// Build every Windows service. Call it on the main thread, the one that
    /// runs the message loop: hotkey registration later needs that thread.
    /// `sounds_dir` holds `start.wav`, `stop.wav`, `cancel.wav`, and `insert.wav`.
    #[allow(clippy::new_ret_no_self)] // The constructor returns the trait bundle on purpose.
    pub fn new(sounds_dir: PathBuf) -> Platform {
        Platform {
            hotkeys: Box::new(hotkeys::WinHotkeys::new()),
            inserter: Arc::new(inserter::WinInserter),
            context: Arc::new(context::WinContext::default()),
            permissions: Arc::new(permissions::WinPermissions),
            audio: Arc::new(audio::WinAudio),
            sounds: Arc::new(sounds::WinSounds::new(sounds_dir)),
            login_item: Arc::new(login_item::WinLoginItem),
            prefs: Arc::new(prefs::WinPrefs),
        }
    }
}
