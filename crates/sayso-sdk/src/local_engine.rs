//! [`SpeechEngine`] and [`ModelRetention`] over the engine that the app ships.

use crate::protocol::{self, ErrorCode, ErrorReply, Segment};
use crossbeam_channel::{Receiver, Sender, never, select, unbounded};
use parking_lot::Mutex;
use sayso_engine_sdk::protocol::engine;
use sayso_engine_sdk::wav::TempWav;
use sayso_engine_sdk::{Engine, Event as EngineSdkEvent, TranscribeOptions};
use sayso_speech::{EngineError, EngineEvent, ModelId, ModelRetention, SpeechEngine, SpeechPart, Turn};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

pub(crate) struct LocalEngine {
    engine: Arc<Engine>,
    /// One load at a time, so two kinds of work do not load one model twice.
    loading: Mutex<()>,
    loaded: Mutex<HashSet<ModelId>>,
    holds: Mutex<HashMap<ModelId, ModelHold>>,
    keep_model: Duration,
    /// The receivers of [`SpeechEngine::events`].
    watchers: Arc<Mutex<Vec<Sender<EngineEvent>>>>,
    /// The models that a request of the app downloads now.
    downloads: Arc<Mutex<HashSet<String>>>,
    /// Asks the thread of the events to pass on all that came, and to answer.
    sync: Sender<Sender<()>>,
    me: Weak<LocalEngine>,
}

#[derive(Default)]
struct ModelHold {
    holders: usize,
    released: Option<Instant>,
}

fn code_of(error: &sayso_engine_sdk::Error) -> Option<ErrorCode> {
    error.code().cloned()
}

fn engine_error(error: sayso_engine_sdk::Error) -> EngineError {
    EngineError { code: code_of(&error), message: error.to_string() }
}

/// The error for the app. An `error` of the engine keeps its message and its reason.
pub(crate) fn reply_error(error: sayso_engine_sdk::Error) -> ErrorReply {
    match error {
        sayso_engine_sdk::Error::Engine(reply) => reply,
        other => ErrorReply { code: None, message: other.to_string(), capability: None, reason: None },
    }
}

impl LocalEngine {
    pub fn new(engine: Engine, keep_model: Duration, on_download: Arc<dyn Fn(protocol::Event) + Send + Sync>) -> Arc<LocalEngine> {
        let events = engine.events();
        let watchers: Arc<Mutex<Vec<Sender<EngineEvent>>>> = Arc::new(Mutex::new(Vec::new()));
        let downloads: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let (sync, sync_in) = unbounded();
        {
            let (watchers, downloads) = (watchers.clone(), downloads.clone());
            let spawned = std::thread::Builder::new().name("sayso-sdk-engine-events".into()).spawn(move || pass_events(&events, &sync_in, &watchers, &downloads, &*on_download));
            if let Err(e) = spawned {
                log::warn!("sdk: cannot start a thread for the events of the engine: {e}");
            }
        }
        Arc::new_cyclic(|me| LocalEngine { engine: Arc::new(engine), loading: Mutex::new(()), loaded: Mutex::new(HashSet::new()), holds: Mutex::new(HashMap::new()), keep_model, watchers, downloads, sync, me: me.clone() })
    }

    pub fn is_running(&self) -> bool {
        self.engine.is_running()
    }

    pub fn features(&self) -> Vec<String> {
        self.engine.features().to_vec()
    }

    pub fn has(&self, feature: &str) -> bool {
        // An engine that lists nothing is older than the features, and answers each request itself.
        self.engine.features().is_empty() || self.engine.has_feature(feature)
    }

    pub fn catalog(&self) -> Result<sayso_engine_sdk::Catalog, ErrorReply> {
        self.engine.list_models().map_err(reply_error)
    }

    /// Download at once. The events of the model go to the app, and the reply comes after the last of them.
    pub fn download(&self, model: &str, size_bytes: u64) -> Result<(), ErrorReply> {
        self.downloads.lock().insert(model.to_string());
        let done = self.engine.download_with(model, Some(size_bytes), |_| {});
        self.pass_on();
        self.downloads.lock().remove(model);
        done.map_err(reply_error)
    }

    /// Wait until the events that came before now went to their receivers.
    fn pass_on(&self) {
        let (done, wait) = crossbeam_channel::bounded(1);
        if self.sync.send(done).is_ok() {
            let _ = wait.recv();
        }
    }

    fn unload_when_unused(&self, model: &ModelId, released: Instant) {
        let unused = self.holds.lock().get(model).is_some_and(|hold| hold.holders == 0 && hold.released == Some(released));
        if !unused || !self.engine.has_feature("unload") {
            return;
        }
        let _one = self.loading.lock();
        if self.loaded.lock().remove(model)
            && let Err(e) = self.engine.unload(model.as_str(), false)
        {
            log::debug!("sdk: cannot unload {model}: {e}");
        }
    }
}

fn pass_events(events: &Receiver<EngineSdkEvent>, sync: &Receiver<Sender<()>>, watchers: &Mutex<Vec<Sender<EngineEvent>>>, downloads: &Mutex<HashSet<String>>, on_download: &(dyn Fn(protocol::Event) + Send + Sync)) {
    let pass = |event: EngineSdkEvent| {
        let EngineSdkEvent::Engine { event, .. } = event else { return };
        let speech = match event {
            engine::Event::Partial(partial) => EngineEvent::Partial { session: partial.session, committed: partial.committed, tentative: partial.tentative },
            engine::Event::SpeechStart(start) => EngineEvent::SpeechStart { session: start.session, start_ms: start.start_ms },
            engine::Event::SpeechEnd(end) => EngineEvent::SpeechEnd { session: end.session, start_ms: end.start_ms, end_ms: end.end_ms },
            engine::Event::ModelState(state) => {
                if downloads.lock().contains(&state.model) {
                    on_download(protocol::Event::ModelState(state));
                }
                return;
            }
            engine::Event::DownloadProgress(progress) => {
                if downloads.lock().contains(&progress.model) {
                    on_download(protocol::Event::DownloadProgress(progress));
                }
                return;
            }
            _ => return,
        };
        watchers.lock().retain(|watcher| watcher.send(speech.clone()).is_ok());
    };
    let mut events = events.clone();
    loop {
        select! {
            recv(events) -> event => match event {
                Ok(event) => pass(event),
                Err(_) => events = never(),
            },
            recv(sync) -> done => {
                let Ok(done) = done else { return };
                for event in events.try_iter() {
                    pass(event);
                }
                let _ = done.send(());
            }
        }
    }
}

fn options(language: &str, segments: bool) -> TranscribeOptions {
    TranscribeOptions { language: language.to_string(), vocabulary: Vec::new(), segments }
}

fn temporary(samples: &[f32]) -> Result<TempWav, EngineError> {
    TempWav::from_f32(samples).map_err(|e| EngineError::new(format!("cannot write a temporary file: {e}")))
}

impl SpeechEngine for LocalEngine {
    fn load(&self, model: &ModelId) -> Result<(), EngineError> {
        let _one = self.loading.lock();
        if self.loaded.lock().contains(model) {
            return Ok(());
        }
        self.engine.load(model.as_str()).map_err(engine_error)?;
        self.loaded.lock().insert(model.clone());
        Ok(())
    }

    fn transcribe(&self, samples: &[f32], model: &ModelId, language: &str) -> Result<String, EngineError> {
        Ok(self.engine.transcribe_f32(model.as_str(), samples, &options(language, false)).map_err(engine_error)?.text)
    }

    fn transcribe_segments(&self, samples: &[f32], model: &ModelId, language: &str) -> Result<Vec<Segment>, EngineError> {
        let found = self.engine.transcribe_f32(model.as_str(), samples, &options(language, self.engine.has_feature("segments"))).map_err(engine_error)?;
        Ok(match found.segments {
            Some(segments) => segments.into_iter().map(|segment| Segment { text: segment.text.trim().to_string(), words: segment.words.filter(|words| !words.is_empty()), ..segment }).collect(),
            None => {
                let end_ms = samples.len() as u64 * 1000 / u64::from(protocol::pcm::SAMPLE_RATE);
                vec![Segment { start_ms: 0, end_ms, text: found.text.trim().to_string(), words: None }]
            }
        })
    }

    fn detect_speech(&self, model: &ModelId, samples: &[f32]) -> Result<Vec<SpeechPart>, EngineError> {
        let wav = temporary(samples)?;
        self.engine.vad(model.as_str(), wav.path()).map_err(engine_error)
    }

    fn diarize(&self, model: &ModelId, samples: &[f32], speakers: Option<u32>) -> Result<Vec<Turn>, EngineError> {
        let wav = temporary(samples)?;
        let found = self.engine.diarize(model.as_str(), wav.path(), speakers).map_err(engine_error)?;
        Ok(found.turns.into_iter().map(|turn| Turn { start_ms: turn.start_ms, end_ms: turn.end_ms, speaker: turn.speaker }).collect())
    }

    fn start_stream(&self, session: u64, model: &ModelId, language: &str) -> Result<(), EngineError> {
        self.engine.stream_start(session, model.as_str(), Some(language)).map_err(engine_error)
    }

    fn start_vad(&self, session: u64, model: &ModelId) -> Result<(), EngineError> {
        self.engine.vad_start(session, model.as_str()).map_err(engine_error)
    }

    fn push_audio(&self, session: u64, samples: &[f32]) -> Result<(), EngineError> {
        self.engine.stream_audio_f32(session, samples).map_err(engine_error)
    }

    fn end_stream(&self, session: u64) {
        // The session does not wait for the last partial: the segments have the text.
        let engine = self.engine.clone();
        let spawned = std::thread::Builder::new().name("sayso-sdk-stream-end".into()).spawn(move || {
            if let Err(e) = engine.stream_end(session) {
                log::debug!("sdk: the end of a stream failed: {e}");
            }
        });
        if let Err(e) = spawned {
            log::debug!("sdk: cannot end a stream: {e}");
        }
    }

    fn end_vad(&self, session: u64) -> Result<(), EngineError> {
        let ended = self.engine.stream_end(session);
        // The last `speech_end` came before the reply, and goes to the receivers now.
        self.pass_on();
        ended.map(|_| ()).map_err(engine_error)
    }

    fn events(&self) -> Receiver<EngineEvent> {
        let (out, events) = unbounded();
        self.watchers.lock().push(out);
        events
    }
}

/// The models of work, until this drops. After the last holder, a model stays in memory for
/// [`LocalEngine::keep_model`], so the next job of a list finds it loaded.
struct Hold {
    engine: Arc<LocalEngine>,
    models: Vec<ModelId>,
}

impl Drop for Hold {
    fn drop(&mut self) {
        let now = Instant::now();
        let mut holds = self.engine.holds.lock();
        for model in &self.models {
            let hold = holds.entry(model.clone()).or_default();
            hold.holders = hold.holders.saturating_sub(1);
            if hold.holders > 0 {
                continue;
            }
            hold.released = Some(now);
            let engine = self.engine.me.clone();
            let (model, keep) = (model.clone(), self.engine.keep_model);
            // It keeps no engine alive while it waits.
            let spawned = std::thread::Builder::new().name("sayso-sdk-unload".into()).spawn(move || {
                std::thread::sleep(keep);
                if let Some(engine) = engine.upgrade() {
                    engine.unload_when_unused(&model, now);
                }
            });
            if let Err(e) = spawned {
                log::debug!("sdk: cannot start a thread to unload a model: {e}");
            }
        }
    }
}

impl ModelRetention for LocalEngine {
    fn hold(&self, models: Vec<ModelId>) -> Box<dyn Send> {
        let mut holds = self.holds.lock();
        for model in &models {
            let hold = holds.entry(model.clone()).or_default();
            hold.holders += 1;
            hold.released = None;
        }
        drop(holds);
        let Some(engine) = self.me.upgrade() else { return Box::new(()) };
        Box::new(Hold { engine, models })
    }
}
