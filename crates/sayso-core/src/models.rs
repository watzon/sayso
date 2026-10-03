//! The speech model catalog (plan §3, "Models").
//!
//! [`catalog`] lists the local models of this system: the macOS catalog
//! below, or [`crate::models_onnx`] on Linux and Windows. Cloud models come
//! from the configured speech providers (see [`crate::speech`]).

use crate::languages;
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
    /// FluidAudio Parakeet EOU 120M: streaming preview only.
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
    /// A speech provider's model. Rust sends the audio; the sidecar never sees it.
    Remote { provider: String, model: String },
    /// A sherpa-onnx model in the Rust engine (Linux and Windows). `archive`
    /// is the file name, without `.tar.bz2`, in the sherpa-onnx `asr-models`
    /// release. `recipe` tells the engine how to load it: "parakeet_tdt",
    /// "parakeet_tdt_ctc", "zipformer_streaming", "whisper", "sense_voice", or
    /// "moonshine". `preview_archive` is a streaming model that comes with it
    /// for the live preview.
    Sherpa { recipe: String, archive: String, preview_archive: Option<String> },
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
            EngineKind::Whisper { .. } => Family::Whisper,
            EngineKind::Sherpa { recipe, .. } if recipe.starts_with("parakeet") => Family::Parakeet,
            EngineKind::Sherpa { recipe, .. } if recipe == "whisper" => Family::Whisper,
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
            EngineKind::Sherpa { .. } => "sherpa-onnx",
            _ => "FluidAudio",
        }
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
    /// Word error rate in percent on LibriSpeech test-clean, if published. For display only.
    pub wer_percent: Option<f32>,
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

    /// "English", "Japanese", "25 languages", or "Many languages" for a cloud model.
    pub fn language_label(&self) -> String {
        match self.languages.as_slice() {
            _ if self.is_remote() => "Many languages".into(),
            [one] => languages::display(one),
            many => format!("{} languages", many.len()),
        }
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
    #[cfg(target_os = "macos")]
    return ModelId::new("parakeet-unified-en");
    #[cfg(not(target_os = "macos"))]
    return crate::models_onnx::default_model();
}

/// The small streaming model that gives a live preview to models without one.
pub fn preview_fallback() -> ModelId {
    #[cfg(target_os = "macos")]
    return ModelId::new("parakeet-eou-120m");
    #[cfg(not(target_os = "macos"))]
    return crate::models_onnx::preview_fallback();
}

struct Entry {
    id: &'static str,
    name: &'static str,
    vendor: &'static str,
    description: &'static str,
    engine: EngineKind,
    size_mb: u64,
    languages: Vec<String>,
    speed: u8,
    wer: Option<f32>,
}

impl Entry {
    /// A batch model: final pass only, no dictionary words, macOS 14.
    fn build(self) -> ModelInfo {
        ModelInfo {
            id: ModelId::new(self.id),
            name: self.name.into(),
            vendor: self.vendor.into(),
            description: self.description.into(),
            engine: self.engine,
            size_bytes: self.size_mb * MB,
            languages: self.languages,
            final_pass: true,
            live_preview: false,
            vocabulary: false,
            speed: self.speed,
            wer_percent: self.wer,
            recommended: false,
            min_macos: 14,
        }
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
    #[cfg(target_os = "macos")]
    return apple_catalog();
    #[cfg(not(target_os = "macos"))]
    return crate::models_onnx::catalog();
}

/// The models that the Swift engine runs on a Mac, in display order.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
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
                description: "Fastest and most accurate for English. Shows your words live while you speak.",
                engine: EngineKind::ParakeetUnified { streaming_tier: "70_2_2".into() },
                size_mb: 595 + 578 + 98,
                languages: en(),
                speed: 5,
                wer: Some(2.2),
            }
            .build()
        },
        Entry {
            id: "parakeet-ultra",
            name: "Parakeet Ultra",
            vendor: "NVIDIA",
            description: "The best local model for European languages. As fast as Parakeet v3 and more accurate.",
            engine: tdt("ultra"),
            size_mb: 632,
            languages: languages::parakeet_v3(),
            speed: 5,
            wer: Some(2.1),
        }
        .build(),
        Entry {
            id: "parakeet-tdt-v3",
            name: "Parakeet TDT v3",
            vendor: "NVIDIA",
            description: "European languages. A smaller download than Ultra.",
            engine: tdt("v3"),
            size_mb: 483,
            languages: languages::parakeet_v3(),
            speed: 5,
            wer: Some(2.3),
        }
        .build(),
        ModelInfo {
            min_macos: 15,
            ..Entry {
                id: "parakeet-redux",
                name: "Parakeet Redux",
                vendor: "NVIDIA",
                description: "The smallest multilingual Parakeet. The first start takes about six minutes.",
                engine: tdt("redux"),
                size_mb: 220,
                languages: languages::parakeet_v3(),
                speed: 5,
                wer: Some(2.7),
            }
            .build()
        },
        Entry {
            id: "parakeet-tdt-v2",
            name: "Parakeet TDT v2",
            vendor: "NVIDIA",
            description: "The earlier Parakeet, without a live preview.",
            engine: tdt("v2"),
            size_mb: 465,
            languages: en(),
            speed: 5,
            wer: None,
        }
        .build(),
        ModelInfo {
            min_macos: 15,
            ..Entry {
                id: "phonon-2",
                name: "Phonon-2",
                vendor: "Fermion Research",
                description: "A compact Parakeet v3 for English. The first start takes about a minute.",
                engine: tdt("phonon2"),
                size_mb: 357,
                languages: en(),
                speed: 5,
                wer: Some(2.5),
            }
            .build()
        },
        Entry {
            id: "parakeet-tdt-ctc-110m",
            name: "Parakeet TDT-CTC 110M",
            vendor: "NVIDIA",
            description: "Small and quick, for older Macs.",
            engine: tdt("tdt_ctc_110m"),
            size_mb: 227,
            languages: en(),
            speed: 5,
            wer: Some(3.0),
        }
        .build(),
        Entry {
            id: "parakeet-ja",
            name: "Parakeet Japanese",
            vendor: "NVIDIA",
            description: "A Parakeet model trained for Japanese.",
            engine: tdt("ja"),
            size_mb: 625,
            languages: vec!["ja".into()],
            speed: 3,
            wer: None,
        }
        .build(),
        ModelInfo {
            final_pass: false,
            live_preview: true,
            ..Entry {
                id: "parakeet-eou-120m",
                name: "Parakeet EOU 120M",
                vendor: "NVIDIA",
                description: "Small live preview for models that have none. No punctuation.",
                engine: EngineKind::ParakeetEou,
                size_mb: 222,
                languages: en(),
                speed: 5,
                wer: Some(4.9),
            }
            .build()
        },
        // Whisper.
        Entry {
            id: "whisper-large-v3-turbo",
            name: "Whisper large-v3 turbo",
            vendor: "OpenAI",
            description: "Nearly as accurate as large-v3 and much faster.",
            engine: whisper("openai_whisper-large-v3-v20240930_turbo_632MB"),
            size_mb: 646,
            languages: languages::whisper(),
            speed: 3,
            wer: None,
        }
        .build(),
        Entry {
            id: "whisper-large-v3",
            name: "Whisper large-v3",
            vendor: "OpenAI",
            description: "Good with strong accents and mixed language. Slow first start.",
            engine: whisper("openai_whisper-large-v3_947MB"),
            size_mb: 948,
            languages: languages::whisper(),
            speed: 2,
            wer: None,
        }
        .build(),
        Entry {
            id: "whisper-large-v2",
            name: "Whisper large-v2",
            vendor: "OpenAI",
            description: "The earlier large model. Some languages work better with it.",
            engine: whisper("openai_whisper-large-v2_949MB"),
            size_mb: 952,
            languages: languages::whisper(),
            speed: 2,
            wer: None,
        }
        .build(),
        Entry {
            id: "whisper-distil-large-v3",
            name: "Distil-Whisper large-v3",
            vendor: "Hugging Face",
            description: "Close to large-v3 accuracy at several times the speed.",
            engine: whisper("distil-whisper_distil-large-v3_594MB"),
            size_mb: 595,
            languages: en(),
            speed: 4,
            wer: None,
        }
        .build(),
        Entry {
            id: "whisper-medium",
            name: "Whisper medium",
            vendor: "OpenAI",
            description: "Between small and large. A large download.",
            engine: whisper("openai_whisper-medium"),
            size_mb: 1530,
            languages: languages::whisper(),
            speed: 2,
            wer: None,
        }
        .build(),
        Entry {
            id: "whisper-small",
            name: "Whisper small",
            vendor: "OpenAI",
            description: "Lower accuracy. Good for a quick try.",
            engine: whisper("openai_whisper-small_216MB"),
            size_mb: 217,
            languages: languages::whisper(),
            speed: 4,
            wer: None,
        }
        .build(),
        Entry {
            id: "whisper-base",
            name: "Whisper base",
            vendor: "OpenAI",
            description: "Small and fast, with many mistakes.",
            engine: whisper("openai_whisper-base"),
            size_mb: 147,
            languages: languages::whisper(),
            speed: 5,
            wer: None,
        }
        .build(),
        Entry {
            id: "whisper-tiny",
            name: "Whisper tiny",
            vendor: "OpenAI",
            description: "The smallest model. For tests and very old Macs.",
            engine: whisper("openai_whisper-tiny"),
            size_mb: 77,
            languages: languages::whisper(),
            speed: 5,
            wer: None,
        }
        .build(),
        // Other families.
        ModelInfo {
            min_macos: 26,
            ..Entry {
                id: "apple-speech",
                name: "Apple Speech",
                vendor: "Apple",
                description: "The speech model of macOS. Fast. macOS downloads a language when you first use it.",
                engine: EngineKind::AppleSpeech,
                size_mb: 0,
                languages: languages::apple_speech(),
                speed: 5,
                wer: None,
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
                description: "Live preview and final pass with one model, in many languages.",
                engine: EngineKind::NemotronMultilingual { chunk_ms: 1120 },
                size_mb: 664,
                languages: languages::nemotron(),
                speed: 4,
                wer: None,
            }
            .build()
        },
        ModelInfo {
            live_preview: true,
            ..Entry {
                id: "nemotron-en",
                name: "Nemotron Streaming EN",
                vendor: "NVIDIA",
                description: "Live preview and final pass with one streaming model.",
                engine: EngineKind::Nemotron { chunk_ms: 1120 },
                size_mb: 627,
                languages: en(),
                speed: 4,
                wer: Some(2.3),
            }
            .build()
        },
        ModelInfo {
            min_macos: 15,
            ..Entry {
                id: "cohere-transcribe",
                name: "Cohere Transcribe",
                vendor: "Cohere",
                description: "The most accurate local model. The first dictation after a download takes about a minute. Set the language in Settings.",
                engine: EngineKind::Cohere,
                size_mb: 2190,
                languages: languages::cohere(),
                speed: 3,
                wer: Some(1.8),
            }
            .build()
        },
        ModelInfo {
            min_macos: 15,
            ..Entry {
                id: "canary-1b-v2",
                name: "Canary 1B v2",
                vendor: "NVIDIA",
                description: "A beta conversion. Slower than Parakeet.",
                engine: EngineKind::Canary,
                size_mb: 569,
                languages: en(),
                speed: 2,
                wer: None,
            }
            .build()
        },
        Entry {
            id: "sensevoice-small",
            name: "SenseVoice Small",
            vendor: "Alibaba",
            description: "Very fast. Strongest in Chinese, Cantonese, English, Japanese, and Korean.",
            engine: EngineKind::SenseVoice,
            size_mb: 240,
            languages: languages::sensevoice(),
            speed: 5,
            wer: None,
        }
        .build(),
        Entry {
            id: "paraformer-zh",
            name: "Paraformer Large",
            vendor: "Alibaba",
            description: "Mandarin only. Very fast.",
            engine: EngineKind::Paraformer,
            size_mb: 644,
            languages: vec!["zh".into()],
            speed: 5,
            wer: None,
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

/// The model that streams the live preview for `active`.
///
/// The active model does it when it can stream. If not, a local streaming
/// model that is on disk and understands the dictation language does it: the
/// small fallback model first, then the others in catalog order. `on_disk`
/// tells which local models are downloaded.
pub fn preview_for(active: &ModelInfo, language_setting: &str, on_disk: impl Fn(&ModelId) -> bool) -> Option<ModelId> {
    if active.live_preview {
        return Some(active.id.clone());
    }
    let language = active.language_for(language_setting);
    let fallback = preview_fallback();
    let mut candidates: Vec<ModelInfo> = catalog()
        .into_iter()
        .filter(|m| m.live_preview && on_disk(&m.id))
        .filter(|m| if language == languages::AUTO { m.is_multilingual() } else { m.languages.contains(&language) })
        .collect();
    candidates.sort_by_key(|m| m.id != fallback);
    candidates.first().map(|m| m.id.clone())
}

/// Human-readable size: "626 MB", "1.3 GB".
pub fn format_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1e9)
    } else {
        format!("{} MB", (bytes as f64 / 1e6).round() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_model_is_in_catalog_and_recommended() {
        let m = find(&default_model()).unwrap();
        assert!(m.recommended && m.final_pass && m.live_preview);
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
        assert!(fallback.live_preview && !fallback.final_pass);
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
        assert_eq!(preview_for(&model("parakeet-unified-en"), "en", on(&[])), Some(default_model()));

        // Whisper in English: the small fallback wins over the bigger streaming models.
        let whisper = model("whisper-large-v3-turbo");
        assert_eq!(preview_for(&whisper, "en", on(&all)), Some(preview_fallback()));
        assert_eq!(preview_for(&whisper, "en", on(&["parakeet-unified-en"])), Some(default_model()));
        assert_eq!(preview_for(&whisper, "en", on(&[])), None);

        // German or detection: only a multilingual streaming model can preview.
        assert_eq!(preview_for(&whisper, "de", on(&all)), Some(ModelId::new("nemotron-multilingual")));
        assert_eq!(preview_for(&whisper, "auto", on(&all)), Some(ModelId::new("nemotron-multilingual")));
        assert_eq!(preview_for(&whisper, "de", on(&["parakeet-eou-120m"])), None, "an English preview of German speech is wrong");

        // An English-only model ignores the setting, so its preview is English too.
        assert_eq!(preview_for(&model("parakeet-tdt-v2"), "de", on(&["parakeet-eou-120m"])), Some(preview_fallback()));
    }

    #[test]
    fn sizes_format() {
        assert_eq!(format_size(626 * MB), "626 MB");
        assert_eq!(format_size(1_271 * MB), "1.3 GB");
    }
}
