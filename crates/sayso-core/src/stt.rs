//! Types shared by speech engines and the dictation pipeline.

use crate::models::ModelId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelStatus {
    NotDownloaded,
    Downloading { fraction: f32, bytes_done: u64, bytes_total: u64 },
    /// First load: Core ML compiles the model for the Neural Engine.
    Optimizing,
    /// Downloaded, not loaded in memory.
    Downloaded,
    Ready,
    /// `message` is the English detail from the engine. `reason` is the stable
    /// code that the app translates. It is `None` for an older engine, or for a
    /// code that this version does not know.
    Failed {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<EngineReason>,
    },
}

/// Why a speech engine or its language model failed or is not available. The
/// engines send the code as the `reason` field (see `sayso-engine-client/NOTES.md`),
/// and the app shows a translated sentence for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineReason {
    /// A download ended, but the model files are not complete.
    IncompleteDownload,
    /// A download stopped with an error.
    DownloadFailed,
    /// The model is on disk, but loading it failed.
    LoadFailed,
    /// The model needs a discrete GPU with a Vulkan driver, and the engine found none.
    NoGpu,
    /// This Mac does not support Apple Intelligence.
    DeviceNotEligible,
    /// Apple Intelligence is off in System Settings.
    IntelligenceOff,
    /// macOS still downloads the Apple Intelligence model.
    ModelNotReady,
    /// Apple Intelligence needs macOS 26 or later.
    #[serde(rename = "needs_macos_26")]
    NeedsMacos26,
    /// The language model is not available, and the engine has no better reason.
    Unavailable,
    /// The language model gave no reply in time.
    Timeout,
    /// The language model refused the text.
    Refused,
    /// The language model failed in another way.
    Failed,
}

impl EngineReason {
    /// The reason for a code, or `None` for a code that this version does not know.
    pub fn from_code(code: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(code.to_string())).ok()
    }

    /// The code as the engines send it, for example `incomplete_download`.
    pub fn code(self) -> &'static str {
        match self {
            EngineReason::IncompleteDownload => "incomplete_download",
            EngineReason::DownloadFailed => "download_failed",
            EngineReason::LoadFailed => "load_failed",
            EngineReason::NoGpu => "no_gpu",
            EngineReason::DeviceNotEligible => "device_not_eligible",
            EngineReason::IntelligenceOff => "intelligence_off",
            EngineReason::ModelNotReady => "model_not_ready",
            EngineReason::NeedsMacos26 => "needs_macos_26",
            EngineReason::Unavailable => "unavailable",
            EngineReason::Timeout => "timeout",
            EngineReason::Refused => "refused",
            EngineReason::Failed => "failed",
        }
    }
}

impl ModelStatus {
    pub fn is_usable(&self) -> bool {
        matches!(self, ModelStatus::Ready)
    }
    pub fn is_on_disk(&self) -> bool {
        matches!(self, ModelStatus::Ready | ModelStatus::Downloaded | ModelStatus::Optimizing)
    }
}

/// Options for one dictation session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionOptions {
    pub final_model: ModelId,
    /// None turns the live preview off.
    pub preview_model: Option<ModelId>,
    /// The `dictation.language` setting. A backend sends each model the nearest
    /// value that the model supports (see `ModelInfo::language_for`).
    pub language: String,
    /// Dictionary words for vocabulary boosting or the prompt.
    pub vocabulary: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub text: String,
    pub model: ModelId,
    /// Time the final pass took, without loading.
    pub elapsed_ms: u64,
}

/// Something the engine reports without a request.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    ModelStatus { model: ModelId, status: ModelStatus },
    /// Live preview. `committed` will not change. `tentative` may still change.
    Partial { session: u64, committed: String, tentative: String },
    /// The engine process stopped. The client restarts it.
    Crashed { message: String },
    Restarted,
    Log { level: String, message: String },
}

/// Audio sample rate everything in Sayso uses after capture.
pub const SAMPLE_RATE: u32 = 16_000;

/// Samples in -1.0..=1.0 to 16-bit PCM. Values outside the range are clamped.
pub fn to_pcm16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|s| (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16)
        .collect()
}

/// A complete 16 kHz mono 16-bit WAV file.
pub fn encode_wav(pcm: &[i16]) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + pcm.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in pcm {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_clamps() {
        let pcm = to_pcm16(&[0.0, 1.0, -1.0, 2.0, -2.0, 0.5]);
        assert_eq!(pcm, vec![0, 32767, -32767, 32767, -32767, 16384]);
    }

    #[test]
    fn wav_header_is_valid() {
        let wav = encode_wav(&[1, 2, 3]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36 + 6);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
        assert_eq!(wav.len(), 44 + 6);
    }

    #[test]
    fn reason_codes_round_trip_and_unknown_codes_are_none() {
        use EngineReason::*;
        for reason in [
            IncompleteDownload, DownloadFailed, LoadFailed, NoGpu, DeviceNotEligible, IntelligenceOff,
            ModelNotReady, NeedsMacos26, Unavailable, Timeout, Refused, Failed,
        ] {
            assert_eq!(EngineReason::from_code(reason.code()), Some(reason));
        }
        assert_eq!(EngineReason::from_code("a_future_reason"), None);
    }

    #[test]
    fn failed_status_without_a_reason_has_no_reason_field() {
        let status = ModelStatus::Failed { message: "x".into(), reason: None };
        assert_eq!(serde_json::to_string(&status).unwrap(), r#"{"state":"failed","message":"x"}"#);
        let old: ModelStatus = serde_json::from_str(r#"{"state":"failed","message":"x"}"#).unwrap();
        assert_eq!(old, status);
    }
}
