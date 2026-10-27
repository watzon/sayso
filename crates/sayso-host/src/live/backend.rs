//! [`LiveEngine`] over the engine of the app (`SttBackend`) and the one
//! subscription of the host to its events (`Bus`).

use super::engine::{EngineError, LiveEngine};
use crate::bus::Bus;
use crossbeam_channel::Receiver;
use sayso_core::models::ModelId;
use sayso_core::stt::{EngineEvent, SessionOptions, Turn};
use sayso_platform::SttBackend;
use sayso_protocol::{Segment, Word};
use std::sync::Arc;

pub struct BackendEngine {
    engine: Arc<dyn SttBackend>,
    bus: Arc<Bus>,
}

impl BackendEngine {
    pub(crate) fn new(engine: Arc<dyn SttBackend>, bus: Arc<Bus>) -> BackendEngine {
        BackendEngine { engine, bus }
    }

    fn options(model: &ModelId, language: &str, preview: bool) -> SessionOptions {
        SessionOptions { final_model: model.clone(), preview_model: preview.then(|| model.clone()), language: language.to_string(), vocabulary: Vec::new() }
    }

    /// True when the engine has `segments`, or did not list its features.
    fn has_segments(&self) -> bool {
        let features = self.engine.features();
        features.is_empty() || features.iter().any(|f| f == "segments")
    }
}

impl LiveEngine for BackendEngine {
    fn load(&self, model: &ModelId) -> Result<(), EngineError> {
        if self.engine.status(model).is_usable() {
            return Ok(());
        }
        Ok(self.engine.load(model)?)
    }

    fn start_stream(&self, session: u64, model: &ModelId, language: &str) -> Result<(), EngineError> {
        Ok(self.engine.start_stream(session, &Self::options(model, language, true))?)
    }

    fn start_vad(&self, session: u64, model: &ModelId) -> Result<(), EngineError> {
        Ok(self.engine.start_vad(session, model)?)
    }

    fn push_audio(&self, session: u64, samples: &[f32]) -> Result<(), EngineError> {
        Ok(self.engine.push_audio(session, samples)?)
    }

    fn end_stream(&self, session: u64) {
        self.engine.end_stream(session);
    }

    fn end_vad(&self, session: u64) -> Result<(), EngineError> {
        let ended = self.engine.end_vad(session);
        // The client sent the last `speech_end` before the reply; the bus passes it on now.
        self.bus.sync();
        Ok(ended?)
    }

    /// An engine without `segments` gives one segment for the whole part.
    fn transcribe_segments(&self, samples: &[f32], model: &ModelId, language: &str) -> Result<Vec<Segment>, EngineError> {
        let options = Self::options(model, language, false);
        if !self.has_segments() {
            let text = self.engine.transcribe(samples, &options)?.text;
            let end_ms = samples.len() as u64 * 1000 / u64::from(sayso_core::stt::SAMPLE_RATE);
            return Ok(vec![Segment { start_ms: 0, end_ms, text: text.trim().to_string(), words: None }]);
        }
        let found = self.engine.transcribe_segments(samples, &options)?;
        Ok(found
            .segments
            .into_iter()
            .map(|s| Segment {
                start_ms: s.start_ms,
                end_ms: s.end_ms,
                text: s.text.trim().to_string(),
                words: (!s.words.is_empty()).then(|| s.words.into_iter().map(|w| Word { start_ms: w.start_ms, end_ms: w.end_ms, text: w.text }).collect()),
            })
            .collect())
    }

    fn diarize(&self, model: &ModelId, samples: &[f32]) -> Result<Vec<Turn>, EngineError> {
        Ok(self.engine.diarize(model, samples, None)?.turns)
    }

    fn events(&self) -> Receiver<EngineEvent> {
        self.bus.watch()
    }
}
