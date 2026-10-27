//! The `engine` field of a request: how to download and load one model.
//!
//! The field is `EngineKind` from `sayso-core`, serialized. This engine runs
//! the `sherpa`, `parakeet_cpp` (Linux), and `whisper_cpp` (Windows) kinds.
//! It also runs `sherpa_vad` and `sherpa_diarization` (the `SherpaVad` and
//! `SherpaDiarization` kinds). This file reads those two itself, because it
//! checks their fields more closely.

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
    /// Silero VAD: one ONNX file. Finds speech, no text.
    SileroVad,
    /// TEN VAD: one ONNX file. Finds speech, no text.
    TenVad,
    /// Speaker diarization: a pyannote segmentation model and a speaker
    /// embedding model. Finds who speaks when, no text.
    Diarization,
}

/// What a model does with audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    /// Audio to text: a final pass, a live preview, or both.
    Speech,
    /// Finds the parts with speech (`vad`, `vad_start`).
    Vad,
    /// Finds who speaks when (`diarize`).
    Diarization,
}

impl Task {
    /// The name of the task in an error message.
    pub fn name(self) -> &'static str {
        match self {
            Task::Speech => "speech",
            Task::Vad => "VAD",
            Task::Diarization => "diarization",
        }
    }
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

    pub fn task(self) -> Task {
        match self {
            Recipe::SileroVad | Recipe::TenVad => Task::Vad,
            Recipe::Diarization => Task::Diarization,
            _ => Task::Speech,
        }
    }

    /// True when the main archive can do the final pass.
    pub fn final_pass(self) -> bool {
        self.task() == Task::Speech && self != Recipe::ZipformerStreaming
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
            Recipe::ZipformerStreaming
            | Recipe::WhisperCpp
            | Recipe::SileroVad
            | Recipe::TenVad
            | Recipe::Diarization => usize::MAX,
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
    /// `file` in the sherpa-onnx `asr-models` release (a VAD model). Its folder, `archive`, is
    /// the file name without `.onnx`.
    SherpaFile { file: String },
    /// A diarization model. `archive` is a `.tar.bz2` in the sherpa-onnx release
    /// `speaker-segmentation-models`. `embedding` is an ONNX file in the release
    /// `speaker-recongition-models` (sic). It has a folder of its own, named after the file
    /// without `.onnx` ([`ModelSpec::embedding_archive`]).
    Diarization { embedding: String },
}

/// The Hugging Face repository with the whisper.cpp GGML files.
pub const WHISPER_CPP_REPO: &str = "ggerganov/whisper.cpp";
/// The revision of a whisper.cpp file, and the name of its folder. The catalog
/// names no commit for these files, so they come from the `main` branch.
pub const WHISPER_CPP_REVISION: &str = "main";

impl ModelSpec {
    /// Parse the `engine` object of a request.
    pub fn from_json(value: &Value) -> Result<ModelSpec> {
        match value.get("kind").and_then(Value::as_str) {
            Some("sherpa_vad") => return vad_spec(value),
            Some("sherpa_diarization") => return diarization_spec(value),
            _ => {}
        }
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
        archives.extend(self.embedding_archive());
        archives
    }

    /// The folder of the speaker embedding model of a diarization model.
    pub fn embedding_archive(&self) -> Option<&str> {
        match &self.source {
            Source::Diarization { embedding } => Some(file_stem(embedding)),
            _ => None,
        }
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

/// A string field of an `engine` object.
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("invalid engine field: missing string {key}"))
}

/// The name of a model file without `.onnx`. It is the name of the folder of the file.
fn file_stem(file: &str) -> &str {
    file.strip_suffix(".onnx").unwrap_or(file)
}

/// `{"kind":"sherpa_vad","recipe":"silero"|"ten","file":"silero_vad.onnx"}`
fn vad_spec(value: &Value) -> Result<ModelSpec> {
    let recipe = match text(value, "recipe")? {
        "silero" => Recipe::SileroVad,
        "ten" => Recipe::TenVad,
        other => bail!("unknown VAD recipe {other}"),
    };
    let file = text(value, "file")?;
    check_archive_name(file)?;
    Ok(ModelSpec {
        recipe,
        archive: file_stem(file).to_string(),
        source: Source::SherpaFile { file: file.to_string() },
        preview_archive: None,
        redecode_preview: false,
    })
}

/// `{"kind":"sherpa_diarization","segmentation":"<archive>","embedding":"<file>.onnx"}`
fn diarization_spec(value: &Value) -> Result<ModelSpec> {
    let segmentation = text(value, "segmentation")?;
    let embedding = text(value, "embedding")?;
    check_archive_name(segmentation)?;
    check_archive_name(embedding)?;
    if file_stem(embedding) == segmentation {
        bail!("the segmentation and the embedding model need different names");
    }
    Ok(ModelSpec {
        recipe: Recipe::Diarization,
        archive: segmentation.to_string(),
        source: Source::Diarization { embedding: embedding.to_string() },
        preview_archive: None,
        redecode_preview: false,
    })
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
    fn parses_a_vad_engine() {
        let spec = ModelSpec::from_json(&json!({"kind": "sherpa_vad", "recipe": "silero", "file": "silero_vad.onnx"})).unwrap();
        assert_eq!(spec.recipe, Recipe::SileroVad);
        assert_eq!(spec.source, Source::SherpaFile { file: "silero_vad.onnx".into() });
        assert_eq!(spec.archives(), vec!["silero_vad"]);
        assert_eq!(spec.recipe.task(), Task::Vad);
        assert!(!spec.recipe.final_pass() && !spec.has_preview() && spec.embedding_archive().is_none());

        let ten = ModelSpec::from_json(&json!({"kind": "sherpa_vad", "recipe": "ten", "file": "ten-vad.onnx"})).unwrap();
        assert_eq!((ten.recipe, ten.archives()), (Recipe::TenVad, vec!["ten-vad"]));

        for bad in [
            json!({"kind": "sherpa_vad", "recipe": "nope", "file": "a.onnx"}),
            json!({"kind": "sherpa_vad", "recipe": "silero"}),
            json!({"kind": "sherpa_vad", "file": "a.onnx"}),
            json!({"kind": "sherpa_vad", "recipe": "silero", "file": "../a.onnx"}),
            json!({"kind": "sherpa_vad", "recipe": "silero", "file": 7}),
        ] {
            assert!(ModelSpec::from_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn parses_a_diarization_engine() {
        let value = json!({
            "kind": "sherpa_diarization",
            "segmentation": "sherpa-onnx-pyannote-segmentation-3-0",
            "embedding": "3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx"
        });
        let spec = ModelSpec::from_json(&value).unwrap();
        assert_eq!(spec.recipe, Recipe::Diarization);
        assert_eq!(spec.recipe.task(), Task::Diarization);
        assert_eq!(spec.archive, "sherpa-onnx-pyannote-segmentation-3-0");
        assert_eq!(spec.embedding_archive(), Some("3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced"));
        assert_eq!(
            spec.archives(),
            vec!["sherpa-onnx-pyannote-segmentation-3-0", "3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced"]
        );
        assert!(!spec.recipe.final_pass() && !spec.has_preview());

        // Any file name works for the embedding model, with or without `.onnx`.
        let other = ModelSpec::from_json(&json!({"kind": "sherpa_diarization", "segmentation": "s", "embedding": "model"})).unwrap();
        assert_eq!(other.archives(), vec!["s", "model"]);

        for bad in [
            json!({"kind": "sherpa_diarization", "segmentation": "s"}),
            json!({"kind": "sherpa_diarization", "embedding": "e.onnx"}),
            json!({"kind": "sherpa_diarization", "segmentation": "../s", "embedding": "e.onnx"}),
            json!({"kind": "sherpa_diarization", "segmentation": "s", "embedding": "a/e.onnx"}),
            json!({"kind": "sherpa_diarization", "segmentation": "e", "embedding": "e.onnx"}),
        ] {
            assert!(ModelSpec::from_json(&bad).is_err(), "{bad}");
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
                let task = match info.task() {
                    sayso_core::models::ModelTask::Speech => Task::Speech,
                    sayso_core::models::ModelTask::Vad => Task::Vad,
                    sayso_core::models::ModelTask::Diarization => Task::Diarization,
                };
                assert_eq!(spec.recipe.task(), task, "{system:?} {}", info.id);
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
