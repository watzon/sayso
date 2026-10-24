//! The `engine` field of a request: how to download and load one model.
//!
//! The field is `EngineKind` from `sayso-core`, serialized. This engine runs
//! the `sherpa`, `parakeet_cpp` (Linux), and `whisper_cpp` (Windows) kinds.

use anyhow::{Result, anyhow, bail};
use sayso_core::models::EngineKind;
use serde_json::Value;

/// How the engine loads the files of an archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recipe {
    /// NeMo Parakeet TDT: encoder, decoder, and joiner (transducer).
    ParakeetTdt,
    /// NeMo Parakeet TDT-CTC: the CTC head, one model file.
    ParakeetTdtCtc,
    /// A Parakeet TDT GGUF, run by parakeet.cpp (`parakeet_cpp` kind).
    ParakeetGguf,
    /// A Whisper GGML file, run by whisper.cpp (`whisper_cpp` kind).
    WhisperCpp,
    /// A streaming zipformer transducer. Live preview only.
    ZipformerStreaming,
    Whisper,
    SenseVoice,
    Moonshine,
}

impl Recipe {
    pub fn parse(name: &str) -> Result<Recipe> {
        Ok(match name {
            "parakeet_tdt" => Recipe::ParakeetTdt,
            "parakeet_tdt_ctc" => Recipe::ParakeetTdtCtc,
            "zipformer_streaming" => Recipe::ZipformerStreaming,
            "whisper" => Recipe::Whisper,
            "sense_voice" => Recipe::SenseVoice,
            "moonshine" => Recipe::Moonshine,
            other => bail!("unknown sherpa recipe {other}"),
        })
    }

    /// True when the main archive can do the final pass.
    pub fn final_pass(self) -> bool {
        self != Recipe::ZipformerStreaming
    }

    /// The longest piece of audio that one decode gets, in seconds. Longer
    /// recordings are cut at quiet points. Whisper reads 30 second windows.
    /// Moonshine (sherpa-onnx 1.13.8) returns no text for 10 seconds or more.
    /// whisper.cpp cuts long audio into windows itself.
    pub fn max_chunk_seconds(self) -> usize {
        match self {
            Recipe::Whisper => 28,
            Recipe::SenseVoice => 25,
            Recipe::Moonshine => 7,
            Recipe::ParakeetTdt | Recipe::ParakeetTdtCtc | Recipe::ParakeetGguf => 60,
            Recipe::ZipformerStreaming | Recipe::WhisperCpp => usize::MAX,
        }
    }

    /// True when the main archive is one downloaded file, not an extracted archive.
    pub fn single_file(self) -> bool {
        matches!(self, Recipe::ParakeetGguf | Recipe::WhisperCpp)
    }
}

/// One model as the engine sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    pub recipe: Recipe,
    /// The main archive's folder in the model's store: the archive name in the
    /// sherpa-onnx `asr-models` release, without `.tar.bz2`, or the GGUF's commit.
    pub archive: String,
    /// Where the main archive comes from.
    pub source: Source,
    /// The streaming archive that gives the live preview, if the main one cannot.
    pub preview_archive: Option<String>,
    /// The final pass also gives the live preview: it decodes the growing recording again.
    pub redecode_preview: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A `.tar.bz2` in the sherpa-onnx release.
    SherpaRelease,
    /// `file` in a Hugging Face repository at the commit `archive`. A new commit gets a new folder, so
    /// it downloads again even when the file keeps its name.
    HuggingFace { repo: String, file: String },
}

/// The Hugging Face repository with the whisper.cpp GGML files.
pub const WHISPER_CPP_REPO: &str = "ggerganov/whisper.cpp";
/// The revision of a whisper.cpp file, and the name of its folder. The catalog
/// names no commit for these files, so they come from the `main` branch.
pub const WHISPER_CPP_REVISION: &str = "main";

impl ModelSpec {
    /// Parse the `engine` object of a request.
    pub fn from_json(value: &Value) -> Result<ModelSpec> {
        let kind: EngineKind = serde_json::from_value(value.clone())
            .map_err(|e| anyhow!("invalid engine field: {e}"))?;
        let spec = match kind {
            EngineKind::Sherpa {
                recipe,
                archive,
                preview_archive,
                redecode_preview,
            } => {
                let recipe = Recipe::parse(&recipe)?;
                if redecode_preview && (preview_archive.is_some() || !recipe.final_pass()) {
                    bail!("a model with redecode_preview needs a final pass and no preview archive");
                }
                ModelSpec {
                    recipe,
                    archive,
                    source: Source::SherpaRelease,
                    preview_archive,
                    redecode_preview,
                }
            }
            EngineKind::ParakeetCpp {
                repo,
                revision,
                file,
                preview_archive,
            } => {
                check_repo(&repo)?;
                check_archive_name(&file)?;
                ModelSpec {
                    recipe: Recipe::ParakeetGguf,
                    archive: revision,
                    source: Source::HuggingFace { repo, file },
                    preview_archive,
                    redecode_preview: false,
                }
            }
            EngineKind::WhisperCpp { file } => {
                check_archive_name(&file)?;
                ModelSpec {
                    recipe: Recipe::WhisperCpp,
                    archive: WHISPER_CPP_REVISION.into(),
                    source: Source::HuggingFace {
                        repo: WHISPER_CPP_REPO.into(),
                        file,
                    },
                    preview_archive: None,
                    redecode_preview: false,
                }
            }
            _ => {
                let name = value.get("kind").and_then(Value::as_str).unwrap_or("?");
                bail!("engine kind {name} does not run in this engine");
            }
        };
        check_archive_name(&spec.archive)?;
        if let Some(preview) = &spec.preview_archive {
            check_archive_name(preview)?;
        }
        Ok(spec)
    }

    /// Every archive to download, main archive first.
    pub fn archives(&self) -> Vec<&str> {
        let mut archives = vec![self.archive.as_str()];
        if let Some(preview) = self
            .preview_archive
            .as_deref()
            .filter(|p| *p != self.archive)
        {
            archives.push(preview);
        }
        archives
    }

    /// True when the model gives a live preview: from a streaming archive, or by
    /// decoding again.
    pub fn has_preview(&self) -> bool {
        self.redecode_preview || self.stream_archive().is_some()
    }

    /// The archive that streams the live preview, if the model has one.
    pub fn stream_archive(&self) -> Option<&str> {
        if self.recipe == Recipe::ZipformerStreaming {
            Some(&self.archive)
        } else {
            self.preview_archive.as_deref()
        }
    }
}

/// A Hugging Face repository id becomes part of a URL: `owner/name`, each one plain path part.
fn check_repo(repo: &str) -> Result<()> {
    match repo.split_once('/') {
        Some((owner, name)) if check_archive_name(owner).is_ok() && check_archive_name(name).is_ok() => Ok(()),
        _ => bail!("invalid Hugging Face repository {repo:?}"),
    }
}

/// An archive name becomes a URL and a folder name, so it must be one plain path part.
fn check_archive_name(name: &str) -> Result<()> {
    let plain = !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if plain {
        Ok(())
    } else {
        bail!("invalid archive name {name:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_a_sherpa_engine() {
        let spec = ModelSpec::from_json(&json!({
            "kind": "sherpa", "recipe": "parakeet_tdt", "archive": "a-int8", "preview_archive": "p"
        }))
        .unwrap();
        assert_eq!(spec.recipe, Recipe::ParakeetTdt);
        assert_eq!(spec.archives(), vec!["a-int8", "p"]);
        assert_eq!(spec.stream_archive(), Some("p"));
        assert!(spec.recipe.final_pass());

        let stream = ModelSpec::from_json(&json!({
            "kind": "sherpa", "recipe": "zipformer_streaming", "archive": "z", "preview_archive": null
        }))
        .unwrap();
        assert_eq!(stream.archives(), vec!["z"]);
        assert_eq!(stream.stream_archive(), Some("z"));
        assert!(!stream.recipe.final_pass());
    }

    #[test]
    fn parses_a_parakeet_cpp_engine() {
        let spec = ModelSpec::from_json(&json!({
            "kind": "parakeet_cpp", "repo": "o/r", "revision": "c0ffee", "file": "m.gguf", "preview_archive": "p"
        }))
        .unwrap();
        assert_eq!(spec.recipe, Recipe::ParakeetGguf);
        assert_eq!(spec.source, Source::HuggingFace { repo: "o/r".into(), file: "m.gguf".into() });
        assert_eq!(spec.archives(), vec!["c0ffee", "p"]);
        assert_eq!(spec.stream_archive(), Some("p"));
    }

    #[test]
    fn parses_a_whisper_cpp_engine() {
        let spec = ModelSpec::from_json(&json!({"kind": "whisper_cpp", "file": "ggml-tiny.bin"})).unwrap();
        assert_eq!(spec.recipe, Recipe::WhisperCpp);
        assert_eq!(
            spec.source,
            Source::HuggingFace { repo: WHISPER_CPP_REPO.into(), file: "ggml-tiny.bin".into() }
        );
        assert_eq!(spec.archives(), vec![WHISPER_CPP_REVISION]);
        assert!(spec.recipe.final_pass() && spec.recipe.single_file() && !spec.has_preview());
        for bad in ["../ggml-tiny.bin", "a/b.bin", ""] {
            assert!(ModelSpec::from_json(&json!({"kind": "whisper_cpp", "file": bad})).is_err(), "{bad}");
        }
    }

    #[test]
    fn parses_a_redecode_preview() {
        let spec = ModelSpec::from_json(&json!({
            "kind": "sherpa", "recipe": "parakeet_tdt", "archive": "a", "preview_archive": null, "redecode_preview": true
        }))
        .unwrap();
        assert!(spec.redecode_preview && spec.has_preview());
        assert_eq!(spec.stream_archive(), None);
        assert_eq!(spec.archives(), vec!["a"]);
        // Without the field there is no redecode preview.
        let plain =
            ModelSpec::from_json(&json!({"kind": "sherpa", "recipe": "parakeet_tdt", "archive": "a", "preview_archive": null}))
                .unwrap();
        assert!(!plain.redecode_preview && !plain.has_preview());
        for bad in [
            json!({"kind": "sherpa", "recipe": "parakeet_tdt", "archive": "a", "preview_archive": "p", "redecode_preview": true}),
            json!({"kind": "sherpa", "recipe": "zipformer_streaming", "archive": "a", "preview_archive": null, "redecode_preview": true}),
        ] {
            assert!(ModelSpec::from_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_catalog_serializes_to_specs_this_engine_reads() {
        use sayso_core::models_onnx::{System, catalog_for};
        for system in [System::Linux, System::Windows] {
            for info in catalog_for(system) {
                let value = serde_json::to_value(&info.engine).unwrap();
                let spec = ModelSpec::from_json(&value).unwrap_or_else(|e| panic!("{}: {e}", info.id));
                assert_eq!(spec.has_preview(), info.live_preview, "{system:?} {}", info.id);
                assert_eq!(spec.recipe.final_pass(), info.final_pass, "{system:?} {}", info.id);
            }
        }
    }

    #[test]
    fn rejects_other_kinds_and_unsafe_names() {
        let error =
            ModelSpec::from_json(&json!({"kind": "whisper", "variant": "tiny"})).unwrap_err();
        assert_eq!(
            error.to_string(),
            "engine kind whisper does not run in this engine"
        );
        assert!(ModelSpec::from_json(&json!({"kind": "nope"})).is_err());
        for bad in ["../x", "a/b", ".hidden", ""] {
            let value = json!({"kind": "sherpa", "recipe": "whisper", "archive": bad, "preview_archive": null});
            assert!(ModelSpec::from_json(&value).is_err(), "{bad}");
        }
        let bad = [
            ("o", "c0ffee", "m.gguf"),
            ("o/r/x", "c0ffee", "m.gguf"),
            ("../r", "c0ffee", "m.gguf"),
            ("o/r", "c0ffee", "../m.gguf"),
            ("o/r", "../main", "m.gguf"),
            ("o/r", "", "m.gguf"),
        ];
        for (repo, revision, file) in bad {
            let value = json!({"kind": "parakeet_cpp", "repo": repo, "revision": revision, "file": file, "preview_archive": null});
            assert!(ModelSpec::from_json(&value).is_err(), "{repo} {revision} {file}");
        }
        let value =
            json!({"kind": "sherpa", "recipe": "nope", "archive": "a", "preview_archive": null});
        assert_eq!(
            ModelSpec::from_json(&value).unwrap_err().to_string(),
            "unknown sherpa recipe nope"
        );
    }
}
