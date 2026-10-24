//! The model folders on disk.
//!
//! Layout, under `SAYSO_MODELS_DIR`:
//!
//! ```text
//! <model id>/
//!   <archive>/            the extracted archive, or a GGUF's commit with the file, renamed into place when complete
//!   <preview archive>/    the streaming archive, for a model with a live preview
//!   .partial-<archive>/   an extraction in progress (deleted when it fails)
//!   .complete             the archive names, one per line, written last
//! ```
//!
//! A model is downloaded only when `.complete` names exactly its archives, so a
//! download that stopped half way never looks complete.
//!
//! The Windows engine before 0.12 (`native/portable`) kept the files of a model
//! directly in its folder, with the marker `.sayso-downloaded`. Its Whisper
//! files move into this layout ([`ModelStore::adopt_legacy`]). Its Parakeet
//! files are in a format this engine cannot read; a new download replaces them.

use crate::spec::{ModelSpec, Recipe, Source};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

const MARKER: &str = ".complete";
const PARTIAL_PREFIX: &str = ".partial-";
/// The marker of the Windows engine before 0.12.
const LEGACY_MARKER: &str = ".sayso-downloaded";

/// The files of the Windows engine before 0.12 in a model folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Legacy {
    /// There are none.
    None,
    /// They were in this engine's format and moved into place.
    Adopted,
    /// They are in a format this engine cannot read.
    FormatChanged,
}

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

    /// Remove the marker and everything that is not a complete archive of
    /// `spec`: extractions that a stopped download left behind, archives of an
    /// older version of the model, and files of the Windows engine before 0.12.
    pub fn prepare_download(&self, model: &str, spec: &ModelSpec) -> Result<()> {
        let folder = self.folder(model)?;
        std::fs::create_dir_all(&folder)
            .with_context(|| format!("cannot create {}", folder.display()))?;
        remove_file_if_exists(&folder.join(MARKER))?;
        let archives = spec.archives();
        for entry in std::fs::read_dir(&folder)? {
            let entry = entry?;
            let name = entry.file_name();
            let keep = entry.file_type()?.is_dir() && archives.iter().any(|a| name.to_str() == Some(a));
            if keep {
                continue;
            }
            let path = entry.path();
            let removed = if entry.file_type()?.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            removed.with_context(|| format!("cannot remove {}", path.display()))?;
        }
        Ok(())
    }

    /// Move the files of the Windows engine before 0.12 into this layout. A
    /// Whisper GGML file moves into its archive folder. Other files stay where
    /// they are, and a download of the model removes them.
    pub fn adopt_legacy(&self, model: &str, spec: &ModelSpec) -> Result<Legacy> {
        let folder = self.folder(model)?;
        let legacy_marker = folder.join(LEGACY_MARKER);
        if !legacy_marker.is_file() {
            return Ok(Legacy::None);
        }
        let file = match (&spec.source, spec.recipe) {
            (Source::HuggingFace { file, .. }, Recipe::WhisperCpp) if folder.join(file).is_file() => file,
            _ => return Ok(Legacy::FormatChanged),
        };
        let dest = self.archive_dir(model, &spec.archive)?;
        std::fs::create_dir_all(&dest).with_context(|| format!("cannot create {}", dest.display()))?;
        std::fs::rename(folder.join(file), dest.join(file))
            .with_context(|| format!("cannot move {file} into {}", dest.display()))?;
        self.mark_downloaded(model, spec)?;
        remove_file_if_exists(&legacy_marker)?;
        Ok(Legacy::Adopted)
    }

    /// Delete every file of a model. Windows can refuse for a moment while a
    /// file is still open (a model being dropped, a virus scanner), so there it
    /// tries again for about two seconds.
    pub fn delete(&self, model: &str) -> Result<()> {
        let folder = self.folder(model)?;
        let attempts = if cfg!(windows) { 20 } else { 1 };
        for attempt in 1.. {
            match std::fs::remove_dir_all(&folder) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(_) if attempt < attempts => std::thread::sleep(std::time::Duration::from_millis(100)),
                Err(e) => return Err(e).with_context(|| format!("cannot delete {}", folder.display())),
                Ok(()) => return Ok(()),
            }
        }
        unreachable!("the loop returns")
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
    use crate::spec::{Recipe, Source};

    fn spec() -> ModelSpec {
        ModelSpec {
            recipe: Recipe::ParakeetTdt,
            archive: "main".into(),
            source: Source::SherpaRelease,
            preview_archive: Some("preview".into()),
            redecode_preview: false,
        }
    }

    fn whisper() -> ModelSpec {
        ModelSpec {
            recipe: Recipe::WhisperCpp,
            archive: "main".into(),
            source: Source::HuggingFace {
                repo: "ggerganov/whisper.cpp".into(),
                file: "ggml-tiny.bin".into(),
            },
            preview_archive: None,
            redecode_preview: false,
        }
    }

    /// A model folder as the Windows engine before 0.12 left it.
    fn legacy(store: &ModelStore, model: &str, files: &[&str], marker: bool) -> PathBuf {
        let folder = store.folder(model).unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        for file in files {
            std::fs::write(folder.join(file), format!("contents of {file}")).unwrap();
        }
        if marker {
            std::fs::write(folder.join(LEGACY_MARKER), b"").unwrap();
        }
        folder
    }

    #[test]
    fn an_old_whisper_download_moves_into_place() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelStore::new(dir.path().to_path_buf());
        let folder = legacy(&store, "whisper-tiny", &["ggml-tiny.bin"], true);
        assert!(!store.is_downloaded("whisper-tiny", &whisper()));

        assert_eq!(store.adopt_legacy("whisper-tiny", &whisper()).unwrap(), Legacy::Adopted);
        assert!(store.is_downloaded("whisper-tiny", &whisper()));
        assert_eq!(
            std::fs::read_to_string(folder.join("main/ggml-tiny.bin")).unwrap(),
            "contents of ggml-tiny.bin"
        );
        assert!(!folder.join(LEGACY_MARKER).exists());
        assert_eq!(store.adopt_legacy("whisper-tiny", &whisper()).unwrap(), Legacy::None, "only once");
    }

    #[test]
    fn old_files_that_cannot_move_are_a_format_change() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelStore::new(dir.path().to_path_buf());
        // Parakeet files of the old engine.
        legacy(&store, "parakeet-tdt-v3", &["encoder-model.int8.onnx", "vocab.txt"], true);
        assert_eq!(store.adopt_legacy("parakeet-tdt-v3", &spec()).unwrap(), Legacy::FormatChanged);
        // A Whisper download that the old engine did not finish has no marker.
        legacy(&store, "whisper-tiny", &["ggml-tiny.bin.partial"], false);
        assert_eq!(store.adopt_legacy("whisper-tiny", &whisper()).unwrap(), Legacy::None);
        // A marker without the file.
        legacy(&store, "whisper-base", &[], true);
        assert_eq!(store.adopt_legacy("whisper-base", &whisper()).unwrap(), Legacy::FormatChanged);
        assert_eq!(store.adopt_legacy("never-downloaded", &whisper()).unwrap(), Legacy::None);
    }

    #[test]
    fn a_download_removes_old_files_and_keeps_complete_archives() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelStore::new(dir.path().to_path_buf());
        let folder = legacy(&store, "m", &["encoder-model.int8.onnx", "x.partial"], true);
        std::fs::create_dir_all(folder.join("main")).unwrap();
        std::fs::create_dir_all(folder.join("main-old")).unwrap();
        store.prepare_download("m", &spec()).unwrap();
        let mut left: Vec<String> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, vec!["main"]);
        assert_eq!(store.adopt_legacy("m", &spec()).unwrap(), Legacy::None);
    }

    #[test]
    fn downloaded_needs_the_marker_and_every_archive() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelStore::new(dir.path().to_path_buf());
        let spec = spec();
        assert!(!store.is_downloaded("m", &spec));

        store.prepare_download("m", &spec).unwrap();
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

        store.prepare_download("m", &spec()).unwrap();
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
