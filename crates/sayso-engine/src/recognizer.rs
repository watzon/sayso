//! The seam between the protocol and the speech library.
//!
//! The engine talks to models only through these traits. [`crate::sherpa`]
//! implements them with sherpa-onnx. The tests use a fake.

use crate::partials::Segment;
use crate::spec::ModelSpec;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

/// The folders of a model's extracted archives.
#[derive(Debug, Clone)]
pub struct ModelDirs {
    /// The main archive.
    pub main: PathBuf,
    /// The archive that streams the live preview, if the model has one.
    pub stream: Option<PathBuf>,
}

/// Loads models. Loading is slow, so the engine calls it on a worker thread.
pub trait Loader: Send + Sync {
    fn load(&self, spec: &ModelSpec, dirs: &ModelDirs) -> Result<Loaded>;
    /// Load only the final pass, for a model whose live preview is in memory
    /// already. None when the recipe has no final pass.
    fn load_final_pass(
        &self,
        spec: &ModelSpec,
        dirs: &ModelDirs,
    ) -> Result<Option<Arc<dyn FinalPass>>>;
}

/// A model in memory. A model has a final pass, a live preview, or both.
/// After an `unload` with `keep_preview`, the final pass of a model with both
/// is out and the live preview stays.
#[derive(Clone)]
pub struct Loaded {
    pub final_pass: Option<Arc<dyn FinalPass>>,
    pub preview: Option<Arc<dyn LivePreview>>,
}

/// The final pass: whole recordings to text.
pub trait FinalPass: Send + Sync {
    /// Transcribe one piece of 16 kHz mono audio. `language` is a language
    /// code or "auto". The engine cuts long recordings into pieces first.
    fn transcribe(&self, samples: &[f32], language: &str) -> Result<String>;
}

/// The live preview: a streaming model.
pub trait LivePreview: Send + Sync {
    /// Start a new stream.
    fn start(&self) -> Result<Box<dyn PreviewStream>>;
}

/// One stream of a live preview.
pub trait PreviewStream: Send {
    /// Add 16 kHz mono audio. This must be fast: it does not decode.
    fn accept(&mut self, samples: &[f32]);
    /// Decode the audio that is ready. Returns the text of the current segment.
    fn decode(&mut self) -> Segment;
    /// No more audio comes. Decode the rest and return the last segment,
    /// with `endpoint` set.
    fn finish(&mut self) -> Segment;
}
