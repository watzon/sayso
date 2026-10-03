//! The record of one staged update: `update.json` in the data folder.
//!
//! Sayso writes each phase before the action of that phase, so a start
//! after a crash knows how far the update came.

use semver::Version;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// The new version is beside the installed app and waits for the swap.
    Staged,
    /// The swap started. The exchange happened or it did not.
    Swapping,
    /// The exchange happened.
    Swapped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// The complete `latest.json` that the staged update came from. Sayso
    /// verifies its signature again before the swap.
    pub envelope: String,
    pub from_version: Version,
    pub to_version: Version,
    /// The installed app at the time of staging.
    pub install_path: PathBuf,
    pub phase: Phase,
    /// The number of swaps that started.
    pub attempts: u32,
    pub error: Option<String>,
}

impl Record {
    pub fn load(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).inspect_err(|e| log::warn!("update: {} is not readable: {e}", path.display())).ok()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        crate::files::write_atomic(path, &serde_json::to_vec_pretty(self)?)
    }

    pub fn delete(path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    /// Write the phase `failed` with its cause.
    pub fn fail(mut self, path: &Path, error: impl Into<String>) -> Self {
        self.phase = Phase::Failed;
        self.error = Some(error.into());
        if let Err(e) = self.save(path) {
            log::error!("update: could not write {}: {e}", path.display());
        }
        self
    }
}
