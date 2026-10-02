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
    Failed { message: String },
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
}
