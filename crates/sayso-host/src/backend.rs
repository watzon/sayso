//! [`SpeechEngine`] over the engine of the app (`SttBackend`) and the one
//! subscription of the host to its events (`Bus`), for jobs and live sessions.

use crate::bus::Bus;
use crossbeam_channel::Receiver;
use sayso_core::models::ModelId;
use sayso_core::stt::SessionOptions;
use sayso_platform::{PlatformError, SttBackend};
use sayso_protocol::{ErrorCode, Segment, Word};
use sayso_speech::{EngineError, EngineEvent, SpeechEngine, SpeechPart, Turn};
use std::sync::Arc;

pub struct BackendEngine {
    engine: Arc<dyn SttBackend>,
    /// None for jobs alone: they use no events.
    bus: Option<Arc<Bus>>,
}

impl BackendEngine {
    pub(crate) fn new(engine: Arc<dyn SttBackend>, bus: Option<Arc<Bus>>) -> BackendEngine {
        BackendEngine { engine, bus }
    }

    fn options(model: &sayso_speech::ModelId, language: &str, preview: bool) -> SessionOptions {
        let model = ModelId::from(model);
        SessionOptions { final_model: model.clone(), preview_model: preview.then_some(model), language: language.to_string(), vocabulary: Vec::new() }
    }

    /// True when the engine has `segments`, or did not list its features.
    fn has_segments(&self) -> bool {
        let features = self.engine.features();
        features.is_empty() || features.iter().any(|f| f == "segments")
    }
}

/// The error of an engine call, with the `code` of the engine's `error` reply.
fn engine_error(error: PlatformError) -> EngineError {
    EngineError { code: error.code().map(ErrorCode::parse), message: error.to_string() }
}

impl SpeechEngine for BackendEngine {
    fn load(&self, model: &sayso_speech::ModelId) -> Result<(), EngineError> {
        let model = ModelId::from(model);
        if self.engine.status(&model).is_usable() {
            return Ok(());
        }
        self.engine.load(&model).map_err(engine_error)
    }

    fn transcribe(&self, samples: &[f32], model: &sayso_speech::ModelId, language: &str) -> Result<String, EngineError> {
        Ok(self.engine.transcribe(samples, &Self::options(model, language, false)).map_err(engine_error)?.text)
    }

    /// An engine without `segments` gives one segment for the whole part. Jobs
    /// do not reach this case today: model resolution gives such an engine the
    /// timestamp level `none` (`resolve.rs`), as live sessions always did.
    fn transcribe_segments(&self, samples: &[f32], model: &sayso_speech::ModelId, language: &str) -> Result<Vec<Segment>, EngineError> {
        let options = Self::options(model, language, false);
        if !self.has_segments() {
            let text = self.engine.transcribe(samples, &options).map_err(engine_error)?.text;
            let end_ms = samples.len() as u64 * 1000 / u64::from(sayso_core::stt::SAMPLE_RATE);
            return Ok(vec![Segment { start_ms: 0, end_ms, text: text.trim().to_string(), words: None }]);
        }
        let found = self.engine.transcribe_segments(samples, &options).map_err(engine_error)?;
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

    fn detect_speech(&self, model: &sayso_speech::ModelId, samples: &[f32]) -> Result<Vec<SpeechPart>, EngineError> {
        let found = self.engine.detect_speech(&model.into(), samples).map_err(engine_error)?;
        Ok(found.into_iter().map(SpeechPart::from).collect())
    }

    fn diarize(&self, model: &sayso_speech::ModelId, samples: &[f32], speakers: Option<u32>) -> Result<Vec<Turn>, EngineError> {
        let found = self.engine.diarize(&model.into(), samples, speakers).map_err(engine_error)?;
        Ok(found.turns.into_iter().map(Turn::from).collect())
    }

    fn start_stream(&self, session: u64, model: &sayso_speech::ModelId, language: &str) -> Result<(), EngineError> {
        self.engine.start_stream(session, &Self::options(model, language, true)).map_err(engine_error)
    }

    fn start_vad(&self, session: u64, model: &sayso_speech::ModelId) -> Result<(), EngineError> {
        self.engine.start_vad(session, &model.into()).map_err(engine_error)
    }

    fn push_audio(&self, session: u64, samples: &[f32]) -> Result<(), EngineError> {
        self.engine.push_audio(session, samples).map_err(engine_error)
    }

    fn end_stream(&self, session: u64) {
        self.engine.end_stream(session);
    }

    fn end_vad(&self, session: u64) -> Result<(), EngineError> {
        let ended = self.engine.end_vad(session);
        // The client sent the last `speech_end` before the reply; the bus passes it on now.
        if let Some(bus) = &self.bus {
            bus.sync();
        }
        ended.map_err(engine_error)
    }

    fn events(&self) -> Receiver<EngineEvent> {
        match &self.bus {
            Some(bus) => bus.watch_speech(),
            None => crossbeam_channel::never(),
        }
    }
}
