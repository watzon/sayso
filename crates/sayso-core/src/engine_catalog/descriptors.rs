//! The descriptors of the model store (`docs/sdk.md`, "Model store", "Layout").
//!
//! A descriptor names the artifacts of one model in one format. An artifact
//! names its source, pinned to a revision, and the path, size, and SHA-256 of
//! each of its files. Its digest comes from the files only ([`digest`]), so an
//! engine knows the digest before it downloads anything.
//!
//! The data is in `descriptors.json`. Do not edit it: `scripts/model-descriptors.py`
//! writes it from [`wanted`], the sources that each model of the catalogs needs:
//!
//! ```text
//! cargo run -q -p sayso-core --example descriptor_sources > /tmp/wanted.json
//! python3 -I scripts/model-descriptors.py /tmp/wanted.json
//! ```

use crate::engine_catalog::{Engine, models};
use crate::models::{EngineKind, ModelInfo};
use crate::models_onnx::System;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// The format of the FluidAudio models of the Swift engine: Core ML bundles and their files.
pub const FLUIDAUDIO: &str = "fluidaudio-coreml-1";
/// The format of the WhisperKit models of the Swift engine: a variant folder and a tokenizer.
pub const WHISPERKIT: &str = "whisperkit-coreml-1";
/// The format of the sherpa-onnx models of the Rust engine: extracted archives and ONNX files.
pub const SHERPA_ONNX: &str = "sherpa-onnx-1";
/// The format of the whisper.cpp models of the Rust engine: one GGML file.
pub const WHISPER_CPP: &str = "whisper-cpp-1";
/// The format of the parakeet.cpp models of the Rust engine: one GGUF file and a sherpa-onnx preview archive.
pub const PARAKEET_CPP: &str = "parakeet-cpp-1";

/// The format of the files of a model of `engine`. None for a model whose files
/// are not in the store: Apple Speech (macOS manages its files) and cloud models.
pub fn format(engine: &EngineKind) -> Option<&'static str> {
    Some(match engine {
        EngineKind::ParakeetUnified { .. }
        | EngineKind::ParakeetEou
        | EngineKind::ParakeetTdt { .. }
        | EngineKind::Nemotron { .. }
        | EngineKind::NemotronMultilingual { .. }
        | EngineKind::Cohere
        | EngineKind::Canary
        | EngineKind::SenseVoice
        | EngineKind::Paraformer
        | EngineKind::FluidVad
        | EngineKind::FluidDiarization => FLUIDAUDIO,
        EngineKind::Whisper { .. } => WHISPERKIT,
        EngineKind::Sherpa { .. } | EngineKind::SherpaVad { .. } | EngineKind::SherpaDiarization { .. } => SHERPA_ONNX,
        EngineKind::WhisperCpp { .. } => WHISPER_CPP,
        EngineKind::ParakeetCpp { .. } => PARAKEET_CPP,
        EngineKind::AppleSpeech | EngineKind::Remote { .. } => return None,
    })
}

/// One file of an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    /// The path relative to the artifact, in `/` form ([`check_path`]).
    pub path: String,
    pub size: u64,
    /// The SHA-256 of the contents, in lowercase hex.
    pub sha256: String,
}

/// Where the files of an artifact come from. Each source is pinned: a commit of
/// a Hugging Face repository, or a file of a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// Files of the Hugging Face repository `repo` at the commit `revision`.
    /// The file `path` of the artifact is `<folder>/<path>` in the repository.
    HuggingFace {
        repo: String,
        revision: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        folder: String,
    },
    /// A `.tar.bz2` file. The files of the artifact are the extracted files.
    /// When the archive holds one folder with everything in it, that folder
    /// is the artifact, as in `crates/sayso-engine/src/download.rs`.
    Archive { url: String },
    /// One file. The artifact has this file only.
    File { url: String },
}

impl Source {
    /// The URL of the file `path` of the artifact. None for an archive, whose
    /// files come out of one download.
    pub fn file_url(&self, path: &str) -> Option<String> {
        match self {
            Source::HuggingFace { repo, revision, folder } if folder.is_empty() => {
                Some(format!("https://huggingface.co/{repo}/resolve/{revision}/{path}"))
            }
            Source::HuggingFace { repo, revision, folder } => {
                Some(format!("https://huggingface.co/{repo}/resolve/{revision}/{folder}/{path}"))
            }
            Source::File { url } => Some(url.clone()),
            Source::Archive { .. } => None,
        }
    }
}

/// A set of files that never change, stored once under `artifacts/<digest>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub source: Source,
    /// The [`digest`] of `files`. A test checks it.
    pub digest: String,
    /// The files, sorted by path.
    pub files: Vec<File>,
}

impl Artifact {
    /// The size of all files, in bytes.
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// An artifact of a descriptor and what the engine does with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    /// What the artifact is for in this format, for example `main`, `preview`, or `tokenizer`.
    pub role: String,
    /// The name of the artifact in [`Descriptors::artifacts`].
    pub artifact: String,
}

/// The artifacts of one model in one format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descriptor {
    pub model: String,
    pub format: String,
    /// Goes up by one each time the artifacts change. It never goes down.
    pub revision: u32,
    pub parts: Vec<Part>,
}

/// The data of `descriptors.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descriptors {
    /// Each artifact by a name that people can read. Two descriptors that use
    /// one name share the artifact.
    pub artifacts: BTreeMap<String, Artifact>,
    /// Sorted by model, then format.
    pub descriptors: Vec<Descriptor>,
}

impl Descriptors {
    /// The descriptor of `model` in `format`.
    pub fn find(&self, model: &str, format: &str) -> Option<&Descriptor> {
        self.descriptors.iter().find(|d| d.model == model && d.format == format)
    }

    /// The artifacts of `descriptor`, with their roles.
    pub fn parts<'a>(&'a self, descriptor: &'a Descriptor) -> impl Iterator<Item = (&'a str, &'a Artifact)> + 'a {
        descriptor.parts.iter().map(|p| {
            let artifact = self.artifacts.get(&p.artifact).unwrap_or_else(|| panic!("no artifact {}", p.artifact));
            (p.role.as_str(), artifact)
        })
    }
}

/// The descriptors that the catalog builds in.
pub fn all() -> &'static Descriptors {
    static DATA: OnceLock<Descriptors> = OnceLock::new();
    DATA.get_or_init(|| serde_json::from_str(DATA_JSON).expect("descriptors.json is valid"))
}

const DATA_JSON: &str = include_str!("descriptors.json");

/// The descriptors of `formats` only, with the artifacts that they use.
pub fn of_formats(data: &Descriptors, formats: &[&str]) -> Descriptors {
    let descriptors: Vec<Descriptor> = data.descriptors.iter().filter(|d| formats.contains(&d.format.as_str())).cloned().collect();
    let artifacts = descriptors
        .iter()
        .flat_map(|d| &d.parts)
        .map(|p| (p.artifact.clone(), data.artifacts[&p.artifact].clone()))
        .collect();
    Descriptors { artifacts, descriptors }
}

/// `data` as JSON with one line for each file, in the form that
/// `scripts/model-descriptors.py` writes. A test checks that `descriptors.json`
/// is in this form, so a change to it by hand shows.
pub fn render(data: &Descriptors) -> String {
    let s = |v: &str| serde_json::to_string(v).expect("a string is JSON");
    let mut out = String::from("{\n  \"artifacts\": {\n");
    for (i, (name, a)) in data.artifacts.iter().enumerate() {
        let source = match &a.source {
            Source::HuggingFace { repo, revision, folder } => {
                let folder = if folder.is_empty() { String::new() } else { format!(", \"folder\": {}", s(folder)) };
                format!("{{\"kind\": \"hugging_face\", \"repo\": {}, \"revision\": {}{folder}}}", s(repo), s(revision))
            }
            Source::Archive { url } => format!("{{\"kind\": \"archive\", \"url\": {}}}", s(url)),
            Source::File { url } => format!("{{\"kind\": \"file\", \"url\": {}}}", s(url)),
        };
        out += &format!("    {}: {{\n      \"source\": {source},\n      \"digest\": {},\n      \"files\": [\n", s(name), s(&a.digest));
        for (j, f) in a.files.iter().enumerate() {
            let comma = if j + 1 < a.files.len() { "," } else { "" };
            out += &format!("        {{\"path\": {}, \"size\": {}, \"sha256\": {}}}{comma}\n", s(&f.path), f.size, s(&f.sha256));
        }
        out += if i + 1 < data.artifacts.len() { "      ]\n    },\n" } else { "      ]\n    }\n" };
    }
    out += "  },\n  \"descriptors\": [\n";
    for (i, d) in data.descriptors.iter().enumerate() {
        let parts: Vec<String> =
            d.parts.iter().map(|p| format!("{{\"role\": {}, \"artifact\": {}}}", s(&p.role), s(&p.artifact))).collect();
        let comma = if i + 1 < data.descriptors.len() { "," } else { "" };
        out += &format!(
            "    {{\"model\": {}, \"format\": {}, \"revision\": {}, \"parts\": [{}]}}{comma}\n",
            s(&d.model),
            s(&d.format),
            d.revision,
            parts.join(", ")
        );
    }
    out + "  ]\n}\n"
}

/// A path or a file list that the store cannot take.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Invalid {
    #[error("the path {0:?} is not a plain relative path in / form")]
    Path(String),
    #[error("the path {0:?} is in the list twice (letter case ignored)")]
    Twice(String),
    #[error("the hash of {0:?} is not 64 lowercase hex digits")]
    Hash(String),
}

/// Check that `path` is a plain path relative to an artifact: parts joined by
/// `/`, no empty part, no `.` or `..`, no `\`, `:`, or control character.
/// An archive or a repository with another path is refused.
pub fn check_path(path: &str) -> Result<(), Invalid> {
    let plain = !path.is_empty()
        && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
        && !path.chars().any(|c| c == '\\' || c == ':' || c.is_control());
    if plain { Ok(()) } else { Err(Invalid::Path(path.into())) }
}

/// The digest of an artifact: the SHA-256, in lowercase hex, of its canonical
/// file list. The list has one entry for each file, sorted by the bytes of the
/// path. An entry is the path, a NUL byte, the size in decimal, a NUL byte, the
/// SHA-256 of the file in lowercase hex, and a line feed. The source is not
/// part of it, so the same files from two revisions are one artifact.
pub fn digest(files: &[File]) -> Result<String, Invalid> {
    let mut sorted: Vec<&File> = files.iter().collect();
    sorted.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    let mut seen = std::collections::HashSet::new();
    let mut hasher = Sha256::new();
    for file in sorted {
        check_path(&file.path)?;
        // Two paths that differ only in letter case are one file on macOS and Windows.
        if !seen.insert(file.path.to_lowercase()) {
            return Err(Invalid::Twice(file.path.clone()));
        }
        let hex = file.sha256.len() == 64 && file.sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if !hex {
            return Err(Invalid::Hash(file.path.clone()));
        }
        hasher.update(file.path.as_bytes());
        hasher.update(b"\0");
        hasher.update(file.size.to_string().as_bytes());
        hasher.update(b"\0");
        hasher.update(file.sha256.as_bytes());
        hasher.update(b"\n");
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

// What the script needs.

/// The sherpa-onnx release with the speech model archives and the VAD file.
const SHERPA_ASR: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models";
/// The sherpa-onnx release with the segmentation archives of diarization.
const SHERPA_SEGMENTATION: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models";
/// The sherpa-onnx release with the speaker embedding files. The tag is spelled so on GitHub.
const SHERPA_EMBEDDING: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models";
/// The Hugging Face repository with the whisper.cpp GGML files.
const WHISPER_CPP_REPO: &str = "ggerganov/whisper.cpp";

/// A source before the script pins it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WantedSource {
    /// Files of a Hugging Face repository. `rev` is a branch or a commit. The
    /// script takes each file under `folder` whose path (relative to `folder`)
    /// is in `include`, or starts with an entry of `include` and `/`.
    HuggingFace { repo: String, rev: String, folder: String, include: Vec<String> },
    Archive { url: String },
    File { url: String },
}

/// An artifact of a [`Wanted`] descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WantedPart {
    pub role: String,
    /// The name of the artifact. Two parts with one name must have the same source.
    pub artifact: String,
    pub source: WantedSource,
}

/// A descriptor before the script pins its sources and hashes its files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wanted {
    pub model: String,
    pub format: String,
    pub parts: Vec<WantedPart>,
}

/// The descriptors that the models of the catalogs of all engines need, sorted
/// by model, then format. A model id with one format on two systems (the
/// sherpa-onnx models of Linux and Windows) is one descriptor.
pub fn wanted() -> Vec<Wanted> {
    let mut all: BTreeMap<(String, String), Wanted> = BTreeMap::new();
    for engine in [Engine::Swift, Engine::Rust(System::Linux), Engine::Rust(System::Windows)] {
        for m in models(engine) {
            let Some(wanted) = wanted_for(&m) else { continue };
            let key = (wanted.model.clone(), wanted.format.clone());
            if let Some(have) = all.get(&key) {
                assert_eq!(have, &wanted, "{} needs other files on another system in one format", m.id);
            } else {
                all.insert(key, wanted);
            }
        }
    }
    all.into_values().collect()
}

/// The descriptor that `m` needs. None for a model without files in the store.
fn wanted_for(m: &ModelInfo) -> Option<Wanted> {
    let format = format(&m.engine)?;
    let parts = match &m.engine {
        EngineKind::Sherpa { archive, preview_archive, .. } => {
            let mut parts = vec![sherpa_archive("main", SHERPA_ASR, archive)];
            parts.extend(preview_archive.iter().map(|p| sherpa_archive("preview", SHERPA_ASR, p)));
            parts
        }
        EngineKind::SherpaVad { file, .. } => vec![sherpa_file("main", SHERPA_ASR, file)],
        EngineKind::SherpaDiarization { segmentation, embedding } => vec![
            sherpa_archive("segmentation", SHERPA_SEGMENTATION, segmentation),
            sherpa_file("embedding", SHERPA_EMBEDDING, embedding),
        ],
        EngineKind::WhisperCpp { file } => {
            vec![hugging_face("main", &format!("huggingface/{WHISPER_CPP_REPO}/{file}"), WHISPER_CPP_REPO, "main", "", &[file])]
        }
        EngineKind::ParakeetCpp { repo, revision, file, preview_archive } => {
            let mut parts = vec![hugging_face("main", &format!("huggingface/{repo}/{file}"), repo, revision, "", &[file])];
            parts.extend(preview_archive.iter().map(|p| sherpa_archive("preview", SHERPA_ASR, p)));
            parts
        }
        EngineKind::Whisper { variant } => whisperkit(variant),
        engine => fluidaudio(engine)?,
    };
    Some(Wanted { model: m.id.as_str().into(), format: format.into(), parts })
}

fn sherpa_archive(role: &str, release: &str, archive: &str) -> WantedPart {
    WantedPart {
        role: role.into(),
        artifact: format!("sherpa-onnx/{archive}"),
        source: WantedSource::Archive { url: format!("{release}/{archive}.tar.bz2") },
    }
}

fn sherpa_file(role: &str, release: &str, file: &str) -> WantedPart {
    WantedPart {
        role: role.into(),
        artifact: format!("sherpa-onnx/{file}"),
        source: WantedSource::File { url: format!("{release}/{file}") },
    }
}

fn hugging_face(role: &str, artifact: &str, repo: &str, rev: &str, folder: &str, include: &[&str]) -> WantedPart {
    WantedPart {
        role: role.into(),
        artifact: artifact.into(),
        source: WantedSource::HuggingFace {
            repo: repo.into(),
            rev: rev.into(),
            folder: folder.into(),
            include: include.iter().map(|s| s.to_string()).collect(),
        },
    }
}

/// The WhisperKit model folder of `variant` and the tokenizer that it needs.
/// `WhisperKit.download` takes every file under `<variant>/` of the repository
/// (glob `*<variant>/*`), and `ModelUtilities.loadTokenizer` takes these three
/// files of the tokenizer repository (argmax-oss-swift 1.1.0).
fn whisperkit(variant: &str) -> Vec<WantedPart> {
    const REPO: &str = "argmaxinc/whisperkit-coreml";
    let model = hugging_face("model", &format!("huggingface/{REPO}/{variant}"), REPO, "main", variant, &[]);
    let tokenizer = whisper_tokenizer(variant);
    let files = ["config.json", "tokenizer.json", "tokenizer_config.json"];
    vec![model, hugging_face("tokenizer", &format!("huggingface/{tokenizer}"), &tokenizer, "main", "", &files)]
}

/// The tokenizer repository of a WhisperKit variant, as `ModelStore.whisperTokenizer`
/// in the Swift engine finds it. Distil-Whisper large-v3 and turbo use the large-v3 tokenizer.
pub(crate) fn whisper_tokenizer(variant: &str) -> String {
    let name = variant.to_lowercase();
    let size = ["large-v3", "large-v2", "medium", "small", "base", "tiny"]
        .into_iter()
        .find(|size| name.contains(size))
        .unwrap_or_else(|| panic!("the WhisperKit variant {variant} has no known tokenizer"));
    let english = if name.contains(&format!("{size}.en")) { ".en" } else { "" };
    format!("openai/whisper-{size}{english}")
}

/// The files of a FluidAudio model: the files that the Swift engine downloads
/// today with FluidAudio 0.17.5 (`ModelHub.download`, `CtcModels.download`) or
/// with its own downloader (`FluidSpeechMini.swift`). `ModelHub` takes each
/// required bundle, and each `.json` and `.txt` file next to the bundles; the
/// lists name them, so a new file on a branch does not change an artifact.
fn fluidaudio(engine: &EngineKind) -> Option<Vec<WantedPart>> {
    let fluid = |role: &str, name: &str, repo: &str, folder: &str, include: &[&str]| {
        let repo = format!("FluidInference/{repo}");
        hugging_face(role, &format!("huggingface/{repo}/{name}"), &repo, "main", folder, include)
    };
    let tdt = |repo: &str, joint: &str, vocabularies: &[&str]| {
        let mut include = vec!["Preprocessor.mlmodelc", "Encoder.mlmodelc", "Decoder.mlmodelc", joint, "config.json"];
        include.extend(vocabularies);
        vec![fluid("model", "tdt", repo, "", &include)]
    };
    let v3_vocabularies = ["parakeet_vocab.json", "parakeet_v3_vocab.json"];
    Some(match engine {
        EngineKind::ParakeetUnified { streaming_tier } => {
            let streaming = format!("parakeet_unified_encoder_streaming_{streaming_tier}_int8.mlmodelc");
            let model = [
                "parakeet_unified_encoder_int8.mlmodelc",
                streaming.as_str(),
                "parakeet_unified_decoder.mlmodelc",
                "parakeet_unified_joint_decision_single_step.mlmodelc",
                "vocab.json",
                "metadata.json",
                "config.json",
            ];
            let ctc = [
                "AudioEncoder.mlmodelc",
                "MelSpectrogram.mlmodelc",
                "config.json",
                "ctc_head_metadata.json",
                "special_tokens_map.json",
                "tokenizer.json",
                "tokenizer_config.json",
                "vocab.json",
            ];
            vec![
                fluid("model", &format!("int8-{streaming_tier}"), "parakeet-unified-en-0.6b-coreml", "", &model),
                fluid("ctc", "vocabulary", "parakeet-ctc-110m-coreml", "", &ctc),
            ]
        }
        EngineKind::ParakeetEou => {
            let include = ["streaming_encoder.mlmodelc", "decoder.mlmodelc", "joint_decision.mlmodelc", "vocab.json"];
            vec![fluid("model", "160ms", "parakeet-realtime-eou-120m-coreml", "160ms", &include)]
        }
        EngineKind::ParakeetTdt { version } => match version.as_str() {
            "v2" => tdt("parakeet-tdt-0.6b-v2-coreml", "JointDecision.mlmodelc", &["parakeet_vocab.json"]),
            "v3" => tdt("parakeet-tdt-0.6b-v3-coreml", "JointDecisionv3.mlmodelc", &v3_vocabularies),
            "ultra" => tdt("parakeet-ultra-coreml", "JointDecisionv3.mlmodelc", &v3_vocabularies),
            "redux" => tdt("parakeet-redux-coreml", "JointDecisionv3.mlmodelc", &v3_vocabularies),
            "phonon2" => tdt("phonon-2-coreml", "JointDecisionv3.mlmodelc", &v3_vocabularies),
            "tdt_ctc_110m" => {
                let include =
                    ["Preprocessor.mlmodelc", "Decoder.mlmodelc", "JointDecision.mlmodelc", "config.json", "parakeet_vocab.json"];
                vec![fluid("model", "tdt", "parakeet-tdt-ctc-110m-coreml", "", &include)]
            }
            "ja" => {
                let include = [
                    "Preprocessor.mlmodelc",
                    "Encoder.mlmodelc",
                    "Decoderv2.mlmodelc",
                    "Jointerv2.mlmodelc",
                    "config.json",
                    "metadata.json",
                    "requirements.txt",
                    "vocab.json",
                ];
                vec![fluid("model", "tdt", "parakeet-0.6b-ja-coreml", "", &include)]
            }
            "fluid_mini" => {
                // Pinned in FluidSpeechMini.swift, with its list of files.
                const REPO: &str = "FluidVoice-Labs/fluid-speech-mini-coreml";
                let include = [
                    "Preprocessor.mlmodelc",
                    "Encoder.mlmodelc",
                    "Decoder.mlmodelc",
                    "JointDecision.mlmodelc",
                    "parakeet_vocab.json",
                    "parakeet_v3_vocab.json",
                    "config.json",
                    "LICENSE",
                    "NOTICE",
                ];
                let artifact = format!("huggingface/{REPO}/coreml");
                vec![hugging_face("model", &artifact, REPO, "d81c3160dfe5a5e1f8f7267164a07d30617a3a1a", "", &include)]
            }
            other => panic!("no files for the FluidAudio Parakeet TDT version {other}"),
        },
        EngineKind::Nemotron { chunk_ms } => {
            let include = [
                "preprocessor.mlmodelc",
                "encoder/encoder_int8.mlmodelc",
                "decoder.mlmodelc",
                "joint.mlmodelc",
                "decoder_joint.mlmodelc",
                "tokenizer.json",
                "metadata.json",
            ];
            let folder = format!("nemotron_coreml_{chunk_ms}ms");
            vec![fluid("model", &folder, "nemotron-speech-streaming-en-0.6b-coreml", &folder, &include)]
        }
        EngineKind::NemotronMultilingual { chunk_ms } => {
            // Each file of the folder but the `.mlpackage` sources.
            let include = [
                "preprocessor.mlmodelc",
                "encoder.mlmodelc",
                "decoder.mlmodelc",
                "joint.mlmodelc",
                "decoder_joint.mlmodelc",
                "tokenizer.json",
                "metadata.json",
            ];
            let folder = format!("multilingual/{chunk_ms}ms");
            vec![fluid("model", &folder, "Nemotron-3.5-ASR-Streaming-Multilingual-0.6b-CoreML", &folder, &include)]
        }
        EngineKind::Cohere => {
            // `ModelHub` takes `vocab.json` from the root of the repository, because `q8/` has none.
            // The engine keeps both artifacts in one folder.
            let include = ["cohere_encoder.mlmodelc", "cohere_decoder_cache_external_v2.mlmodelc"];
            vec![
                fluid("model", "q8", "cohere-transcribe-03-2026-coreml", "q8", &include),
                fluid("vocabulary", "vocab", "cohere-transcribe-03-2026-coreml", "", &["vocab.json"]),
            ]
        }
        EngineKind::Canary => {
            let include = [
                "Preprocessor.mlmodelc",
                "Projection.mlmodelc",
                "EncoderInt4.mlmodelc",
                "DecoderInt4.mlmodelc",
                "config.json",
                "metadata.json",
                "vocab.json",
            ];
            vec![fluid("model", "int4", "canary-1b-v2-coreml", "", &include)]
        }
        EngineKind::SenseVoice => {
            let include = ["SenseVoicePreprocessor.mlmodelc", "SenseVoiceSmall_fp32.mlmodelc", "config.json", "vocab.json"];
            vec![fluid("model", "fp32", "sensevoice-small-coreml", "", &include)]
        }
        EngineKind::Paraformer => {
            // `ModelHub` takes the int8 bundles too, although the engine loads the others.
            let include = [
                "ParaformerPreprocessor.mlmodelc",
                "ParaformerEncoder.mlmodelc",
                "ParaformerEncoder_int8.mlmodelc",
                "ParaformerCifAlphas.mlmodelc",
                "ParaformerDecoder.mlmodelc",
                "ParaformerDecoder_int8.mlmodelc",
                "config.json",
                "vocab.json",
            ];
            vec![fluid("model", "model", "paraformer-large-zh-coreml", "", &include)]
        }
        EngineKind::FluidVad => {
            let include = ["silero-vad-unified-256ms-v6.2.1.mlmodelc", "config.json"];
            vec![fluid("model", "unified-256ms-v6.2.1", "silero-vad-coreml", "", &include)]
        }
        EngineKind::FluidDiarization => {
            // FluidAudio pins this repository (`ModelNames.swift`, `Repo.diarizer`).
            const REPO: &str = "FluidInference/speaker-diarization-coreml";
            let include = [
                "Segmentation.mlmodelc",
                "FBank.mlmodelc",
                "Embedding.mlmodelc",
                "PldaRho.mlmodelc",
                "plda-parameters.json",
                "config.json",
                "provenance.json",
                "xvector-transform.json",
            ];
            let artifact = format!("huggingface/{REPO}/offline");
            vec![hugging_face("model", &artifact, REPO, "df2625ac79a7ac6b65ad868fee6d80f320da4232", "", &include)]
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models_onnx;

    fn file(path: &str, size: u64, sha256: &str) -> File {
        File { path: path.into(), size, sha256: sha256.into() }
    }

    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    /// The vector comes from `printf` and `shasum -a 256` (see the comment), not from this code.
    #[test]
    fn the_digest_has_a_fixed_test_vector() {
        // printf 'a/b.txt\0003\000ba78...15ad\nempty\0000\000e3b0...b855\n' | shasum -a 256
        let files = [file("empty", 0, EMPTY), file("a/b.txt", 3, ABC)];
        assert_eq!(digest(&files).unwrap(), "6e06e6ee31cad84ba1722ef4b4d6ab92e82e42ce047bbd24f895be38611995cb");
        assert_eq!(digest(&[]).unwrap(), EMPTY, "no files: the hash of no bytes");
    }

    #[test]
    fn the_digest_sorts_by_bytes_and_ignores_the_source() {
        let a = [file("B", 1, ABC), file("a", 2, EMPTY), file("a/x", 3, ABC)];
        let b = [file("a/x", 3, ABC), file("a", 2, EMPTY), file("B", 1, ABC)];
        assert_eq!(digest(&a).unwrap(), digest(&b).unwrap());
        // A change of a size or a hash changes the digest.
        let c = [file("B", 1, ABC), file("a", 2, EMPTY), file("a/x", 4, ABC)];
        assert_ne!(digest(&a).unwrap(), digest(&c).unwrap());
        let d = [file("B", 1, EMPTY), file("a", 2, EMPTY), file("a/x", 3, ABC)];
        assert_ne!(digest(&a).unwrap(), digest(&d).unwrap());
    }

    #[test]
    fn the_digest_refuses_bad_paths_and_hashes() {
        for bad in ["", "/abs", "a//b", "a/", "./a", "a/../b", "..", "a\\b", "C:x", "a\nb", "a\0b"] {
            assert_eq!(digest(&[file(bad, 1, ABC)]), Err(Invalid::Path(bad.into())), "{bad:?}");
        }
        assert_eq!(digest(&[file("A", 1, ABC), file("a", 1, ABC)]), Err(Invalid::Twice("a".into())));
        assert_eq!(digest(&[file("a", 1, &ABC.to_uppercase())]), Err(Invalid::Hash("a".into())));
        assert_eq!(digest(&[file("a", 1, "00")]), Err(Invalid::Hash("a".into())));
        assert!(check_path("a/b.c/d-e_f").is_ok());
    }

    #[test]
    fn the_formats_cover_every_local_model() {
        for engine in [Engine::Swift, Engine::Rust(System::Linux), Engine::Rust(System::Windows)] {
            for m in models(engine) {
                let format = format(&m.engine);
                assert_eq!(format.is_none(), m.engine == EngineKind::AppleSpeech, "{engine:?} {}", m.id);
            }
        }
    }

    #[test]
    fn the_data_file_is_in_the_form_of_the_script() {
        assert!(render(all()) == DATA_JSON, "descriptors.json was changed by hand; run scripts/model-descriptors.py");
    }

    #[test]
    fn the_data_is_consistent() {
        let data = all();
        for (name, artifact) in &data.artifacts {
            assert_eq!(digest(&artifact.files).as_deref(), Ok(artifact.digest.as_str()), "{name}");
            let sorted = artifact.files.windows(2).all(|w| w[0].path.as_bytes() < w[1].path.as_bytes());
            assert!(sorted && !artifact.files.is_empty(), "{name}: files must be sorted and not empty");
            match &artifact.source {
                Source::HuggingFace { repo, revision, folder } => {
                    assert!(repo.split('/').count() == 2, "{name}: repo {repo}");
                    let commit = revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit());
                    assert!(commit, "{name}: {revision} is not a commit");
                    assert!(folder.is_empty() || check_path(folder).is_ok(), "{name}: folder {folder}");
                }
                Source::Archive { url } => assert!(url.starts_with("https://") && url.ends_with(".tar.bz2"), "{name}"),
                Source::File { url } => {
                    assert!(url.starts_with("https://") && artifact.files.len() == 1, "{name}");
                    assert!(url.ends_with(&format!("/{}", artifact.files[0].path)), "{name}");
                }
            }
        }
        let mut used = std::collections::BTreeSet::new();
        let mut keys = Vec::new();
        for d in &data.descriptors {
            assert!(d.revision >= 1 && !d.parts.is_empty(), "{} {}", d.model, d.format);
            for part in &d.parts {
                assert!(data.artifacts.contains_key(&part.artifact), "{} {}: no artifact {}", d.model, d.format, part.artifact);
                used.insert(part.artifact.as_str());
            }
            keys.push((d.model.as_str(), d.format.as_str()));
        }
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "the descriptors must be sorted and unique");
        let unused: Vec<_> = data.artifacts.keys().filter(|k| !used.contains(k.as_str())).collect();
        assert!(unused.is_empty(), "artifacts that no descriptor uses: {unused:?}");
    }

    /// A descriptor in the data has the parts that the catalog wants, from the same sources.
    #[test]
    fn the_data_matches_the_catalog() {
        let data = all();
        let wanted = wanted();
        for d in &data.descriptors {
            let w = wanted
                .iter()
                .find(|w| w.model == d.model && w.format == d.format)
                .unwrap_or_else(|| panic!("{} {} is not in a catalog; run scripts/model-descriptors.py", d.model, d.format));
            let roles: Vec<(&str, &str)> = d.parts.iter().map(|p| (p.role.as_str(), p.artifact.as_str())).collect();
            let want: Vec<(&str, &str)> = w.parts.iter().map(|p| (p.role.as_str(), p.artifact.as_str())).collect();
            assert_eq!(roles, want, "{} {}", d.model, d.format);
            for part in &w.parts {
                let source = &data.artifacts[&part.artifact].source;
                let same = match (&part.source, source) {
                    (WantedSource::HuggingFace { repo, rev, folder, .. }, Source::HuggingFace { repo: r, revision, folder: f }) => {
                        // A branch is pinned to a commit; a commit stays.
                        repo == r && folder == f && (rev.len() != 40 || rev == revision)
                    }
                    (WantedSource::Archive { url }, Source::Archive { url: u }) | (WantedSource::File { url }, Source::File { url: u }) => url == u,
                    _ => false,
                };
                assert!(same, "{} {}: the source of {} changed; run scripts/model-descriptors.py", d.model, d.format, part.artifact);
            }
        }
    }

    /// Each model of each catalog with files in the store has a descriptor.
    #[test]
    fn every_local_model_has_a_descriptor() {
        let data = all();
        let missing: Vec<String> = wanted()
            .into_iter()
            .filter(|w| data.find(&w.model, &w.format).is_none())
            .map(|w| format!("{} ({})", w.model, w.format))
            .collect();
        assert!(missing.is_empty(), "models without a descriptor: {missing:?}; run scripts/model-descriptors.py");
    }

    #[test]
    fn sherpa_onnx_models_have_their_archives() {
        let data = all();
        let d = data.find("parakeet-tdt-v2", SHERPA_ONNX).expect("parakeet-tdt-v2");
        let parts: Vec<_> = data.parts(d).map(|(role, a)| (role, a.source.clone())).collect();
        assert_eq!(
            parts,
            [
                ("main", Source::Archive { url: format!("{SHERPA_ASR}/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2") }),
                ("preview", Source::Archive { url: format!("{SHERPA_ASR}/{}.tar.bz2", models_onnx::PREVIEW_ARCHIVE) }),
            ]
        );
        // The preview archive is one artifact for every model that uses it.
        let kroko = data.find("kroko-en", SHERPA_ONNX).expect("kroko-en");
        assert_eq!(kroko.parts[0].artifact, d.parts[1].artifact);
    }
}
