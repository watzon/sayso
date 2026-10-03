//! The model folders on disk.
//!
//! Layout, under `SAYSO_MODELS_DIR`:
//!
//! ```text
//! <model id>/
//!   <archive>/            the extracted archive, renamed into place when complete
//!   <preview archive>/    the streaming archive, for a model with a live preview
//!   .partial-<archive>/   an extraction in progress (deleted when it fails)
//!   .complete             the archive names, one per line, written last
//! ```
//!
//! A model is downloaded only when `.complete` names exactly its archives, so a
//! download that stopped half way never looks complete.

use crate::spec::ModelSpec;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

const MARKER: &str = ".complete";
const PARTIAL_PREFIX: &str = ".partial-";

pub struct ModelStore {
    root: PathBuf,
}

impl ModelStore {
    pub fn new(root: PathBuf) -> Self {
        ModelStore { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder of a model. The id must be one plain path part.
    pub fn folder(&self, model: &str) -> Result<PathBuf> {
        let plain = !model.is_empty()
            && !model.starts_with('.')
            && !model.contains(['/', '\\', ':'])
            && model != "..";
        if !plain {
            bail!("invalid model id {model:?}");
        }
        Ok(self.root.join(model))
    }

    /// The extracted files of one archive.
    pub fn archive_dir(&self, model: &str, archive: &str) -> Result<PathBuf> {
        Ok(self.folder(model)?.join(archive))
    }

    /// The temporary folder for an extraction in progress.
    pub fn staging_dir(&self, model: &str, archive: &str) -> Result<PathBuf> {
        Ok(self
            .folder(model)?
            .join(format!("{PARTIAL_PREFIX}{archive}")))
    }

    /// True when every archive of the model is extracted and marked complete.
    pub fn is_downloaded(&self, model: &str, spec: &ModelSpec) -> bool {
        let Ok(folder) = self.folder(model) else {
            return false;
        };
        let Ok(marker) = std::fs::read_to_string(folder.join(MARKER)) else {
            return false;
        };
        let listed: Vec<&str> = marker.lines().filter(|l| !l.is_empty()).collect();
        listed == spec.archives() && spec.archives().iter().all(|a| folder.join(a).is_dir())
    }

    /// Write the marker. Call it after every archive is in place.
    pub fn mark_downloaded(&self, model: &str, spec: &ModelSpec) -> Result<()> {
        let folder = self.folder(model)?;
        let temp = folder.join(format!("{MARKER}.tmp"));
        let mut text = spec.archives().join("\n");
        text.push('\n');
        std::fs::write(&temp, text).with_context(|| format!("cannot write {}", temp.display()))?;
        std::fs::rename(&temp, folder.join(MARKER))
            .with_context(|| format!("cannot write the marker in {}", folder.display()))
    }

    /// Remove the marker and any extraction that a stopped download left behind.
    pub fn prepare_download(&self, model: &str) -> Result<()> {
        let folder = self.folder(model)?;
        std::fs::create_dir_all(&folder)
            .with_context(|| format!("cannot create {}", folder.display()))?;
        remove_file_if_exists(&folder.join(MARKER))?;
        for entry in std::fs::read_dir(&folder)? {
            let entry = entry?;
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(PARTIAL_PREFIX)
            {
                std::fs::remove_dir_all(entry.path())
                    .with_context(|| format!("cannot remove {}", entry.path().display()))?;
            }
        }
        Ok(())
    }

    /// Delete every file of a model.
    pub fn delete(&self, model: &str) -> Result<()> {
        let folder = self.folder(model)?;
        match std::fs::remove_dir_all(&folder) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("cannot delete {}", folder.display()))
            }
            _ => Ok(()),
        }
    }
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("cannot remove {}", path.display()))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Recipe;

    fn spec() -> ModelSpec {
        ModelSpec {
            recipe: Recipe::ParakeetTdt,
            archive: "main".into(),
            preview_archive: Some("preview".into()),
        }
    }

    #[test]
    fn downloaded_needs_the_marker_and_every_archive() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelStore::new(dir.path().to_path_buf());
        let spec = spec();
        assert!(!store.is_downloaded("m", &spec));

        store.prepare_download("m").unwrap();
        std::fs::create_dir_all(store.archive_dir("m", "main").unwrap()).unwrap();
        assert!(!store.is_downloaded("m", &spec), "no marker yet");

        std::fs::create_dir_all(store.archive_dir("m", "preview").unwrap()).unwrap();
        store.mark_downloaded("m", &spec).unwrap();
        assert!(store.is_downloaded("m", &spec));

        // Another archive in the catalog means a new download.
        let other = ModelSpec {
            archive: "main-v2".into(),
            ..spec.clone()
        };
        assert!(!store.is_downloaded("m", &other));

        store.delete("m").unwrap();
        assert!(!store.is_downloaded("m", &spec));
        store.delete("m").unwrap(); // Deleting twice is fine.
    }

    #[test]
    fn prepare_removes_partial_extractions_and_the_marker() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelStore::new(dir.path().to_path_buf());
        let staging = store.staging_dir("m", "main").unwrap();
        std::fs::create_dir_all(staging.join("x")).unwrap();
        std::fs::create_dir_all(store.archive_dir("m", "main").unwrap()).unwrap();
        std::fs::create_dir_all(store.archive_dir("m", "preview").unwrap()).unwrap();
        store.mark_downloaded("m", &spec()).unwrap();

        store.prepare_download("m").unwrap();
        assert!(!staging.exists());
        assert!(!store.is_downloaded("m", &spec()));
        assert!(
            store.archive_dir("m", "main").unwrap().is_dir(),
            "complete archives stay"
        );
    }

    #[test]
    fn rejects_unsafe_model_ids() {
        let store = ModelStore::new(PathBuf::from("/tmp/models"));
        for bad in ["", "..", ".x", "a/b", "openai:whisper-1"] {
            assert!(store.folder(bad).is_err(), "{bad}");
        }
        assert_eq!(
            store.folder("kroko-en").unwrap(),
            PathBuf::from("/tmp/models/kroko-en")
        );
    }
}
