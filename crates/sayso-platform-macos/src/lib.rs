//! macOS implementations of the `sayso-platform` traits.
//!
//! Build everything with [`MacPlatform::new`]. The `window` module holds the
//! AppKit helpers the UI calls directly.

pub mod audio;
pub mod conflicts;
pub mod context;
pub mod esc;
pub mod ffi;
pub mod hotkeys;
pub mod inserter;
pub mod keymap;
pub mod login_item;
pub mod permissions;
pub mod prefs;
pub mod resample;
pub mod secure_input;
pub mod sounds;
pub mod tap;
pub mod tap_logic;
pub mod window;

use sayso_platform::Platform;
use std::path::PathBuf;
use std::sync::Arc;

/// Entry point of this crate.
pub struct MacPlatform;

impl MacPlatform {
    /// Build every macOS service. Call it on the main thread, because hotkey
    /// registration later needs the main thread. `sounds_dir` holds
    /// `start.wav`, `stop.wav`, `cancel.wav`, and `insert.wav`.
    #[allow(clippy::new_ret_no_self)] // The constructor returns the trait bundle on purpose.
    pub fn new(sounds_dir: PathBuf) -> Platform {
        Platform {
            hotkeys: Box::new(hotkeys::MacHotkeys::new()),
            inserter: Arc::new(inserter::MacInserter),
            context: Arc::new(context::MacContext),
            permissions: Arc::new(permissions::MacPermissions),
            audio: Arc::new(audio::MacAudio),
            sounds: Arc::new(sounds::MacSounds::new(sounds_dir)),
            login_item: Arc::new(login_item::MacLoginItem),
            prefs: Arc::new(prefs::MacPrefs),
        }
    }
}
