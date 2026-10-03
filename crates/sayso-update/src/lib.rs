//! Sayso updates. `docs/updates.md` has the design.
//!
//! - [`manifest`]: the signed update manifest and the rules for it.
//! - [`check`]: one request for the manifest.
//! - [`state`]: what Sayso remembers between checks (`update-state.json`).
//! - [`Updater`]: the thread that checks at start and one time each day, and
//!   reports each [`Status`].
//!
//! - [`download`], [`install`], [`record`], [`lock`], [`swap`]: the download
//!   of a release file, the staged copy beside the installed app, and the
//!   exchange of the two. Only macOS installs by itself now. On the other
//!   systems a newer version shows with a [`Manual`] reason, and the app
//!   opens the download page.
//!
//! This crate has no UI and names no platform crate.

pub mod check;
pub mod download;
mod files;
pub mod install;
pub mod keys;
pub mod lock;
pub mod manifest;
pub mod record;
pub mod state;
pub mod swap;
mod updater;

pub use install::Manual;
pub use swap::{AtStart, SwapError};
pub use updater::{Options, Status, Updater};

/// The version of this build.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where the update manifest of the newest release is.
pub const MANIFEST_URL: &str = "https://github.com/watzon/sayso/releases/latest/download/latest.json";

/// Where a user gets a new version by hand.
pub const DOWNLOAD_PAGE: &str = "https://justsayso.app/download";

/// True for a build from the Release workflow, which sets
/// `SAYSO_OFFICIAL_BUILD=1`. Only an official build looks for updates: a
/// build from source must never offer the official app as its update.
pub fn official_build() -> bool {
    option_env!("SAYSO_OFFICIAL_BUILD") == Some("1")
}

/// The key of this system in the `assets` table of a manifest, for example
/// `macos-aarch64`.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}
