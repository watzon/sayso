//! The local model catalog for Linux and Windows.
//!
//! On these systems the engine is `sayso-engine`, a Rust sidecar on
//! sherpa-onnx that runs on the CPU. Most models are one archive from the
//! sherpa-onnx `asr-models` release. Two kinds are not:
//!
//! - On Linux, a `ParakeetCpp` model: one GGUF file on Hugging Face that
//!   parakeet.cpp runs on the GPU through Vulkan.
//! - On Windows, Whisper: one GGML file on Hugging Face that whisper.cpp runs
//!   (`WhisperCpp`). It is faster than the sherpa-onnx Whisper on the CPU.
//!
//! A model with a live preview also gets a small streaming model, downloaded
//! with it, or decodes its own growing recording again (`redecode_preview`).
//!
//! The VAD model and the diarization model come last. They are single ONNX
//! files and a segmentation archive of other sherpa-onnx releases.
//!
//! The ids match the macOS catalog where the model is the same, so a config
//! file and the history mean the same model on every system.

use crate::languages;
use crate::models::{EngineKind, ModelId, ModelInfo};
use sayso_i18n::t;

const MB: u64 = 1_000_000;

/// The streaming English model that gives the live preview.
pub const PREVIEW_ARCHIVE: &str = "sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06";

/// The speaker embedding model of the diarization model: an ONNX file of the
/// sherpa-onnx `speaker-recongition-models` release, and its size in bytes.
/// A benchmark on meeting recordings chooses it (`docs/extensions.md`,
/// "Diarization models"). Until it runs, this is the 3D-Speaker CAM++ model
/// for Chinese and English.
pub const SPEAKER_EMBEDDING: (&str, u64) = ("3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx", 28_281_164);

/// The pyannote segmentation archive of the diarization model, without
/// `.tar.bz2`, and its size in bytes.
pub const SPEAKER_SEGMENTATION: (&str, u64) = ("sherpa-onnx-pyannote-segmentation-3-0", 6_958_444);

/// A system with its own list of models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    Linux,
    Windows,
}

impl System {
    /// The system that this build runs on. Every system but Windows uses the Linux list.
    pub fn current() -> System {
        if cfg!(windows) { System::Windows } else { System::Linux }
    }
}

pub fn default_model() -> ModelId {
    default_model_for(System::current())
}

/// Linux: Parakeet TDT v2 with the small streaming preview. Windows: Parakeet
/// TDT v3, which has 25 languages and gives its own live preview, as the Windows
/// engine before 0.12 did.
pub fn default_model_for(system: System) -> ModelId {
    match system {
        System::Linux => ModelId::new("parakeet-tdt-v2"),
        System::Windows => ModelId::new("parakeet-tdt-v3"),
    }
}

pub fn preview_fallback() -> ModelId {
    preview_fallback_for(System::current())
}

pub fn preview_fallback_for(system: System) -> ModelId {
    match system {
        System::Linux => ModelId::new("kroko-en"),
        System::Windows => ModelId::new("parakeet-tdt-v3"),
    }
}

fn sherpa(recipe: &str, archive: &str, preview: bool) -> EngineKind {
    EngineKind::Sherpa {
        recipe: recipe.into(),
        archive: archive.into(),
        preview_archive: preview.then(|| PREVIEW_ARCHIVE.to_string()),
        redecode_preview: false,
    }
}

/// A sherpa-onnx model that gives its own live preview by decoding the growing recording again.
fn sherpa_redecode(recipe: &str, archive: &str) -> EngineKind {
    EngineKind::Sherpa {
        recipe: recipe.into(),
        archive: archive.into(),
        preview_archive: None,
        redecode_preview: true,
    }
}

fn ggml(file: &str) -> EngineKind {
    EngineKind::WhisperCpp { file: file.into() }
}

struct Entry {
    id: &'static str,
    name: &'static str,
    vendor: &'static str,
    description: String,
    engine: EngineKind,
    size_mb: u64,
    languages: &'static [&'static str],
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

/// The models that the Rust engine runs on this system, in display order.
pub fn catalog() -> Vec<ModelInfo> {
    catalog_for(System::current())
}

/// The models that the Rust engine runs on `system`, in display order.
pub fn catalog_for(system: System) -> Vec<ModelInfo> {
    let mut models = Vec::new();
    // Parakeet.
    match system {
        System::Linux => {
            models.push(ModelInfo { recommended: true, ..parakeet_tdt_v2() });
            models.push(fluid_speech_mini());
            models.push(parakeet_tdt_v3());
        }
        System::Windows => {
            models.push(ModelInfo { recommended: true, ..parakeet_tdt_v3() });
            models.push(parakeet_tdt_v2());
        }
    }
    models.push(
        Entry {
            id: "parakeet-tdt-ctc-110m",
            name: "Parakeet TDT-CTC 110M",
            vendor: "NVIDIA",
            description: t!("core.model.parakeet_tdt_ctc_110m_onnx"),
            engine: sherpa("parakeet_tdt_ctc", "sherpa-onnx-nemo-parakeet_tdt_ctc_110m-en-36000-int8", false),
            size_mb: 104,
            languages: languages::english(),
            speed: 5,
        }
        .build(),
    );
    models.push(ModelInfo {
        final_pass: false,
        live_preview: true,
        ..Entry {
            id: "kroko-en",
            name: "Kroko Streaming EN",
            vendor: "Banafo",
            description: t!("core.model.kroko_en"),
            engine: sherpa("zipformer_streaming", PREVIEW_ARCHIVE, false),
            size_mb: 57,
            languages: languages::english(),
            speed: 5,
        }
        .build()
    });
    // Whisper.
    match system {
        System::Linux => models.extend(sherpa_whisper()),
        System::Windows => models.extend(whisper_cpp()),
    }
    // Other families.
    models.push(
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
    );
    models.push(
        Entry {
            id: "moonshine-base-en",
            name: "Moonshine Base EN",
            vendor: "Useful Sensors",
            description: t!("core.model.moonshine_base_en"),
            engine: sherpa("moonshine", "sherpa-onnx-moonshine-base-en-quantized-2026-02-27", false),
            size_mb: 111,
            languages: languages::english(),
            speed: 5,
        }
        .build(),
    );
    models.extend(role_models());
    models
}

/// The VAD and diarization models. TEN VAD is not here, although the engine
/// runs it: its license adds conditions to Apache 2.0 that Sayso cannot accept
/// for its users (`docs/extensions.md`, "VAD models").
fn role_models() -> Vec<ModelInfo> {
    let (segmentation, segmentation_size) = SPEAKER_SEGMENTATION;
    let (embedding, embedding_size) = SPEAKER_EMBEDDING;
    vec![
        ModelInfo {
            card_url: Some("https://github.com/snakers4/silero-vad".into()),
            ..crate::models::role_model(
                "silero-vad",
                "Silero VAD",
                "Silero",
                t!("core.model.silero_vad"),
                EngineKind::SherpaVad { recipe: "silero".into(), file: "silero_vad.onnx".into() },
                643_854,
            )
        },
        ModelInfo {
            card_url: Some("https://huggingface.co/pyannote/segmentation-3.0".into()),
            ..crate::models::role_model(
                "speaker-diarization",
                "Speaker Diarization",
                "pyannote",
                t!("core.model.speaker_diarization_onnx"),
                EngineKind::SherpaDiarization { segmentation: segmentation.into(), embedding: embedding.into() },
                segmentation_size + embedding_size,
            )
        },
    ]
}

fn parakeet_tdt_v2() -> ModelInfo {
    ModelInfo {
        live_preview: true,
        ..Entry {
            id: "parakeet-tdt-v2",
            name: "Parakeet TDT v2",
            vendor: "NVIDIA",
            description: t!("core.model.parakeet_tdt_v2_onnx"),
            engine: sherpa("parakeet_tdt", "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8", true),
            size_mb: 482 + 57,
            languages: languages::english(),
            speed: 4,
        }
        .build()
    }
}

/// Parakeet TDT v3 has no streaming model for its 25 languages, so it decodes the growing recording again.
fn parakeet_tdt_v3() -> ModelInfo {
    ModelInfo {
        live_preview: true,
        ..Entry {
            id: "parakeet-tdt-v3",
            name: "Parakeet TDT v3",
            vendor: "NVIDIA",
            description: t!("core.model.parakeet_tdt_v3_portable"),
            engine: sherpa_redecode("parakeet_tdt", "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8"),
            size_mb: 487,
            languages: languages::parakeet_v3(),
            speed: 4,
        }
        .build()
    }
}

/// On the GPU: FluidVoice's quantized Parakeet Ultra, in the GGUF it publishes for parakeet.cpp. Linux only.
fn fluid_speech_mini() -> ModelInfo {
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
        languages: languages::english(),
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
}

/// Whisper on sherpa-onnx (Linux).
fn sherpa_whisper() -> Vec<ModelInfo> {
    vec![
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
            languages: languages::english(),
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
    ]
}

/// Whisper on whisper.cpp (Windows). Whisper takes the dictionary words as its initial prompt.
fn whisper_cpp() -> Vec<ModelInfo> {
    let model = |id, name, description, file, size_mb, speed| ModelInfo {
        vocabulary: true,
        ..Entry {
            id,
            name,
            vendor: "OpenAI",
            description,
            engine: ggml(file),
            size_mb,
            languages: languages::whisper(),
            speed,
        }
        .build()
    };
    vec![
        model(
            "whisper-large-v3-turbo",
            "Whisper large-v3 turbo",
            t!("core.model.whisper_large_v3_turbo_portable"),
            "ggml-large-v3-turbo-q5_0.bin",
            574,
            3,
        ),
        model(
            "whisper-large-v3",
            "Whisper large-v3",
            t!("core.model.whisper_large_v3_portable"),
            "ggml-large-v3-q5_0.bin",
            1081,
            1,
        ),
        model("whisper-medium", "Whisper medium", t!("core.model.whisper_medium_portable"), "ggml-medium-q5_0.bin", 539, 2),
        model("whisper-small", "Whisper small", t!("core.model.whisper_small"), "ggml-small.bin", 488, 4),
        model("whisper-base", "Whisper base", t!("core.model.whisper_base"), "ggml-base.bin", 148, 5),
        model("whisper-tiny", "Whisper tiny", t!("core.model.whisper_tiny_portable"), "ggml-tiny.bin", 78, 5),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYSTEMS: [System; 2] = [System::Linux, System::Windows];

    fn find(system: System, id: &str) -> ModelInfo {
        catalog_for(system)
            .into_iter()
            .find(|m| m.id.as_str() == id)
            .unwrap_or_else(|| panic!("{id} is not in the {system:?} catalog"))
    }

    #[test]
    fn default_and_fallback_are_in_the_catalog() {
        for system in SYSTEMS {
            let default = find(system, default_model_for(system).as_str());
            assert!(default.recommended && default.final_pass && default.live_preview, "{system:?}");
            assert_eq!(catalog_for(system).iter().filter(|m| m.recommended).count(), 1, "{system:?}");
            let fallback = find(system, preview_fallback_for(system).as_str());
            assert!(fallback.live_preview, "{system:?}");
        }
        assert!(!find(System::Linux, "kroko-en").final_pass);
    }

    #[test]
    fn every_model_names_files_that_the_rust_engine_runs_on_its_system() {
        for system in SYSTEMS {
            for m in catalog_for(system) {
                // A model with a live preview streams itself, brings the preview model, or decodes again.
                let previews = match &m.engine {
                    EngineKind::Sherpa { recipe, archive, preview_archive, redecode_preview } => {
                        assert!(!recipe.is_empty() && archive.starts_with("sherpa-onnx-"), "{}", m.id);
                        assert!(!(*redecode_preview && preview_archive.is_some()), "{}: two previews", m.id);
                        recipe == "zipformer_streaming" || preview_archive.is_some() || *redecode_preview
                    }
                    EngineKind::ParakeetCpp { repo, revision, file, preview_archive } => {
                        assert_eq!(system, System::Linux, "{}: parakeet.cpp runs on Linux only", m.id);
                        assert!(repo.contains('/') && !revision.is_empty() && file.ends_with(".gguf"), "{}", m.id);
                        preview_archive.is_some()
                    }
                    EngineKind::WhisperCpp { file } => {
                        assert_eq!(system, System::Windows, "{}: whisper.cpp runs on Windows only", m.id);
                        assert!(file.starts_with("ggml-") && file.ends_with(".bin"), "{}", m.id);
                        false
                    }
                    EngineKind::SherpaVad { recipe, file } => {
                        assert!(recipe == "silero" && file.ends_with(".onnx"), "{}", m.id);
                        false
                    }
                    EngineKind::SherpaDiarization { segmentation, embedding } => {
                        assert!(segmentation.starts_with("sherpa-onnx-") && embedding.ends_with(".onnx"), "{}", m.id);
                        false
                    }
                    _ => panic!("{} is not a model the Rust engine runs", m.id),
                };
                assert_eq!(m.live_preview, previews, "{system:?} {}: live_preview must match the engine", m.id);
                assert!(m.size_bytes > 0 && !m.languages.is_empty() && (1..=5).contains(&m.speed), "{}", m.id);
                assert!(!m.id.as_str().contains(':'), "{}: a colon marks a cloud model", m.id);
            }
        }
    }

    #[test]
    fn ids_are_unique() {
        for system in SYSTEMS {
            let mut ids: Vec<_> = catalog_for(system).into_iter().map(|m| m.id).collect();
            let n = ids.len();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), n, "{system:?}");
        }
    }

    /// The Windows engine before 0.12 had these models. Their ids stay, so the config files and the
    /// history of those users keep their meaning.
    #[test]
    fn windows_keeps_the_ids_of_the_old_windows_engine() {
        let windows: Vec<_> = catalog_for(System::Windows).into_iter().map(|m| m.id).collect();
        for id in [
            "parakeet-tdt-v3",
            "parakeet-tdt-v2",
            "whisper-large-v3-turbo",
            "whisper-large-v3",
            "whisper-medium",
            "whisper-small",
            "whisper-base",
            "whisper-tiny",
        ] {
            assert!(windows.contains(&ModelId::new(id)), "{id}");
        }
        assert!(find(System::Windows, "whisper-small").vocabulary, "whisper.cpp reads the dictionary words");
    }
}
