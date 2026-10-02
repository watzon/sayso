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
