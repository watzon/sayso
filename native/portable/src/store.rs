//! Where models live under `SAYSO_MODELS_DIR`, which files each model needs,
//! and the local (no network) completeness check.
//!
//! Each model owns the folder `<models dir>/<model id>/`. A download writes
//! each file as `<file>.partial` and renames it when it is complete. The
//! marker `.sayso-downloaded` goes in last, after every file succeeded. Only a
//! folder with the marker and every file is "downloaded".

use crate::protocol::{Result, engine_kind_name};
use sayso_core::models::EngineKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MARKER: &str = ".sayso-downloaded";
pub const PARTIAL_SUFFIX: &str = ".partial";

/// Files of the int8 Parakeet exports by istupakov that transcribe-rs loads
/// (`ParakeetModel::load` with `Quantization::Int8`). `config.json` is not read.
pub const PARAKEET_FILES: [&str; 4] = [
    "encoder-model.int8.onnx",
    "decoder_joint-model.int8.onnx",
    "nemo128.onnx",
    "vocab.txt",
];

/// The Hugging Face repository with the whisper.cpp GGML files.
pub const WHISPER_CPP_REPO: &str = "ggerganov/whisper.cpp";

/// The runtimes of the portable engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// NVIDIA Parakeet TDT through ONNX Runtime.
    Parakeet,
    /// OpenAI Whisper through whisper.cpp.
    Whisper,
}

impl Family {
    /// The runtime of an engine kind, or why this engine cannot run it.
    pub fn of(engine: &EngineKind) -> Result<Family> {
        match engine {
            EngineKind::Onnx { family, repo } if family == "parakeet" => {
                check_repo(repo)?;
                Ok(Family::Parakeet)
            }
            EngineKind::Onnx { family, .. } => {
                Err(format!("onnx family {family} is not supported"))
            }
            EngineKind::WhisperCpp { file } => {
                check_file_name(file)?;
                Ok(Family::Whisper)
            }
            other => Err(format!(
                "engine kind {} does not run in the portable engine",
                engine_kind_name(other)
            )),
        }
    }

    /// Parakeet gets a live preview by decoding the growing recording again.
    /// Whisper is too slow for that on a CPU.
    pub fn has_preview(self) -> bool {
        self == Family::Parakeet
    }
}

/// One file to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    pub name: String,
    pub url: String,
}

/// The files of a model, with their Hugging Face URLs under `endpoint`
/// (`https://huggingface.co`).
pub fn remote_files(engine: &EngineKind, endpoint: &str) -> Result<Vec<RemoteFile>> {
    let endpoint = endpoint.trim_end_matches('/');
    let file = |repo: &str, name: &str| RemoteFile {
        name: name.to_string(),
        url: format!("{endpoint}/{repo}/resolve/main/{name}"),
    };
    Ok(match (Family::of(engine)?, engine) {
        (Family::Parakeet, EngineKind::Onnx { repo, .. }) => {
            PARAKEET_FILES.iter().map(|name| file(repo, name)).collect()
        }
        (Family::Whisper, EngineKind::WhisperCpp { file: name }) => {
            vec![file(WHISPER_CPP_REPO, name)]
        }
        _ => unreachable!("Family::of checked the kind"),
    })
}

/// A Hugging Face repository id: `owner/name` with URL-safe characters.
fn check_repo(repo: &str) -> Result<()> {
    let mut parts = repo.split('/');
    let ok = matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(b), None) if safe_name(a) && safe_name(b));
    if ok {
        Ok(())
    } else {
        Err(format!("bad Hugging Face repository {repo:?}"))
    }
}

fn check_file_name(file: &str) -> Result<()> {
    if safe_name(file) {
        Ok(())
    } else {
        Err(format!("bad model file name {file:?}"))
    }
}

/// Letters, digits, `.`, `_`, and `-`, not starting with a dot. Safe as one
/// path component and in a URL.
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Disk layout under the models directory.
#[derive(Debug, Clone)]
pub struct ModelStore {
    root: PathBuf,
}

impl ModelStore {
    pub fn new(root: PathBuf) -> ModelStore {
        ModelStore { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder of one model. The id must be a plain folder name.
    pub fn folder(&self, model: &str) -> Result<PathBuf> {
        if safe_name(model) {
            Ok(self.root.join(model))
        } else {
            Err(format!("bad model id {model:?}"))
        }
    }

    /// The marker and every file are there. No network.
    pub fn is_downloaded(&self, model: &str, engine: &EngineKind) -> bool {
        let (Ok(folder), Ok(files)) = (self.folder(model), remote_files(engine, "")) else {
            return false;
        };
        folder.join(MARKER).is_file() && files.iter().all(|f| folder.join(&f.name).is_file())
    }

    pub fn mark_downloaded(&self, model: &str) -> Result<()> {
        let path = self.folder(model)?.join(MARKER);
        std::fs::write(&path, b"").map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    /// Remove the folder of a model. Windows can refuse for a moment while a
    /// file is still open (a model being dropped, a virus scanner), so it tries
    /// again for about two seconds.
    pub fn delete(&self, model: &str) -> Result<()> {
        let folder = self.folder(model)?;
        let mut attempt = 0;
        loop {
            match std::fs::remove_dir_all(&folder) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(_) if attempt < 20 => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(format!("cannot delete {}: {e}", folder.display())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parakeet() -> EngineKind {
        EngineKind::Onnx {
            family: "parakeet".into(),
            repo: "istupakov/parakeet-tdt-0.6b-v3-onnx".into(),
        }
    }

    #[test]
    fn families() {
        assert_eq!(Family::of(&parakeet()), Ok(Family::Parakeet));
        assert_eq!(
            Family::of(&EngineKind::WhisperCpp {
                file: "ggml-tiny.bin".into()
            }),
            Ok(Family::Whisper)
        );
        assert!(
            Family::of(&EngineKind::Onnx {
                family: "moonshine".into(),
                repo: "a/b".into()
            })
            .is_err()
        );
        let apple = Family::of(&EngineKind::AppleSpeech).unwrap_err();
        assert_eq!(
            apple,
            "engine kind apple_speech does not run in the portable engine"
        );
        assert!(
            Family::of(&EngineKind::WhisperCpp {
                file: "../x.bin".into()
            })
            .is_err()
        );
        assert!(
            Family::of(&EngineKind::Onnx {
                family: "parakeet".into(),
                repo: "a/b/c".into()
            })
            .is_err()
        );
        assert!(
            Family::of(&EngineKind::Onnx {
                family: "parakeet".into(),
                repo: "a/b?x=1".into()
            })
            .is_err()
        );
        assert!(Family::Parakeet.has_preview() && !Family::Whisper.has_preview());
    }

    #[test]
    fn urls_point_at_hugging_face() {
        let files = remote_files(&parakeet(), "https://huggingface.co/").unwrap();
        assert_eq!(files.len(), 4);
        assert_eq!(
            files[0].url,
            "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/encoder-model.int8.onnx"
        );
        let files = remote_files(
            &EngineKind::WhisperCpp {
                file: "ggml-tiny.bin".into(),
            },
            "https://huggingface.co",
        )
        .unwrap();
        assert_eq!(
            files,
            vec![RemoteFile {
                name: "ggml-tiny.bin".into(),
                url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin"
                    .into()
            }]
        );
    }

    #[test]
    fn downloaded_needs_the_marker_and_every_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelStore::new(dir.path().to_path_buf());
        let engine = EngineKind::WhisperCpp {
            file: "ggml-tiny.bin".into(),
        };
        assert!(!store.is_downloaded("whisper-tiny", &engine));
        let folder = store.folder("whisper-tiny").unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("ggml-tiny.bin"), b"x").unwrap();
        assert!(
            !store.is_downloaded("whisper-tiny", &engine),
            "no marker yet"
        );
        store.mark_downloaded("whisper-tiny").unwrap();
        assert!(store.is_downloaded("whisper-tiny", &engine));
        std::fs::remove_file(folder.join("ggml-tiny.bin")).unwrap();
        assert!(
            !store.is_downloaded("whisper-tiny", &engine),
            "a file is missing"
        );
        store.delete("whisper-tiny").unwrap();
        assert!(!folder.exists());
        store.delete("whisper-tiny").unwrap();
    }

    #[test]
    fn model_ids_are_plain_folder_names() {
        let store = ModelStore::new(PathBuf::from("models"));
        assert!(store.folder("parakeet-tdt-v3").is_ok());
        for bad in ["", "..", "../x", "a/b", "a\\b", ".hidden", "c:x"] {
            assert!(store.folder(bad).is_err(), "{bad}");
        }
    }
}
