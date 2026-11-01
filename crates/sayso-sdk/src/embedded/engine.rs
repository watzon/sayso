//! The engine that an app ships: a [`SpeechEngine`] and [`EngineModels`] over
//! `sayso_engine_sdk::Engine`.
//!
//! The engine starts at the first call that needs it, and it starts again after
//! a crash. One thread passes the events of the engine on to each receiver of
//! [`SpeechEngine::events`], because a channel gives each event to one receiver only.

use super::{
    DownloadProgress, EngineError, EngineEvent, ModelId, ModelState, Role, Segment, SpeechEngine,
    SpeechPart, Turn,
};
use crossbeam_channel::{Receiver, Sender, bounded, select, unbounded};
use parking_lot::Mutex;
use sayso_engine_sdk::protocol::engine::Event as RawEvent;
use sayso_engine_sdk::wav::TempWav;
use sayso_engine_sdk::{
    Engine, EngineOptions, Error as SdkError, Event as SdkEvent, Stderr, TranscribeOptions,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// A model of the engine and its state on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub size_bytes: u64,
    pub languages: Vec<String>,
    pub roles: Vec<Role>,
    pub state: ModelState,
    pub recommended: bool,
}

impl ModelInfo {
    pub fn on_disk(&self) -> bool {
        matches!(
            self.state,
            ModelState::Downloaded | ModelState::Ready | ModelState::Optimizing
        )
    }

    pub fn has_role(&self, role: &Role) -> bool {
        self.roles.contains(role)
    }

    /// True when the model knows `language`, a code such as `en`. `auto` needs more than one language.
    pub fn understands(&self, language: &str) -> bool {
        if language == "auto" {
            self.languages.len() > 1
        } else {
            self.languages.iter().any(|l| l == language)
        }
    }
}

/// The models of an engine. [`super::Speech`] uses this beside [`SpeechEngine`].
pub trait EngineModels: Send + Sync {
    /// The catalog of the engine, with the state of each model on disk.
    fn catalog(&self) -> Result<Vec<ModelInfo>, EngineError>;
    fn download(
        &self,
        model: &str,
        size_bytes: Option<u64>,
        on_progress: &mut dyn FnMut(&DownloadProgress),
    ) -> Result<(), EngineError>;
    fn delete(&self, model: &str) -> Result<(), EngineError>;
    fn load(&self, model: &str) -> Result<(), EngineError>;
    fn unload(&self, model: &str) -> Result<(), EngineError>;
}

/// Starts the engine program and talks to it. Calls can come from many threads.
pub struct ProgramEngine {
    program: PathBuf,
    models_dir: PathBuf,
    store: Option<PathBuf>,
    running: Mutex<Option<Arc<Running>>>,
    watchers: Arc<Mutex<Vec<Sender<EngineEvent>>>>,
}

/// The engine process, the models that it holds, and its thread of events.
struct Running {
    engine: Engine,
    loaded: Mutex<HashSet<String>>,
    syncs: Sender<Sender<()>>,
}

impl ProgramEngine {
    /// `store` is the model store of this user. An engine with the feature
    /// `store_v1` uses it. Other engines use `models_dir`.
    pub fn new(program: PathBuf, models_dir: PathBuf, store: Option<PathBuf>) -> ProgramEngine {
        ProgramEngine {
            program,
            models_dir,
            store,
            running: Mutex::new(None),
            watchers: Arc::default(),
        }
    }

    /// Stop the engine, which frees the memory of its models. The next call starts it again.
    pub fn stop(&self) {
        *self.running.lock() = None;
    }

    /// True while the engine process runs.
    pub fn is_running(&self) -> bool {
        self.running
            .lock()
            .as_ref()
            .is_some_and(|running| running.engine.is_running())
    }

    fn running(&self) -> Result<Arc<Running>, EngineError> {
        let mut slot = self.running.lock();
        if let Some(running) = slot.as_ref().filter(|running| running.engine.is_running()) {
            return Ok(running.clone());
        }
        let mut options = EngineOptions::new(&self.program, &self.models_dir).stderr(
            Stderr::lines(|line| log::debug!(target: "sayso_engine", "{line}")),
        );
        if let Some(store) = &self.store {
            options = options.env("SAYSO_MODEL_STORE", store);
        }
        let engine = Engine::start(options).map_err(sdk_error)?;
        let (syncs, synced) = unbounded();
        let events = engine.events();
        let watchers = self.watchers.clone();
        std::thread::Builder::new()
            .name("sayso-sdk-engine".into())
            .spawn(move || fan_out(&events, &synced, &watchers))
            .map_err(|e| EngineError::new(format!("cannot start a thread: {e}")))?;
        let running = Arc::new(Running {
            engine,
            loaded: Mutex::default(),
            syncs,
        });
        *slot = Some(running.clone());
        Ok(running)
    }

    /// Run `f` with `model` in memory, also after a restart of the engine.
    fn with_model<T>(
        &self,
        model: &str,
        f: impl FnOnce(&Engine) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        let running = self.running()?;
        ensure_loaded(&running, model)?;
        f(&running.engine)
    }
}

fn ensure_loaded(running: &Running, model: &str) -> Result<(), EngineError> {
    if running.loaded.lock().contains(model) {
        return Ok(());
    }
    running.engine.load(model).map_err(sdk_error)?;
    running.loaded.lock().insert(model.to_string());
    Ok(())
}

impl EngineModels for ProgramEngine {
    fn catalog(&self) -> Result<Vec<ModelInfo>, EngineError> {
        let running = self.running()?;
        let catalog = running.engine.list_models().map_err(sdk_error)?;
        let states: HashMap<String, ModelState> = catalog
            .states
            .into_iter()
            .map(|state| (state.model, state.state))
            .collect();
        Ok(catalog
            .models
            .into_iter()
            .map(|model| ModelInfo {
                state: states
                    .get(&model.id)
                    .cloned()
                    .unwrap_or(ModelState::NotDownloaded),
                id: model.id,
                name: model.name,
                size_bytes: model.size_bytes,
                languages: model.languages,
                roles: model.roles,
                recommended: model.recommended,
            })
            .collect())
    }

    fn download(
        &self,
        model: &str,
        size_bytes: Option<u64>,
        on_progress: &mut dyn FnMut(&DownloadProgress),
    ) -> Result<(), EngineError> {
        self.running()?
            .engine
            .download_with(model, size_bytes, on_progress)
            .map_err(sdk_error)
    }

    fn delete(&self, model: &str) -> Result<(), EngineError> {
        let running = self.running()?;
        running.engine.delete(model).map_err(sdk_error)?;
        running.loaded.lock().remove(model);
        Ok(())
    }

    fn load(&self, model: &str) -> Result<(), EngineError> {
        ensure_loaded(&*self.running()?, model)
    }

    fn unload(&self, model: &str) -> Result<(), EngineError> {
        let running = self.running()?;
        if running.loaded.lock().remove(model) {
            running.engine.unload(model, false).map_err(sdk_error)?;
        }
        Ok(())
    }
}

impl SpeechEngine for ProgramEngine {
    fn load(&self, model: &ModelId) -> Result<(), EngineError> {
        EngineModels::load(self, model.as_str())
    }

    fn transcribe(
        &self,
        samples: &[f32],
        model: &ModelId,
        language: &str,
    ) -> Result<String, EngineError> {
        let options = TranscribeOptions {
            language: language.to_string(),
            ..TranscribeOptions::default()
        };
        self.with_model(model.as_str(), |engine| {
            Ok(engine
                .transcribe_f32(model.as_str(), samples, &options)
                .map_err(sdk_error)?
                .text)
        })
    }

    fn transcribe_segments(
        &self,
        samples: &[f32],
        model: &ModelId,
        language: &str,
    ) -> Result<Vec<Segment>, EngineError> {
        self.with_model(model.as_str(), |engine| {
            let options = TranscribeOptions {
                language: language.to_string(),
                segments: engine.has_feature("segments"),
                ..TranscribeOptions::default()
            };
            let transcript = engine
                .transcribe_f32(model.as_str(), samples, &options)
                .map_err(sdk_error)?;
            Ok(match transcript.segments {
                Some(segments) => segments
                    .into_iter()
                    .map(|segment| Segment {
                        text: segment.text.trim().to_string(),
                        ..segment
                    })
                    .collect(),
                None => {
                    let end_ms = samples.len() as u64 * 1000
                        / u64::from(sayso_engine_sdk::protocol::pcm::SAMPLE_RATE);
                    vec![Segment {
                        start_ms: 0,
                        end_ms,
                        text: transcript.text.trim().to_string(),
                        words: None,
                    }]
                }
            })
        })
    }

    fn detect_speech(
        &self,
        model: &ModelId,
        samples: &[f32],
    ) -> Result<Vec<SpeechPart>, EngineError> {
        let wav = TempWav::from_f32(samples)
            .map_err(|e| EngineError::new(format!("cannot write a temporary file: {e}")))?;
        self.with_model(model.as_str(), |engine| {
            engine.vad(model.as_str(), wav.path()).map_err(sdk_error)
        })
    }

    fn diarize(
        &self,
        model: &ModelId,
        samples: &[f32],
        speakers: Option<u32>,
    ) -> Result<Vec<Turn>, EngineError> {
        let wav = TempWav::from_f32(samples)
            .map_err(|e| EngineError::new(format!("cannot write a temporary file: {e}")))?;
        let reply = self.with_model(model.as_str(), |engine| {
            engine
                .diarize(model.as_str(), wav.path(), speakers)
                .map_err(sdk_error)
        })?;
        Ok(reply
            .turns
            .into_iter()
            .map(|turn| Turn {
                start_ms: turn.start_ms,
                end_ms: turn.end_ms,
                speaker: turn.speaker,
            })
            .collect())
    }

    fn start_stream(
        &self,
        session: u64,
        model: &ModelId,
        language: &str,
    ) -> Result<(), EngineError> {
        self.with_model(model.as_str(), |engine| {
            engine
                .stream_start(session, model.as_str(), Some(language))
                .map_err(sdk_error)
        })
    }

    fn start_vad(&self, session: u64, model: &ModelId) -> Result<(), EngineError> {
        self.with_model(model.as_str(), |engine| {
            engine.vad_start(session, model.as_str()).map_err(sdk_error)
        })
    }

    fn push_audio(&self, session: u64, samples: &[f32]) -> Result<(), EngineError> {
        // No start here: a stream of an engine that ended is gone.
        let running = self
            .running
            .lock()
            .clone()
            .ok_or_else(|| EngineError::new("the engine does not run"))?;
        running
            .engine
            .stream_audio_f32(session, samples)
            .map_err(sdk_error)
    }

    fn end_stream(&self, session: u64) {
        let Some(running) = self.running.lock().clone() else {
            return;
        };
        // The engine decodes the rest of the audio before its reply, and a live session does not wait for that.
        let spawned = std::thread::Builder::new()
            .name("sayso-sdk-end-stream".into())
            .spawn(move || {
                if let Err(e) = running.engine.stream_end(session) {
                    log::debug!("the end of stream {session} failed: {e}");
                }
            });
        if let Err(e) = spawned {
            log::warn!("cannot end stream {session}: {e}");
        }
    }

    fn end_vad(&self, session: u64) -> Result<(), EngineError> {
        let running = self
            .running
            .lock()
            .clone()
            .ok_or_else(|| EngineError::new("the engine does not run"))?;
        let ended = running.engine.stream_end(session).map_err(sdk_error);
        // The engine sent the last `speech_end` before the reply. Pass it on before this returns.
        let (ack, acked) = bounded(1);
        if running.syncs.send(ack).is_ok() {
            let _ = acked.recv();
        }
        ended.map(|_| ())
    }

    fn events(&self) -> Receiver<EngineEvent> {
        let (tx, rx) = unbounded();
        self.watchers.lock().push(tx);
        rx
    }
}

/// Pass the events of one engine process on to each watcher. The thread ends with the process.
fn fan_out(
    events: &Receiver<SdkEvent>,
    syncs: &Receiver<Sender<()>>,
    watchers: &Mutex<Vec<Sender<EngineEvent>>>,
) {
    let send = |event: SdkEvent| {
        let SdkEvent::Engine { event, .. } = event else {
            return;
        };
        let Some(event) = speech_event(event) else {
            return;
        };
        watchers
            .lock()
            .retain(|watcher| watcher.send(event.clone()).is_ok());
    };
    loop {
        select! {
            recv(events) -> event => match event {
                Ok(event) => send(event),
                Err(_) => return,
            },
            recv(syncs) -> ack => match ack {
                Ok(ack) => {
                    for event in events.try_iter() {
                        send(event);
                    }
                    let _ = ack.send(());
                }
                Err(_) => return,
            },
        }
    }
}

/// The events that live sessions use.
fn speech_event(event: RawEvent) -> Option<EngineEvent> {
    match event {
        RawEvent::Partial(p) => Some(EngineEvent::Partial {
            session: p.session,
            committed: p.committed,
            tentative: p.tentative,
        }),
        RawEvent::SpeechStart(s) => Some(EngineEvent::SpeechStart {
            session: s.session,
            start_ms: s.start_ms,
        }),
        RawEvent::SpeechEnd(s) => Some(EngineEvent::SpeechEnd {
            session: s.session,
            start_ms: s.start_ms,
            end_ms: s.end_ms,
        }),
        RawEvent::Log(log) => {
            log::debug!(target: "sayso_engine", "{}", log.message);
            None
        }
        _ => None,
    }
}

/// An error of the engine, with the `code` of its `error` reply.
fn sdk_error(error: SdkError) -> EngineError {
    let code = error.code().cloned();
    EngineError {
        code,
        message: error.to_string(),
    }
}
