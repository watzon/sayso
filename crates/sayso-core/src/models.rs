//! The speech model catalog (plan §3, "Models").
//!
//! [`catalog`] lists the local models of this system: the Swift engine's
//! models on macOS, the portable engine's models (`portable`) on Windows, and
//! the sherpa-onnx models ([`crate::models_onnx`]) on Linux. Cloud models
//! come from the configured speech providers (see [`crate::speech`]).

mod benchmark;
mod portable;

use crate::config::PreviewChoice;
use crate::languages;
use sayso_i18n::t;
use crate::speech::Speech;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelId(pub String);

impl ModelId {
    pub fn new(s: impl Into<String>) -> Self {
        ModelId(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which engine family runs the model. The engine sidecar maps this to a library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineKind {
    /// FluidAudio Parakeet Unified: batch, streaming, and the CTC vocabulary helper.
    ParakeetUnified { streaming_tier: String },
    /// FluidAudio Parakeet EOU 120M: streaming. The final pass streams the whole recording.
    ParakeetEou,
    /// FluidAudio Parakeet TDT family, batch only. `version` is one of "v2",
    /// "v3", "ultra", "redux", "phonon2", "tdt_ctc_110m", "ja".
    ParakeetTdt { version: String },
    /// FluidAudio Nemotron streaming, English. One model does preview and final pass.
    Nemotron { chunk_ms: u32 },
    /// FluidAudio Nemotron streaming with the full multilingual vocabulary.
    NemotronMultilingual { chunk_ms: u32 },
    /// FluidAudio Cohere Transcribe (int8 encoder).
    Cohere,
    /// FluidAudio Canary 1B v2 (int4).
    Canary,
    /// FluidAudio SenseVoice Small (int8 encoder).
    SenseVoice,
    /// FluidAudio Paraformer large, Mandarin (int8).
    Paraformer,
    /// WhisperKit with a variant from `argmaxinc/whisperkit-coreml`.
    Whisper { variant: String },
    /// Apple's SpeechAnalyzer model. macOS downloads and stores its files.
    AppleSpeech,
    /// Portable engine: whisper.cpp with one GGML file, `file` in
    /// `ggerganov/whisper.cpp` on Hugging Face.
    WhisperCpp { file: String },
    /// Portable engine: an ONNX model through transcribe-rs. `family` is the
    /// transcribe-rs engine ("parakeet", "moonshine", ...). `repo` is the
    /// Hugging Face repository with the files.
    Onnx { family: String, repo: String },
    /// A speech provider's model. Rust sends the audio; the sidecar never sees it.
    Remote { provider: String, model: String },
    /// A sherpa-onnx model in the Rust engine `sayso-engine` (Linux). `archive`
    /// is the file name, without `.tar.bz2`, in the sherpa-onnx `asr-models`
    /// release. `recipe` tells the engine how to load it: "parakeet_tdt",
    /// "parakeet_tdt_ctc", "zipformer_streaming", "whisper", "sense_voice", or
    /// "moonshine". `preview_archive` is a streaming model that comes with it
    /// for the live preview.
    Sherpa { recipe: String, archive: String, preview_archive: Option<String> },
    /// A Parakeet TDT GGUF that parakeet.cpp runs through Vulkan in the Rust
    /// engine `sayso-engine` (Linux); it loads only on a discrete GPU. `file` is
    /// in the Hugging Face repository `repo` at the commit `revision`.
    /// `preview_archive` is a sherpa-onnx streaming model, as for `Sherpa`.
    ParakeetCpp { repo: String, revision: String, file: String, preview_archive: Option<String> },
}

/// A group of models in the Models page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Parakeet,
    Whisper,
    Other,
    Cloud,
}

impl EngineKind {
    pub fn family(&self) -> Family {
        match self {
            EngineKind::ParakeetUnified { .. } | EngineKind::ParakeetEou | EngineKind::ParakeetTdt { .. } => Family::Parakeet,
            EngineKind::Onnx { family, .. } if family == "parakeet" => Family::Parakeet,
            EngineKind::Whisper { .. } | EngineKind::WhisperCpp { .. } => Family::Whisper,
            EngineKind::Sherpa { recipe, .. } if recipe.starts_with("parakeet") => Family::Parakeet,
            EngineKind::Sherpa { recipe, .. } if recipe == "whisper" => Family::Whisper,
            EngineKind::ParakeetCpp { .. } => Family::Parakeet,
            EngineKind::Remote { .. } => Family::Cloud,
            _ => Family::Other,
        }
    }

    /// The library that runs the model, for the Models page.
    pub fn runtime(&self) -> &'static str {
        match self {
            EngineKind::Whisper { .. } => "WhisperKit",
            EngineKind::Remote { .. } => "Cloud",
            EngineKind::AppleSpeech => "macOS",
            EngineKind::WhisperCpp { .. } => "whisper.cpp",
            EngineKind::Onnx { .. } => "ONNX Runtime",
            EngineKind::Sherpa { .. } => "sherpa-onnx",
            EngineKind::ParakeetCpp { .. } => "parakeet.cpp",
            _ => "FluidAudio",
        }
    }

    /// The model runs only on a GPU: on Linux, a `ParakeetCpp` model. The app hides it on a computer
    /// without a discrete Vulkan GPU (`shell::model_runs_here`).
    pub fn needs_gpu(&self) -> bool {
        matches!(self, EngineKind::ParakeetCpp { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: ModelId,
    pub name: String,
    pub vendor: String,
    pub description: String,
    pub engine: EngineKind,
    /// Download size. Zero for a cloud model, and for Apple Speech, whose files macOS manages.
    pub size_bytes: u64,
    /// Language codes the model can be set to (see [`crate::languages`]).
    pub languages: Vec<String>,
    /// Can do the final pass.
    pub final_pass: bool,
    /// Can stream a live preview.
    pub live_preview: bool,
    /// Uses dictionary words (vocabulary boosting, a prompt, or keyterms).
    pub vocabulary: bool,
    /// 1 (slow) to 5 (fast), for the speed meter in Models.
    pub speed: u8,
    /// Word error rate in percent, measured by Sayso (`models/benchmark.rs`). None
    /// for a model that was not measured.
    pub wer_percent: Option<f32>,
    /// The word error rate of the live preview, when it differs from the final pass.
    pub preview_wer_percent: Option<f32>,
    /// The text has punctuation and capitals.
    pub punctuation: bool,
    /// Measured speed on a Mac: seconds of audio per second of processing.
    pub rtfx: Option<u32>,
    /// The page of the upstream model.
    pub card_url: Option<String>,
    /// The credit that the license of the model asks for.
    pub credit: Option<String>,
    pub recommended: bool,
    /// The first macOS version that can run the model. Zero on other systems.
    pub min_macos: u32,
}

impl ModelInfo {
    /// True for a speech provider's model.
    pub fn is_remote(&self) -> bool {
        matches!(self.engine, EngineKind::Remote { .. })
    }

    /// True when the user can choose a language for this model.
    pub fn is_multilingual(&self) -> bool {
        self.languages.len() > 1
    }

    /// "English", "Japanese", "25 languages", or "Many languages" for a
    /// cloud model, in the interface language.
    pub fn language_label(&self) -> String {
        match self.languages.as_slice() {
            _ if self.is_remote() => t!("core.models.many_languages"),
            [one] => languages::display(one),
            many => t!("core.models.languages", count = many.len()),
        }
    }

    /// The time between two live preview updates.
    pub fn preview_interval_ms(&self) -> Option<u32> {
        if !self.live_preview {
            return None;
        }
        match &self.engine {
            // One chunk of the streaming tier "70_2_2": 2 frames of 80 ms.
            EngineKind::ParakeetUnified { .. } | EngineKind::ParakeetEou => Some(160),
            EngineKind::Nemotron { chunk_ms } | EngineKind::NemotronMultilingual { chunk_ms } => Some(*chunk_ms),
            _ => None,
        }
    }

    /// True when the model understands `language`, a code or "auto".
    pub fn understands(&self, language: &str) -> bool {
        if language == languages::AUTO { self.is_multilingual() } else { self.languages.iter().any(|l| l == language) }
    }

    /// The language to send to the model for a `dictation.language` setting:
    /// the setting when the model supports it, else "auto" for a
    /// multilingual model, else the model's only language.
    pub fn language_for(&self, setting: &str) -> String {
        if self.languages.iter().any(|l| l == setting) {
            setting.to_string()
        } else if self.is_multilingual() {
            languages::AUTO.to_string()
        } else {
            self.languages.first().cloned().unwrap_or_else(|| languages::AUTO.to_string())
        }
    }
}

const MB: u64 = 1_000_000;

pub fn default_model() -> ModelId {
    if cfg!(target_os = "macos") {
        ModelId::new("parakeet-unified-en")
    } else if cfg!(windows) {
        ModelId::new(portable::DEFAULT_MODEL)
    } else {
        crate::models_onnx::default_model()
    }
}

/// The local model to suggest for a dictation language: the default model
/// for English, a Parakeet model for the other European languages it
/// knows, a model trained on Chinese, Cantonese, Japanese, or Korean where
/// this system has one, and Whisper large-v3 turbo for every other
/// language. For a language Whisper does not know, the first model that
/// knows it and `runs_here`.
pub fn suggested_for(language: &str, runs_here: impl Fn(&ModelInfo) -> bool) -> ModelId {
    let european = languages::parakeet_v3().iter().any(|l| l == language);
    let id = match language {
        "en" | languages::AUTO => return default_model(),
        _ if european => by_os("parakeet-ultra", "parakeet-tdt-v3", "parakeet-tdt-v3"),
        "ja" => by_os("parakeet-ja", "whisper-large-v3-turbo", "sensevoice-small"),
        "zh" | "yue" | "ko" => by_os("sensevoice-small", "whisper-large-v3-turbo", "sensevoice-small"),
        _ => "whisper-large-v3-turbo",
    };
    let id = ModelId::new(id);
    let usable = |m: &ModelInfo| m.final_pass && m.understands(language) && runs_here(m);
    if find(&id).is_some_and(|m| usable(&m)) {
        return id;
    }
    catalog().into_iter().find(usable).map_or(id, |m| m.id)
}

/// The text for macOS, for Windows, or for Linux.
fn by_os(mac: &'static str, windows: &'static str, linux: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        mac
    } else if cfg!(windows) {
        windows
    } else {
        linux
    }
}

/// The small streaming model that gives a live preview to models without one.
pub fn preview_fallback() -> ModelId {
    if cfg!(target_os = "macos") {
        ModelId::new("parakeet-eou-120m")
    } else if cfg!(windows) {
        ModelId::new(portable::PREVIEW_FALLBACK)
    } else {
        crate::models_onnx::preview_fallback()
    }
}

struct Entry {
    id: &'static str,
    name: &'static str,
    vendor: &'static str,
    description: String,
    engine: EngineKind,
    size_mb: u64,
    languages: Vec<String>,
    speed: u8,
}

impl Entry {
    /// A batch model: final pass only, no dictionary words, macOS 14.
    fn build(self) -> ModelInfo {
        let p = facts(self.id);
        // The speeds were measured on a Mac. Other systems run other builds.
        let rtfx = if cfg!(target_os = "macos") { p.rtfx } else { None };
        ModelInfo {
            id: ModelId::new(self.id),
            name: self.name.into(),
            vendor: self.vendor.into(),
            description: self.description,
            engine: self.engine,
            size_bytes: self.size_mb * MB,
            languages: self.languages,
            final_pass: true,
            live_preview: false,
            vocabulary: false,
            speed: rtfx.map_or(self.speed, speed_level),
            wer_percent: p.wer,
            preview_wer_percent: p.preview_wer,
            punctuation: p.punctuation,
            rtfx,
            card_url: p.card_url(),
            credit: p.credit.map(Into::into),
            recommended: false,
            min_macos: 14,
        }
    }
}

/// The speed meter level for a measured speed (audio time over processing time).
fn speed_level(rtfx: u32) -> u8 {
    match rtfx {
        x if x >= 120 => 5,
        x if x >= 60 => 4,
        x if x >= 30 => 3,
        x if x >= 12 => 2,
        _ => 1,
    }
}

/// What Sayso knows about a model, by model id. The ids are the same on all systems.
pub(crate) struct Facts {
    /// From `models/benchmark.rs`. Punctuation does not count.
    pub wer: Option<f32>,
    pub preview_wer: Option<f32>,
    pub rtfx: Option<u32>,
    pub punctuation: bool,
    /// The Hugging Face repository.
    pub card: Option<&'static str>,
    /// The credit that the license asks for. It stays in English.
    pub credit: Option<&'static str>,
}

impl Facts {
    pub fn card_url(&self) -> Option<String> {
        self.card.map(|repo| format!("https://huggingface.co/{repo}"))
    }
}

pub(crate) fn facts(id: &str) -> Facts {
    let card = match id {
        "cohere-transcribe" => Some("CohereLabs/cohere-transcribe-03-2026"),
        "parakeet-unified-en" => Some("nvidia/parakeet-unified-en-0.6b"),
        "parakeet-tdt-v2" => Some("nvidia/parakeet-tdt-0.6b-v2"),
        "parakeet-tdt-v3" => Some("nvidia/parakeet-tdt-0.6b-v3"),
        "nemotron-en" => Some("nvidia/nemotron-speech-streaming-en-0.6b"),
        "canary-1b-v2" => Some("nvidia/canary-1b-v2"),
        "whisper-large-v3" => Some("openai/whisper-large-v3"),
        "parakeet-tdt-ctc-110m" => Some("nvidia/parakeet-tdt_ctc-110m"),
        "whisper-distil-large-v3" => Some("distil-whisper/distil-large-v3"),
        "whisper-large-v3-turbo" => Some("openai/whisper-large-v3-turbo"),
        "whisper-large-v2" => Some("openai/whisper-large-v2"),
        "parakeet-eou-120m" => Some("nvidia/parakeet_realtime_eou_120m-v1"),
        "parakeet-ultra" => Some("moondream/parakeet-ultra"),
        "fluid-speech-mini" => Some("FluidVoice-Labs/fluid-speech-mini-coreml"),
        "parakeet-redux" => Some("moondream/parakeet-redux"),
        "phonon-2" => Some("FermionResearch/Phonon-2"),
        "parakeet-ja" => Some("nvidia/parakeet-tdt_ctc-0.6b-ja"),
        "nemotron-multilingual" => Some("nvidia/nemotron-3.5-asr-streaming-0.6b"),
        "whisper-medium" => Some("openai/whisper-medium"),
        "whisper-small" => Some("openai/whisper-small"),
        "whisper-base" => Some("openai/whisper-base"),
        "whisper-tiny" => Some("openai/whisper-tiny"),
        "sensevoice-small" => Some("FunAudioLLM/SenseVoiceSmall"),
        _ => None,
    };
    let measured = benchmark::MEASURED.iter().find(|(m, _, _)| *m == id);
    Facts {
        wer: measured.map(|(_, wer, _)| *wer),
        preview_wer: benchmark::PREVIEW.iter().find(|(m, _)| *m == id).map(|(_, wer)| *wer),
        rtfx: measured.map(|(_, _, rtfx)| *rtfx),
        punctuation: id != "parakeet-eou-120m",
        card,
        // CC BY 4.0 asks for this text.
        credit: (id == "fluid-speech-mini")
            .then_some("Fluid Speech Mini by FluidVoice. Built on NVIDIA Parakeet TDT 0.6B v3 and Moondream Parakeet Ultra."),
    }
}

fn tdt(version: &str) -> EngineKind {
    EngineKind::ParakeetTdt { version: version.into() }
}

fn whisper(variant: &str) -> EngineKind {
    EngineKind::Whisper { variant: variant.into() }
}

/// The local models of this system, in display order.
pub fn catalog() -> Vec<ModelInfo> {
    if cfg!(target_os = "macos") {
        apple_catalog()
    } else if cfg!(windows) {
        portable::catalog()
    } else {
        crate::models_onnx::catalog()
    }
}

/// The models that the Swift engine (`native/macos/SaysoEngine`) runs on a Mac, in display order.
fn apple_catalog() -> Vec<ModelInfo> {
    let en = languages::english;
    vec![
        // Parakeet.
        ModelInfo {
            live_preview: true,
            vocabulary: true,
            recommended: true,
            ..Entry {
                id: "parakeet-unified-en",
                name: "Parakeet Unified EN",
                vendor: "NVIDIA",
                description: t!("core.model.parakeet_unified_en"),
                engine: EngineKind::ParakeetUnified { streaming_tier: "70_2_2".into() },
                size_mb: 595 + 578 + 98,
                languages: en(),
                speed: 5,
            }
            .build()
        },
        Entry {
            id: "parakeet-ultra",
            name: "Parakeet Ultra",
            vendor: "NVIDIA",
            description: t!("core.model.parakeet_ultra"),
            engine: tdt("ultra"),
            size_mb: 632,
            languages: languages::parakeet_v3(),
            speed: 5,
        }
        .build(),
        ModelInfo {
            // The Core ML build needs macOS 15.
            min_macos: 15,
            ..Entry {
                id: "fluid-speech-mini",
                name: "Fluid Speech Mini",
                vendor: "FluidVoice",
                description: t!("core.model.fluid_speech_mini"),
                engine: tdt("fluid_mini"),
                size_mb: 253,
                languages: en(),
                speed: 5,
            }
            .build()
        },
        Entry {
            id: "parakeet-tdt-v3",
            name: "Parakeet TDT v3",
            vendor: "NVIDIA",
            description: t!("core.model.parakeet_tdt_v3"),
            engine: tdt("v3"),
            size_mb: 483,
            languages: languages::parakeet_v3(),
            speed: 5,
        }
        .build(),
        ModelInfo {
            min_macos: 15,
            ..Entry {
                id: "parakeet-redux",
                name: "Parakeet Redux",
                vendor: "NVIDIA",
                description: t!("core.model.parakeet_redux"),
                engine: tdt("redux"),
                size_mb: 220,
                languages: languages::parakeet_v3(),
                speed: 5,
            }
            .build()
        },
        Entry {
            id: "parakeet-tdt-v2",
            name: "Parakeet TDT v2",
            vendor: "NVIDIA",
            description: t!("core.model.parakeet_tdt_v2"),
            engine: tdt("v2"),
            size_mb: 465,
            languages: en(),
            speed: 5,
        }
        .build(),
        ModelInfo {
            min_macos: 15,
            ..Entry {
                id: "phonon-2",
                name: "Phonon-2",
                vendor: "Fermion Research",
                description: t!("core.model.phonon_2"),
                engine: tdt("phonon2"),
                size_mb: 357,
                languages: en(),
                speed: 5,
            }
            .build()
        },
        Entry {
            id: "parakeet-tdt-ctc-110m",
            name: "Parakeet TDT-CTC 110M",
            vendor: "NVIDIA",
            description: t!("core.model.parakeet_tdt_ctc_110m"),
            engine: tdt("tdt_ctc_110m"),
            size_mb: 227,
            languages: en(),
            speed: 5,
        }
        .build(),
        Entry {
            id: "parakeet-ja",
            name: "Parakeet Japanese",
            vendor: "NVIDIA",
            description: t!("core.model.parakeet_ja"),
            engine: tdt("ja"),
            size_mb: 625,
            languages: vec!["ja".into()],
            speed: 3,
        }
        .build(),
        ModelInfo {
            live_preview: true,
            ..Entry {
                id: "parakeet-eou-120m",
                name: "Parakeet EOU 120M",
                vendor: "NVIDIA",
                description: t!("core.model.parakeet_eou_120m"),
                engine: EngineKind::ParakeetEou,
                size_mb: 222,
                languages: en(),
                speed: 5,
            }
            .build()
        },
        // Whisper.
        Entry {
            id: "whisper-large-v3-turbo",
            name: "Whisper large-v3 turbo",
            vendor: "OpenAI",
            description: t!("core.model.whisper_large_v3_turbo"),
            engine: whisper("openai_whisper-large-v3-v20240930_turbo_632MB"),
            size_mb: 646,
            languages: languages::whisper(),
            speed: 3,
        }
        .build(),
        Entry {
            id: "whisper-large-v3",
            name: "Whisper large-v3",
            vendor: "OpenAI",
            description: t!("core.model.whisper_large_v3"),
            engine: whisper("openai_whisper-large-v3_947MB"),
            size_mb: 948,
            languages: languages::whisper(),
            speed: 2,
        }
        .build(),
        Entry {
            id: "whisper-large-v2",
            name: "Whisper large-v2",
            vendor: "OpenAI",
            description: t!("core.model.whisper_large_v2"),
            engine: whisper("openai_whisper-large-v2_949MB"),
            size_mb: 952,
            languages: languages::whisper(),
            speed: 2,
        }
        .build(),
        Entry {
            id: "whisper-distil-large-v3",
            name: "Distil-Whisper large-v3",
            vendor: "Hugging Face",
            description: t!("core.model.whisper_distil_large_v3"),
            engine: whisper("distil-whisper_distil-large-v3_594MB"),
            size_mb: 595,
            languages: en(),
            speed: 4,
        }
        .build(),
        Entry {
            id: "whisper-medium",
            name: "Whisper medium",
            vendor: "OpenAI",
            description: t!("core.model.whisper_medium"),
            engine: whisper("openai_whisper-medium"),
            size_mb: 1530,
            languages: languages::whisper(),
            speed: 2,
        }
        .build(),
        Entry {
            id: "whisper-small",
            name: "Whisper small",
            vendor: "OpenAI",
            description: t!("core.model.whisper_small"),
            engine: whisper("openai_whisper-small_216MB"),
            size_mb: 217,
            languages: languages::whisper(),
            speed: 4,
        }
        .build(),
        Entry {
            id: "whisper-base",
            name: "Whisper base",
            vendor: "OpenAI",
            description: t!("core.model.whisper_base"),
            engine: whisper("openai_whisper-base"),
            size_mb: 147,
            languages: languages::whisper(),
            speed: 5,
        }
        .build(),
        Entry {
            id: "whisper-tiny",
            name: "Whisper tiny",
            vendor: "OpenAI",
            description: t!("core.model.whisper_tiny"),
            engine: whisper("openai_whisper-tiny"),
            size_mb: 77,
            languages: languages::whisper(),
            speed: 5,
        }
        .build(),
        // Other families.
        ModelInfo {
            min_macos: 26,
            ..Entry {
                id: "apple-speech",
                name: "Apple Speech",
                vendor: "Apple",
                description: t!("core.model.apple_speech"),
                engine: EngineKind::AppleSpeech,
                size_mb: 0,
                languages: languages::apple_speech(),
                speed: 5,
            }
            .build()
        },
        ModelInfo {
            live_preview: true,
            vocabulary: true,
            ..Entry {
                id: "nemotron-multilingual",
                name: "Nemotron Multilingual",
                vendor: "NVIDIA",
                description: t!("core.model.nemotron_multilingual"),
                engine: EngineKind::NemotronMultilingual { chunk_ms: 1120 },
                size_mb: 664,
                languages: languages::nemotron(),
                speed: 4,
            }
            .build()
        },
        ModelInfo {
            live_preview: true,
            ..Entry {
                id: "nemotron-en",
                name: "Nemotron Streaming EN",
                vendor: "NVIDIA",
                description: t!("core.model.nemotron_en"),
                engine: EngineKind::Nemotron { chunk_ms: 1120 },
                size_mb: 627,
                languages: en(),
                speed: 4,
            }
            .build()
        },
        ModelInfo {
            min_macos: 15,
            ..Entry {
                id: "cohere-transcribe",
                name: "Cohere Transcribe",
                vendor: "Cohere",
                description: t!("core.model.cohere_transcribe"),
                engine: EngineKind::Cohere,
                size_mb: 2190,
                languages: languages::cohere(),
                speed: 3,
            }
            .build()
        },
        ModelInfo {
            min_macos: 15,
            ..Entry {
                id: "canary-1b-v2",
                name: "Canary 1B v2",
                vendor: "NVIDIA",
                description: t!("core.model.canary_1b_v2"),
                engine: EngineKind::Canary,
                size_mb: 569,
                languages: en(),
                speed: 2,
            }
            .build()
        },
        Entry {
            id: "sensevoice-small",
            name: "SenseVoice Small",
            vendor: "Alibaba",
            description: t!("core.model.sensevoice_small"),
            engine: EngineKind::SenseVoice,
            size_mb: 945,
            languages: languages::sensevoice(),
            speed: 5,
        }
        .build(),
        Entry {
            id: "paraformer-zh",
            name: "Paraformer Large",
            vendor: "Alibaba",
            description: t!("core.model.paraformer_zh"),
            engine: EngineKind::Paraformer,
            size_mb: 644,
            languages: vec!["zh".into()],
            speed: 5,
        }
        .build(),
    ]
}

/// A local model of this system.
pub fn find(id: &ModelId) -> Option<ModelInfo> {
    catalog().into_iter().find(|m| &m.id == id)
}

/// The local models and the models of every configured speech provider.
pub fn catalog_with(speech: &Speech) -> Vec<ModelInfo> {
    let mut list = catalog();
    list.extend(speech.catalog());
    list
}

/// A local model, or a model of a configured speech provider.
pub fn find_with(speech: &Speech, id: &ModelId) -> Option<ModelInfo> {
    find(id).or_else(|| speech.catalog().into_iter().find(|m| &m.id == id))
}

/// The model that streams the live preview for `active`, the final model.
///
/// A chosen model does it when it is on disk and understands the dictation
/// language. With `Auto`, the final model does it when it can stream. If not,
/// a local streaming model that is on disk and understands the language does
/// it: the small fallback model first, then the others in catalog order.
/// `on_disk` tells which local models are downloaded.
pub fn preview_for(active: &ModelInfo, choice: &PreviewChoice, language_setting: &str, on_disk: impl Fn(&ModelId) -> bool) -> Option<ModelId> {
    let language = active.language_for(language_setting);
    if let PreviewChoice::Model(id) = choice {
        return find(id).filter(|m| m.live_preview && on_disk(&m.id) && m.understands(&language)).map(|m| m.id);
    }
    if active.live_preview {
        return Some(active.id.clone());
    }
    let fallback = preview_fallback();
    let mut candidates: Vec<ModelInfo> =
        catalog().into_iter().filter(|m| m.live_preview && on_disk(&m.id) && m.understands(&language)).collect();
    candidates.sort_by_key(|m| m.id != fallback);
    candidates.first().map(|m| m.id.clone())
}

/// True when the chosen preview model does not understand the dictation
/// language, so the dictation has no live preview.
pub fn preview_language_mismatch(active: &ModelInfo, choice: &PreviewChoice, language_setting: &str) -> bool {
    let PreviewChoice::Model(id) = choice else { return false };
    find(id).is_some_and(|m| !m.understands(&active.language_for(language_setting)))
}

/// Human-readable size: "626 MB", "1.3 GB".
pub fn format_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{} GB", sayso_i18n::numbers::decimal(bytes as f64 / 1e9, 1))
    } else if bytes >= 1_000_000 {
        format!("{} MB", (bytes as f64 / 1e6).round() as u64)
    } else {
        format!("{} KB", (bytes as f64 / 1e3).round() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn default_model_is_in_catalog_and_recommended() {
        let m = find(&default_model()).unwrap();
        assert!(m.recommended && m.final_pass && m.live_preview);
    }

    #[test]
    fn the_suggested_model_understands_each_language() {
        let any = |_: &ModelInfo| true;
        for code in languages::all() {
            let id = suggested_for(&code, any);
            let m = find(&id).unwrap_or_else(|| panic!("{id:?} for {code} is not in the catalog"));
            let known = catalog().iter().any(|m| m.final_pass && m.understands(&code));
            assert!(!known || (m.final_pass && m.understands(&code)), "{id:?} for {code}");
        }
        assert_eq!(suggested_for("en", any), default_model());
        assert_eq!(suggested_for(languages::AUTO, any), default_model());
        assert_eq!(suggested_for("zh", any), ModelId::new(by_os("sensevoice-small", "whisper-large-v3-turbo", "sensevoice-small")));
    }

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<_> = catalog().into_iter().map(|m| m.id).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }

    #[test]
    fn every_local_model_is_usable_for_something() {
        for m in catalog() {
            assert!(m.final_pass || m.live_preview, "{} does nothing", m.id);
            assert!(!m.is_remote());
            assert!(m.size_bytes > 0 || m.engine == EngineKind::AppleSpeech, "{} has no download size", m.id);
            assert!(!m.languages.is_empty(), "{} lists no language", m.id);
            assert!(!m.id.as_str().contains(':'), "{}: a colon marks a cloud model", m.id);
            assert!((1..=5).contains(&m.speed));
        }
        let fallback = find(&preview_fallback()).unwrap();
        assert!(fallback.live_preview);
        #[cfg(target_os = "macos")]
        // Rough, but it can write the final text.
        assert!(fallback.final_pass && !fallback.punctuation);
        let default = find(&default_model()).unwrap();
        assert!(default.recommended && default.final_pass);
    }

    #[test]
    #[cfg(target_os = "macos")] // The ids are from the macOS catalog.
    fn language_falls_back_to_what_the_model_supports() {
        let unified = find(&default_model()).unwrap();
        assert_eq!(unified.language_for("de"), "en", "an English model stays English");
        assert_eq!(unified.language_label(), "English");
        let ultra = find(&ModelId::new("parakeet-ultra")).unwrap();
        assert_eq!(ultra.language_for("de"), "de");
        assert_eq!(ultra.language_for("ja"), "auto", "an unsupported language becomes detection");
        assert_eq!(ultra.language_for("auto"), "auto");
        assert_eq!(ultra.language_label(), "25 languages");
        assert_eq!(find(&ModelId::new("parakeet-ja")).unwrap().language_label(), "Japanese");
    }

    #[test]
    fn cloud_models_join_the_catalog() {
        use crate::speech::{SpeechProvider, SpeechProviderKind};
        let mut speech = Speech::default();
        assert_eq!(catalog_with(&speech).len(), catalog().len());
        speech.providers.push(SpeechProvider {
            id: "openai".into(),
            name: "OpenAI".into(),
            kind: SpeechProviderKind::OpenAi,
            base_url: None,
            api_key_account: Some("speech.openai".into()),
            models: vec![],
        });
        let id = ModelId::new("openai:gpt-4o-transcribe");
        assert_eq!(find(&id), None);
        let m = find_with(&speech, &id).unwrap();
        assert!(m.is_remote());
        assert_eq!(m.language_label(), "Many languages");
        assert_eq!(m.language_for("de"), "de");
        assert!(find_with(&speech, &default_model()).is_some());
    }

    #[test]
    #[cfg(target_os = "macos")] // The ids are from the macOS catalog.
    fn preview_comes_from_the_active_model_or_a_streaming_model_on_disk() {
        let model = |id: &str| find(&ModelId::new(id)).unwrap();
        fn on(ids: &[&'static str]) -> impl Fn(&ModelId) -> bool {
            let ids = ids.to_vec();
            move |id| ids.contains(&id.as_str())
        }
        let all = ["parakeet-unified-en", "parakeet-eou-120m", "nemotron-multilingual", "nemotron-en"];

        // A streaming model previews itself, whatever else is on disk.
        assert_eq!(preview_for(&model("parakeet-unified-en"), &PreviewChoice::Auto, "en", on(&[])), Some(default_model()));

        // Whisper in English: the small fallback wins over the bigger streaming models.
        let whisper = model("whisper-large-v3-turbo");
        assert_eq!(preview_for(&whisper, &PreviewChoice::Auto, "en", on(&all)), Some(preview_fallback()));
        assert_eq!(preview_for(&whisper, &PreviewChoice::Auto, "en", on(&["parakeet-unified-en"])), Some(default_model()));
        assert_eq!(preview_for(&whisper, &PreviewChoice::Auto, "en", on(&[])), None);

        // German or detection: only a multilingual streaming model can preview.
        assert_eq!(preview_for(&whisper, &PreviewChoice::Auto, "de", on(&all)), Some(ModelId::new("nemotron-multilingual")));
        assert_eq!(preview_for(&whisper, &PreviewChoice::Auto, "auto", on(&all)), Some(ModelId::new("nemotron-multilingual")));
        assert_eq!(preview_for(&whisper, &PreviewChoice::Auto, "de", on(&["parakeet-eou-120m"])), None, "an English preview of German speech is wrong");

        // An English-only model ignores the setting, so its preview is English too.
        assert_eq!(preview_for(&model("parakeet-tdt-v2"), &PreviewChoice::Auto, "de", on(&["parakeet-eou-120m"])), Some(preview_fallback()));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn a_chosen_preview_model_wins_when_it_can_run() {
        let model = |id: &str| find(&ModelId::new(id)).unwrap();
        let eou = PreviewChoice::Model(ModelId::new("parakeet-eou-120m"));
        let unified = model("parakeet-unified-en");
        let disk = |id: &ModelId| id.as_str() == "parakeet-eou-120m";

        // Also when the final model streams by itself.
        assert_eq!(preview_for(&unified, &eou, "en", disk), Some(ModelId::new("parakeet-eou-120m")));
        assert_eq!(preview_for(&unified, &eou, "en", |_| false), None, "not on disk");

        // An English model under a German final pass: no preview, and the page says why.
        let whisper = model("whisper-large-v3");
        assert_eq!(preview_for(&whisper, &eou, "de", disk), None);
        assert!(preview_language_mismatch(&whisper, &eou, "de"));
        assert!(!preview_language_mismatch(&whisper, &eou, "en"));
        assert!(!preview_language_mismatch(&whisper, &PreviewChoice::Auto, "de"));
    }

    #[test]
    fn every_measured_model_has_a_page() {
        for m in catalog() {
            // Apple Speech is part of macOS and has no model page.
            if m.wer_percent.is_some() && m.engine != EngineKind::AppleSpeech {
                assert!(m.card_url.is_some(), "{}", m.id);
            }
            assert!(m.preview_wer_percent.is_none() || m.live_preview, "{}", m.id);
        }
    }

    #[test]
    fn sizes_format() {
        assert_eq!(format_size(626 * MB), "626 MB");
        assert_eq!(format_size(1_271 * MB), "1.3 GB");
        assert_eq!(format_size(412_000), "412 KB");
    }
}
