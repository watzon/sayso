//! What Sayso remembers between update checks: `update-state.json` in the
//! data folder. The file stays for the life of the install.

use semver::Version;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateState {
    /// The highest version from a good manifest. A manifest that is older
    /// than this is a replay.
    pub highest_seen: Option<Version>,
    /// The time of the last check that succeeded, in seconds since 1970.
    pub last_check: Option<u64>,
    /// The version of the last update, until the user saw "Sayso is now X".
    pub notice: Option<Version>,
}

impl UpdateState {
    /// A file that is missing or not readable gives the default: the only
    /// loss is the replay rule until the next good check.
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                log::warn!("update: {} is not readable: {e}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        crate::files::write_atomic(path, &serde_json::to_vec_pretty(self)?)
    }

    /// Record a good manifest with `version`. `highest_seen` never goes down.
    pub fn saw(&mut self, version: &Version, now: u64) {
        if self.highest_seen.as_ref().is_none_or(|seen| version.cmp_precedence(seen).is_gt()) {
            self.highest_seen = Some(version.clone());
        }
        self.last_check = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_survives_a_save_and_a_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-state.json");
        assert_eq!(UpdateState::load(&path), UpdateState::default());
        let mut state = UpdateState::default();
        state.saw(&Version::new(0, 3, 0), 100);
        state.save(&path).unwrap();
        assert_eq!(UpdateState::load(&path), state);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn highest_seen_never_goes_down() {
        let mut state = UpdateState::default();
        state.saw(&Version::new(0, 4, 0), 100);
        state.saw(&Version::new(0, 3, 0), 200);
        assert_eq!(state.highest_seen, Some(Version::new(0, 4, 0)));
        assert_eq!(state.last_check, Some(200));
    }

    #[test]
    fn a_broken_file_gives_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-state.json");
        std::fs::write(&path, "{").unwrap();
        assert_eq!(UpdateState::load(&path), UpdateState::default());
    }
}
