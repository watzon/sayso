//! The models of the portable engine (`native/portable/sayso-engine`), the
//! speech engine of every platform but macOS. It runs Parakeet through ONNX
//! Runtime and Whisper through whisper.cpp, both with transcribe-rs.
//!
//! Ids match the macOS catalog where the model is the same, so a config file
//! and History mean the same model on every platform.

use super::{Entry, EngineKind, ModelInfo};
use crate::languages;

/// Parakeet TDT v3: fast on a CPU, accurate, and 25 languages.
pub(super) const DEFAULT_MODEL: &str = "parakeet-tdt-v3";
/// The engine gives Parakeet a live preview by decoding the growing
/// recording again, so the default model is also the preview fallback.
pub(super) const PREVIEW_FALLBACK: &str = DEFAULT_MODEL;

/// Files of the int8 Parakeet exports by istupakov, in MB (encoder, decoder
/// and joint, preprocessor, vocabulary).
const PARAKEET_V3_MB: u64 = 652 + 18 + 1;
const PARAKEET_V2_MB: u64 = 652 + 9 + 1;

fn parakeet(repo: &str) -> EngineKind {
    EngineKind::Onnx { family: "parakeet".into(), repo: repo.into() }
}

fn ggml(file: &str) -> EngineKind {
    EngineKind::WhisperCpp { file: file.into() }
}

pub(super) fn catalog() -> Vec<ModelInfo> {
    let en = languages::english;
    vec![
        // Parakeet.
        ModelInfo {
            live_preview: true,
            recommended: true,
            ..Entry {
                id: "parakeet-tdt-v3",
                name: "Parakeet TDT v3",
                vendor: "NVIDIA",
                description: "Fast and accurate on the processor of this computer. English and 24 European languages. Shows your words live while you speak.",
                engine: parakeet("istupakov/parakeet-tdt-0.6b-v3-onnx"),
                size_mb: PARAKEET_V3_MB,
                languages: languages::parakeet_v3(),
                speed: 5,
                wer: Some(2.3),
            }
            .build()
        },
        ModelInfo {
            live_preview: true,
            ..Entry {
                id: "parakeet-tdt-v2",
                name: "Parakeet TDT v2",
                vendor: "NVIDIA",
                description: "The English Parakeet. Shows your words live while you speak.",
                engine: parakeet("istupakov/parakeet-tdt-0.6b-v2-onnx"),
                size_mb: PARAKEET_V2_MB,
                languages: en(),
                speed: 5,
                wer: Some(1.7),
            }
            .build()
        },
        // Whisper.
        ModelInfo {
            vocabulary: true,
            ..Entry {
                id: "whisper-large-v3-turbo",
                name: "Whisper large-v3 turbo",
                vendor: "OpenAI",
                description: "Nearly as accurate as large-v3 and much faster. Quantized to 5 bits.",
                engine: ggml("ggml-large-v3-turbo-q5_0.bin"),
                size_mb: 574,
                languages: languages::whisper(),
                speed: 3,
                wer: None,
            }
            .build()
        },
        ModelInfo {
            vocabulary: true,
            ..Entry {
                id: "whisper-large-v3",
                name: "Whisper large-v3",
                vendor: "OpenAI",
                description: "Good with strong accents and mixed language. Slow without a fast processor. Quantized to 5 bits.",
                engine: ggml("ggml-large-v3-q5_0.bin"),
                size_mb: 1081,
                languages: languages::whisper(),
                speed: 1,
                wer: None,
            }
            .build()
        },
        ModelInfo {
            vocabulary: true,
            ..Entry {
                id: "whisper-medium",
                name: "Whisper medium",
                vendor: "OpenAI",
                description: "Between small and large. Quantized to 5 bits.",
                engine: ggml("ggml-medium-q5_0.bin"),
                size_mb: 539,
                languages: languages::whisper(),
                speed: 2,
                wer: None,
            }
            .build()
        },
        ModelInfo {
            vocabulary: true,
            ..Entry {
                id: "whisper-small",
                name: "Whisper small",
                vendor: "OpenAI",
                description: "Lower accuracy. Good for a quick try.",
                engine: ggml("ggml-small.bin"),
                size_mb: 488,
                languages: languages::whisper(),
                speed: 4,
                wer: None,
            }
            .build()
        },
        ModelInfo {
            vocabulary: true,
            ..Entry {
                id: "whisper-base",
                name: "Whisper base",
                vendor: "OpenAI",
                description: "Small and fast, with many mistakes.",
                engine: ggml("ggml-base.bin"),
                size_mb: 148,
                languages: languages::whisper(),
                speed: 5,
                wer: None,
            }
            .build()
        },
        ModelInfo {
            vocabulary: true,
            ..Entry {
                id: "whisper-tiny",
                name: "Whisper tiny",
                vendor: "OpenAI",
                description: "The smallest model. For tests and very old computers.",
                engine: ggml("ggml-tiny.bin"),
                size_mb: 78,
                languages: languages::whisper(),
                speed: 5,
                wer: None,
            }
            .build()
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ModelId, preview_for};

    fn model(id: &str) -> ModelInfo {
        catalog().into_iter().find(|m| m.id.as_str() == id).unwrap()
    }

    #[test]
    fn the_default_previews_itself_and_is_recommended() {
        let m = model(DEFAULT_MODEL);
        assert!(m.recommended && m.final_pass && m.live_preview);
        assert_eq!(catalog().iter().filter(|m| m.recommended).count(), 1);
    }

    #[test]
    fn every_model_runs_in_the_portable_engine() {
        for m in catalog() {
            assert!(
                matches!(m.engine, EngineKind::Onnx { .. } | EngineKind::WhisperCpp { .. }),
                "{} needs the macOS engine",
                m.id
            );
            assert!(m.final_pass);
        }
    }

    #[test]
    #[cfg(not(target_os = "macos"))] // `preview_for` reads the catalog of this platform.
    fn whisper_borrows_the_parakeet_preview_in_parakeet_languages() {
        let whisper = model("whisper-small");
        let on = |ids: &'static [&'static str]| move |id: &ModelId| ids.contains(&id.as_str());
        assert_eq!(preview_for(&whisper, "en", on(&["parakeet-tdt-v3"])), Some(ModelId::new("parakeet-tdt-v3")));
        assert_eq!(preview_for(&whisper, "en", on(&["parakeet-tdt-v2"])), Some(ModelId::new("parakeet-tdt-v2")));
        assert_eq!(preview_for(&whisper, "de", on(&["parakeet-tdt-v2"])), None, "v2 is English only");
        assert_eq!(preview_for(&whisper, "ja", on(&["parakeet-tdt-v3"])), None);
        assert_eq!(preview_for(&whisper, "en", on(&[])), None);
    }
}
