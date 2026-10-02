//! The speech model catalog (plan §3, "Models").

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
    /// WhisperKit with a variant from `argmaxinc/whisperkit-coreml`.
    Whisper { variant: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: ModelId,
    pub name: String,
    pub vendor: String,
    pub description: String,
    pub engine: EngineKind,
    pub size_bytes: u64,
    /// BCP 47 codes. v0.1 uses "en" only, but the catalog keeps the real list.
    pub languages: Vec<String>,
    /// Can do the final pass.
    pub final_pass: bool,
    /// Can stream a live preview.
    pub live_preview: bool,
    /// Supports dictionary vocabulary boosting.
    pub vocabulary: bool,
    /// 1 (slow) to 5 (fast), for the speed meter in Models.
    pub speed: u8,
    /// Word error rate in percent, if published. For display only.
    pub wer_percent: Option<f32>,
    pub recommended: bool,
}

const MB: u64 = 1_000_000;

pub fn default_model() -> ModelId {
    ModelId::new("parakeet-unified-en")
}

pub fn catalog() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: default_model(),
            name: "Parakeet Unified EN".into(),
            vendor: "NVIDIA".into(),
            description: "Fastest and most accurate for English. Shows your words live while you speak.".into(),
            engine: EngineKind::ParakeetUnified { streaming_tier: "70_2_2".into() },
            size_bytes: 595 * MB + 578 * MB + 98 * MB,
            languages: vec!["en".into()],
            final_pass: true,
            live_preview: true,
            vocabulary: true,
            speed: 5,
            wer_percent: Some(2.2),
            recommended: true,
        },
        ModelInfo {
            id: ModelId::new("whisper-large-v3"),
            name: "Whisper large-v3".into(),
            vendor: "OpenAI".into(),
            description: "Good with strong accents and mixed language. No live preview. Slower first start.".into(),
            engine: EngineKind::Whisper { variant: "openai_whisper-large-v3-v20240930_626MB".into() },
            size_bytes: 626 * MB,
            languages: vec!["en".into(), "multilingual".into()],
            final_pass: true,
            live_preview: false,
            vocabulary: false,
            speed: 2,
            wer_percent: None,
            recommended: false,
        },
        ModelInfo {
            id: ModelId::new("whisper-large-v3-turbo"),
            name: "Whisper large-v3 turbo".into(),
            vendor: "OpenAI".into(),
            description: "Faster, nearly as accurate as large-v3.".into(),
            engine: EngineKind::Whisper { variant: "openai_whisper-large-v3-v20240930_turbo_632MB".into() },
            size_bytes: 632 * MB,
            languages: vec!["en".into(), "multilingual".into()],
            final_pass: true,
            live_preview: false,
            vocabulary: false,
            speed: 3,
            wer_percent: None,
            recommended: false,
        },
        ModelInfo {
            id: ModelId::new("parakeet-eou-120m"),
            name: "Parakeet EOU 120M".into(),
            vendor: "NVIDIA".into(),
            description: "Small live preview for older Macs. No punctuation.".into(),
            engine: EngineKind::ParakeetEou,
            size_bytes: 222 * MB,
            languages: vec!["en".into()],
            final_pass: false,
            live_preview: true,
            vocabulary: false,
            speed: 5,
            wer_percent: Some(4.9),
            recommended: false,
        },
        ModelInfo {
            id: ModelId::new("whisper-small"),
            name: "Whisper small".into(),
            vendor: "OpenAI".into(),
            description: "Smallest Whisper. Lower accuracy. Good for a quick try.".into(),
            engine: EngineKind::Whisper { variant: "openai_whisper-small_216MB".into() },
            size_bytes: 217 * MB,
            languages: vec!["en".into(), "multilingual".into()],
            final_pass: true,
            live_preview: false,
            vocabulary: false,
            speed: 4,
            wer_percent: None,
            recommended: false,
        },
    ]
}

pub fn find(id: &ModelId) -> Option<ModelInfo> {
    catalog().into_iter().find(|m| &m.id == id)
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
    fn sizes_format() {
        assert_eq!(format_size(626 * MB), "626 MB");
        assert_eq!(format_size(1_271 * MB), "1.3 GB");
    }
}
