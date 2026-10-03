//! The install of this Sayso, and the system steps that replace it.
//!
//! An install that can update itself gives an [`Installer`]. Any other
//! install gives a [`Manual`] reason, and the user gets the new version from
//! the download page. Only macOS has an installer now.

use semver::Version;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(target_os = "macos")]
mod macos;

/// Why this install cannot update itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manual {
    /// Sayso has no installer for this system yet.
    Unsupported,
    /// The app runs from a place that cannot be changed: a disk image, an
    /// App Translocation path, or outside an app bundle.
    WrongFolder,
    /// The user cannot write to the folder of the app.
    NoPermission,
    /// The disk of the app cannot exchange two folders in one step.
    NoExchange,
}

/// The system steps of an update. `swap.rs` has the order and the rules,
/// and the tests give it an installer that works on temporary folders.
pub trait Installer: Send + Sync {
    /// The installed app.
    fn install_path(&self) -> &Path;
    /// The file of the install lock.
    fn lock_path(&self) -> PathBuf;
    /// The record of the update of this install (`update.json`). It is
    /// beside the staged app, so every Sayso from this install sees the same
    /// record, with any data folder.
    fn record_path(&self) -> PathBuf;
    /// The version of the app that is at the install path now. It is not
    /// the version of this process when a swap ran after the process started.
    fn installed_version(&self) -> Option<Version>;
    /// Unpack the release file beside the installed app and verify it.
    fn stage(&self, file: &Path, version: &Version) -> Result<(), String>;
    /// True when a staged app (or, after the exchange, the old app) is there.
    fn staging_exists(&self) -> bool;
    /// Verify the staged app again: the signature, and the notarization.
    fn verify_staged(&self) -> Result<(), String>;
    /// Exchange the staged app and the installed app in one step. A second
    /// call puts them back.
    fn exchange(&self) -> std::io::Result<()>;
    /// Start the installed app after this process exits.
    fn relaunch(&self) -> std::io::Result<()>;
    /// Delete the staged app (or, after the exchange, the old app). The record stays.
    fn remove_staging(&self);
}

/// The installer of this install, or the reason it has none.
pub fn detect() -> Result<Arc<dyn Installer>, Manual> {
    #[cfg(target_os = "macos")]
    {
        macos::MacInstall::detect().map(|install| Arc::new(install) as Arc<dyn Installer>)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(Manual::Unsupported)
    }
}
