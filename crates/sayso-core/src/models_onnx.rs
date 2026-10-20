//! The local model catalog for Linux and Windows.
//!
//! On these systems the engine is `sayso-engine`, a Rust sidecar on
//! sherpa-onnx that runs on the CPU. Each model is one archive from the
//! sherpa-onnx `asr-models` release, except a `ParakeetCpp` model: one GGUF
//! file on Hugging Face that parakeet.cpp runs on the GPU through Vulkan. A
//! model with a live preview also gets a small streaming model, downloaded
//! with it.
//!
//! The ids match the macOS catalog where the model is the same, so a config
//! file and the history mean the same model on every system.

use crate::languages;
use crate::models::{EngineKind, ModelId, ModelInfo};
use sayso_i18n::t;

const MB: u64 = 1_000_000;

/// The streaming English model that gives the live preview.
pub const PREVIEW_ARCHIVE: &str = "sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06";

pub fn default_model() -> ModelId {
    ModelId::new("parakeet-tdt-v2")
}

pub fn preview_fallback() -> ModelId {
    ModelId::new("kroko-en")
}

fn sherpa(recipe: &str, archive: &str, preview: bool) -> EngineKind {
    EngineKind::Sherpa {
        recipe: recipe.into(),
        archive: archive.into(),
        preview_archive: preview.then(|| PREVIEW_ARCHIVE.to_string()),
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
    /// A batch model: final pass only, no dictionary words.
    fn build(self) -> ModelInfo {
        let p = crate::models::facts(self.id);
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
            speed: self.speed,
            wer_percent: p.wer,
            preview_wer_percent: p.preview_wer,
            punctuation: p.punctuation,
            // The speeds were measured on a Mac.
            rtfx: None,
            card_url: p.card_url(),
            credit: p.credit.map(Into::into),
            recommended: false,
            min_macos: 0,
        }
    }
}

/// The models that the Rust engine runs, in display order.
pub fn catalog() -> Vec<ModelInfo> {
    let en = languages::english;
    vec![
        // Parakeet.
        ModelInfo {
            live_preview: true,
            recommended: true,
            ..Entry {
                id: "parakeet-tdt-v2",
                name: "Parakeet TDT v2",
                vendor: "NVIDIA",
                description: t!("core.model.parakeet_tdt_v2_onnx"),
                engine: sherpa("parakeet_tdt", "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8", true),
                size_mb: 482 + 57,
                languages: en(),
                speed: 4,
            }
            .build()
        },
        // On the GPU: FluidVoice's quantized Parakeet Ultra, in the GGUF it publishes for parakeet.cpp.
        {
            const REPO: &str = "FluidVoice-Labs/fluid-speech-mini-gguf";
            let info = Entry {
                id: "fluid-speech-mini",
                name: "Fluid Speech Mini",
                vendor: "FluidVoice",
                description: t!("core.model.fluid_speech_mini_onnx"),
                engine: EngineKind::ParakeetCpp {
                    repo: REPO.into(),
                    revision: "490bcd5de891f5efd0e732434dfb0a4b0899aead".into(),
                    file: "fluid-speech-mini-q2k-q4k.gguf".into(),
                    preview_archive: Some(PREVIEW_ARCHIVE.into()),
                },
                size_mb: 347 + 57,
                languages: en(),
                speed: 5, // 213x real time on an RTX 4070 (docs/benchmark.md, "Linux")
            }
            .build();
            ModelInfo {
                live_preview: true,
                // This GGUF on parakeet.cpp, measured on Linux; the shared figure is the Core ML build's.
                wer_percent: Some(6.56),
                // The shared facts name the Core ML build that macOS runs.
                card_url: Some(format!("https://huggingface.co/{REPO}")),
                // The MIT License of the runtime asks for its notice too.
                credit: info.credit.map(|c| format!("{c} Runs on parakeet.cpp and ggml (MIT License).")),
                ..info
            }
        },
        Entry {
            id: "parakeet-tdt-v3",
            name: "Parakeet TDT v3",
            vendor: "NVIDIA",
            description: t!("core.model.parakeet_tdt_v3_onnx"),
            engine: sherpa("parakeet_tdt", "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8", false),
            size_mb: 487,
            languages: languages::parakeet_v3(),
            speed: 4,
        }
        .build(),
        Entry {
            id: "parakeet-tdt-ctc-110m",
            name: "Parakeet TDT-CTC 110M",
            vendor: "NVIDIA",
            description: t!("core.model.parakeet_tdt_ctc_110m_onnx"),
            engine: sherpa("parakeet_tdt_ctc", "sherpa-onnx-nemo-parakeet_tdt_ctc_110m-en-36000-int8", false),
            size_mb: 104,
            languages: en(),
            speed: 5,
        }
        .build(),
        ModelInfo {
            final_pass: false,
            live_preview: true,
            ..Entry {
                id: "kroko-en",
                name: "Kroko Streaming EN",
                vendor: "Banafo",
                description: t!("core.model.kroko_en"),
                engine: sherpa("zipformer_streaming", PREVIEW_ARCHIVE, false),
                size_mb: 57,
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
            description: t!("core.model.whisper_large_v3_turbo_onnx"),
            engine: sherpa("whisper", "sherpa-onnx-whisper-turbo", false),
            size_mb: 563,
            languages: languages::whisper(),
            speed: 2,
        }
        .build(),
        Entry {
            id: "whisper-distil-large-v3",
            name: "Distil-Whisper large-v3",
            vendor: "Hugging Face",
            description: t!("core.model.whisper_distil_large_v3_onnx"),
            engine: sherpa("whisper", "sherpa-onnx-whisper-distil-large-v3", false),
            size_mb: 529,
            languages: en(),
            speed: 3,
        }
        .build(),
        Entry {
            id: "whisper-small",
            name: "Whisper small",
            vendor: "OpenAI",
            description: t!("core.model.whisper_small"),
            engine: sherpa("whisper", "sherpa-onnx-whisper-small", false),
            size_mb: 639,
            languages: languages::whisper(),
            speed: 3,
        }
        .build(),
        Entry {
            id: "whisper-base",
            name: "Whisper base",
            vendor: "OpenAI",
            description: t!("core.model.whisper_base"),
            engine: sherpa("whisper", "sherpa-onnx-whisper-base", false),
            size_mb: 207,
            languages: languages::whisper(),
            speed: 4,
        }
        .build(),
        Entry {
            id: "whisper-tiny",
            name: "Whisper tiny",
            vendor: "OpenAI",
            description: t!("core.model.whisper_tiny_portable"),
            engine: sherpa("whisper", "sherpa-onnx-whisper-tiny", false),
            size_mb: 116,
            languages: languages::whisper(),
            speed: 5,
        }
        .build(),
        // Other families.
        Entry {
            id: "sensevoice-small",
            name: "SenseVoice Small",
            vendor: "Alibaba",
            description: t!("core.model.sensevoice_small"),
            engine: sherpa("sense_voice", "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09", false),
            size_mb: 165,
            languages: languages::sensevoice(),
            speed: 5,
        }
        .build(),
        Entry {
            id: "moonshine-base-en",
            name: "Moonshine Base EN",
            vendor: "Useful Sensors",
            description: t!("core.model.moonshine_base_en"),
            engine: sherpa("moonshine", "sherpa-onnx-moonshine-base-en-quantized-2026-02-27", false),
            size_mb: 111,
            languages: en(),
            speed: 5,
        }
        .build(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(id: &str) -> ModelInfo {
        catalog().into_iter().find(|m| m.id.as_str() == id).unwrap_or_else(|| panic!("{id} is not in the catalog"))
    }

    #[test]
    fn default_and_fallback_are_in_the_catalog() {
        let default = find(default_model().as_str());
        assert!(default.recommended && default.final_pass && default.live_preview);
        let fallback = find(preview_fallback().as_str());
        assert!(fallback.live_preview && !fallback.final_pass);
    }

    #[test]
    fn every_model_names_a_sherpa_archive_or_a_gguf() {
        for m in catalog() {
            // A model with a live preview streams itself or brings the preview model.
            let streams_itself = match &m.engine {
                EngineKind::Sherpa { recipe, archive, preview_archive } => {
                    assert!(!recipe.is_empty() && archive.starts_with("sherpa-onnx-"), "{}", m.id);
                    recipe == "zipformer_streaming" || preview_archive.is_some()
                }
                EngineKind::ParakeetCpp { repo, revision, file, preview_archive } => {
                    assert!(repo.contains('/') && !revision.is_empty() && file.ends_with(".gguf"), "{}", m.id);
                    preview_archive.is_some()
                }
                _ => panic!("{} is not a model the Rust engine runs", m.id),
            };
            assert_eq!(m.live_preview, streams_itself, "{}: live_preview must match the archives", m.id);
            assert!(m.size_bytes > 0 && !m.languages.is_empty() && (1..=5).contains(&m.speed), "{}", m.id);
            assert!(!m.id.as_str().contains(':'), "{}: a colon marks a cloud model", m.id);
        }
    }

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<_> = catalog().into_iter().map(|m| m.id).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }
}
