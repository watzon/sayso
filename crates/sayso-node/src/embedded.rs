//! `sayso_sdk::embedded` for JavaScript: a session in the process of the app, with an
//! engine layer of the app (`docs/sdk-typescript.md`, "API": `embedded`, `connect_with`).

use crate::bridge::{Args, Bridge};
use crate::capture::JsSource;
use crate::{addon_error, error, id_of, outcome_text, parse_arg, spawn_promise, to_value};
use crossbeam_channel::{Receiver, Sender, bounded, select};
use napi::bindgen_prelude::{Env, Float32Array, Function, Object, Status};
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use parking_lot::Mutex;
use sayso_protocol::{ErrorCode, ErrorReply, Segment};
use sayso_sdk::deepgram::{Deepgram, DeepgramFinal};
use sayso_sdk::embedded::{
    App, CaptureMicrophone, EngineError, EngineEvent, EngineSource, Error as EmbeddedError, Event, Microphone, Mode, ModelId, Options, ProgramEngine, SessionOptions, Source, Speech as Inner,
    SpeechEngine, SpeechPart, Turn, VadRun,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JsOptions {
    app: JsApp,
    engine: JsProgram,
    temp_dir: PathBuf,
    final_pass: Option<JsFinalPass>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JsApp {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JsProgram {
    program: PathBuf,
    models_dir: PathBuf,
    store: Option<PathBuf>,
}

/// The final pass: Deepgram, or a function of JavaScript for the models in `models`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "kind", deny_unknown_fields)]
enum JsFinalPass {
    Deepgram { key: String, base_url: Option<String> },
    Function { models: Vec<String> },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JsSession {
    sources: Vec<Source>,
    final_model: String,
    preview_model: Option<String>,
    /// A VAD model id, or `basic` (the default).
    vad: Option<String>,
    language: Option<String>,
    silence_limit_ms: Option<u64>,
}

impl JsSession {
    fn options(self) -> SessionOptions {
        SessionOptions {
            sources: self.sources,
            final_model: ModelId::new(self.final_model),
            preview_model: self.preview_model.map(ModelId::new),
            vad: match self.vad.as_deref() {
                None | Some("basic") => VadRun::Basic,
                Some(model) => VadRun::Model(ModelId::new(model)),
            },
            language: self.language.unwrap_or_else(|| "auto".into()),
            silence_limit: self.silence_limit_ms.map(Duration::from_millis),
        }
    }
}

fn embedded_error(e: EmbeddedError) -> ErrorReply {
    match e {
        EmbeddedError::Engine(e) => ErrorReply { code: e.code, message: e.message, capability: None, reason: None },
        EmbeddedError::Session { code, message } => ErrorReply { code, message, capability: None, reason: None },
        EmbeddedError::Io(e) => error(None, format!("the file system refused: {e}")),
    }
}

fn engine_error(e: ErrorReply) -> EngineError {
    EngineError { code: e.code, message: e.message }
}

fn event_value(event: Event) -> crate::Outcome {
    match event {
        Event::Partial(partial) => to_value(&sayso_protocol::Event::Partial(partial)),
        Event::Segment(segment) => to_value(&sayso_protocol::Event::Segment(segment)),
        Event::SilenceLimit { session, silence_ms } => to_value(&sayso_protocol::Event::SilenceLimit { session, silence_ms }),
        other => Err(addon_error(format!("an event that this addon does not know: {other:?}"))),
    }
}

/// The final pass of the models of the app, in JavaScript. Everything else goes to the engine.
struct JsFinal {
    local: Arc<dyn SpeechEngine>,
    models: Vec<String>,
    bridge: Bridge,
}

impl JsFinal {
    fn owns(&self, model: &ModelId) -> bool {
        self.models.iter().any(|m| m == model.as_str())
    }

    fn text(&self, samples: &[f32], model: &ModelId, language: &str) -> Result<String, EngineError> {
        let answer = self.bridge.call("transcribe", json!({ "model": model.as_str(), "language": language }).to_string(), Some(samples.to_vec())).map_err(engine_error)?;
        serde_json::from_str::<String>(&answer).map_err(|_| EngineError::new("the final pass of the app must give a string"))
    }
}

impl SpeechEngine for JsFinal {
    fn load(&self, model: &ModelId) -> Result<(), EngineError> {
        if self.owns(model) { Ok(()) } else { self.local.load(model) }
    }

    fn transcribe(&self, samples: &[f32], model: &ModelId, language: &str) -> Result<String, EngineError> {
        if self.owns(model) { self.text(samples, model, language) } else { self.local.transcribe(samples, model, language) }
    }

    fn transcribe_segments(&self, samples: &[f32], model: &ModelId, language: &str) -> Result<Vec<Segment>, EngineError> {
        if !self.owns(model) {
            return self.local.transcribe_segments(samples, model, language);
        }
        let text = self.text(samples, model, language)?;
        let end_ms = samples.len() as u64 * 1000 / u64::from(sayso_protocol::pcm::SAMPLE_RATE);
        Ok(vec![Segment { start_ms: 0, end_ms, text: text.trim().to_string(), words: None }])
    }

    fn detect_speech(&self, model: &ModelId, samples: &[f32]) -> Result<Vec<SpeechPart>, EngineError> {
        self.local.detect_speech(model, samples)
    }

    fn diarize(&self, model: &ModelId, samples: &[f32], speakers: Option<u32>) -> Result<Vec<Turn>, EngineError> {
        self.local.diarize(model, samples, speakers)
    }

    fn start_stream(&self, session: u64, model: &ModelId, language: &str) -> Result<(), EngineError> {
        self.local.start_stream(session, model, language)
    }

    fn start_vad(&self, session: u64, model: &ModelId) -> Result<(), EngineError> {
        self.local.start_vad(session, model)
    }

    fn push_audio(&self, session: u64, samples: &[f32]) -> Result<(), EngineError> {
        self.local.push_audio(session, samples)
    }

    fn end_stream(&self, session: u64) {
        self.local.end_stream(session);
    }

    fn end_vad(&self, session: u64) -> Result<(), EngineError> {
        self.local.end_vad(session)
    }

    fn events(&self) -> Receiver<EngineEvent> {
        self.local.events()
    }
}

/// What `connect_with` puts around the engine.
type Layer = Box<dyn FnOnce(Arc<dyn SpeechEngine>) -> Arc<dyn SpeechEngine> + Send>;
type EventFn = ThreadsafeFunction<String, (), String, Status, false, true>;
type LevelFn = ThreadsafeFunction<f64, (), f64, Status, false, true>;

struct Pump {
    stop: Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

struct Connected {
    inner: Arc<Inner>,
    /// The microphone of the SDK, when the app gave none.
    microphone: Option<Arc<CaptureMicrophone>>,
    program: Option<Arc<ProgramEngine>>,
}

/// `sayso_sdk::embedded::Speech`. Make it, then `connect`.
#[napi]
pub struct NativeEmbedded {
    state: Arc<Mutex<Option<Connected>>>,
    pump: Mutex<Option<Pump>>,
}

fn closed() -> ErrorReply {
    error(Some(ErrorCode::ConnectionLost), "this EmbeddedSpeech is closed or not connected")
}

fn inner_of(state: &Mutex<Option<Connected>>) -> Result<Arc<Inner>, ErrorReply> {
    state.lock().as_ref().map(|c| c.inner.clone()).ok_or_else(closed)
}

#[napi]
impl NativeEmbedded {
    #[napi(constructor)]
    pub fn new() -> NativeEmbedded {
        NativeEmbedded { state: Arc::default(), pump: Mutex::new(None) }
    }

    fn inner(&self) -> Result<Arc<Inner>, ErrorReply> {
        inner_of(&self.state)
    }

    /// Get ready. This starts no engine and loads no model.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn connect<'env>(&self, env: &'env Env, options_json: String, microphone: Option<Function<'_, Args, ()>>, final_pass: Option<Function<'_, Args, ()>>) -> napi::Result<Object<'env>> {
        let js: JsOptions = parse_arg(&options_json, "the options")?;
        if self.state.lock().is_some() {
            return Err(napi::Error::from_reason("this EmbeddedSpeech is connected already"));
        }
        let microphone = microphone.map(Bridge::new).transpose()?;
        let final_bridge = final_pass.map(Bridge::new).transpose()?;
        let program = Arc::new(ProgramEngine::new(js.engine.program, js.engine.models_dir, Some(js.engine.store.unwrap_or_else(sayso_sdk::model_store))));
        let (own_microphone, microphone): (Option<Arc<CaptureMicrophone>>, Arc<dyn Microphone>) = match microphone {
            Some(bridge) => (None, Arc::new(JsSource::new(bridge))),
            None => {
                let capture = Arc::new(CaptureMicrophone::default());
                (Some(capture.clone()), capture)
            }
        };
        let layer: Layer = match (js.final_pass, final_bridge) {
            (None, _) => Box::new(|engine| engine),
            (Some(JsFinalPass::Deepgram { key, base_url }), _) => {
                let client = match base_url {
                    Some(url) => Deepgram::with_base_url(key, url),
                    None => Deepgram::new(key),
                };
                Box::new(move |engine| Arc::new(DeepgramFinal::new(engine, client)))
            }
            (Some(JsFinalPass::Function { models }), Some(bridge)) => Box::new(move |engine| Arc::new(JsFinal { local: engine, models, bridge })),
            (Some(JsFinalPass::Function { .. }), None) => return Err(napi::Error::new(Status::InvalidArg, "a final pass of kind function needs its function")),
        };
        let options = Options { mode: Mode::Local, app: App::new(js.app.name), engine: EngineSource::Program(program.clone()), temp_dir: js.temp_dir, microphone: Some(microphone) };
        let state = self.state.clone();
        spawn_promise(env, "embedded-connect", move || {
            let inner = Inner::connect_with(options, layer).map_err(embedded_error)?;
            *state.lock() = Some(Connected { inner: Arc::new(inner), microphone: own_microphone, program: Some(program) });
            Ok(json!("local"))
        })
    }

    /// Start to give the events to `callback`, as JSON text of an outcome. Once.
    #[napi]
    pub fn listen(&self, callback: Function<'_, String, ()>) -> napi::Result<()> {
        let inner = self.inner().map_err(|e| napi::Error::from_reason(e.message))?;
        let mut slot = self.pump.lock();
        if slot.is_some() {
            return Err(napi::Error::from_reason("this EmbeddedSpeech has a listener already"));
        }
        let callback: EventFn = callback.build_threadsafe_function::<String>().callee_handled::<false>().weak::<true>().build_callback(|ctx| Ok(ctx.value))?;
        let (stop, stopped) = bounded::<()>(1);
        let events = inner.events();
        let thread = std::thread::Builder::new()
            .name("sayso-node-embedded-events".into())
            .spawn(move || {
                let deliver = |event: Event| {
                    callback.call(outcome_text(event_value(event)), ThreadsafeFunctionCallMode::NonBlocking);
                };
                loop {
                    select! {
                        recv(events) -> event => match event {
                            Ok(event) => deliver(event),
                            Err(_) => return,
                        },
                        recv(stopped) -> _ => {
                            events.try_iter().for_each(deliver);
                            return;
                        }
                    }
                }
            })
            .map_err(|e| napi::Error::from_reason(format!("cannot start a thread: {e}")))?;
        *slot = Some(Pump { stop, thread });
        Ok(())
    }

    /// The models of the engine and their state on disk. Starts the engine.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn models<'env>(&self, env: &'env Env) -> napi::Result<Object<'env>> {
        let state = self.state.clone();
        spawn_promise(env, "models", move || {
            let models = inner_of(&state)?.models().map_err(embedded_error)?;
            let models: Vec<Value> = models
                .into_iter()
                .map(|m| json!({ "id": m.id, "name": m.name, "size_bytes": m.size_bytes, "languages": m.languages, "roles": m.roles, "state": m.state, "recommended": m.recommended }))
                .collect();
            Ok(Value::Array(models))
        })
    }

    /// Download a model at once. `on_progress` gets the JSON text of each `DownloadProgress`.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn download<'env>(&self, env: &'env Env, model: String, size_bytes: Option<f64>, on_progress: Function<'_, String, ()>) -> napi::Result<Object<'env>> {
        let size_bytes = size_bytes.map(|size| id_of(size, "the size")).transpose()?;
        let progress: EventFn = on_progress.build_threadsafe_function::<String>().callee_handled::<false>().weak::<true>().build_callback(|ctx| Ok(ctx.value))?;
        let state = self.state.clone();
        spawn_promise(env, "download", move || {
            inner_of(&state)?
                .download(&model, size_bytes, |p| {
                    if let Ok(value) = to_value(p) {
                        progress.call(value.to_string(), ThreadsafeFunctionCallMode::NonBlocking);
                    }
                })
                .map_err(embedded_error)?;
            Ok(Value::Null)
        })
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn delete<'env>(&self, env: &'env Env, model: String) -> napi::Result<Object<'env>> {
        let state = self.state.clone();
        spawn_promise(env, "delete", move || inner_of(&state)?.delete(&model).map(|()| Value::Null).map_err(embedded_error))
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn load<'env>(&self, env: &'env Env, model: String) -> napi::Result<Object<'env>> {
        let state = self.state.clone();
        spawn_promise(env, "load", move || inner_of(&state)?.load(&model).map(|()| Value::Null).map_err(embedded_error))
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn unload<'env>(&self, env: &'env Env, model: String) -> napi::Result<Object<'env>> {
        let state = self.state.clone();
        spawn_promise(env, "unload", move || inner_of(&state)?.unload(&model).map(|()| Value::Null).map_err(embedded_error))
    }

    /// Start a live session. Waits while its models load. At most one session runs at a time.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn start_session<'env>(&self, env: &'env Env, options_json: String) -> napi::Result<Object<'env>> {
        let session: JsSession = parse_arg(&options_json, "the session options")?;
        let state = self.state.clone();
        spawn_promise(env, "session", move || Ok(json!(inner_of(&state)?.start_session(session.options()).map_err(embedded_error)?)))
    }

    /// Audio of the source `client`: 16 kHz mono samples from -1 to 1.
    #[napi]
    pub fn session_audio(&self, session: f64, samples: Float32Array) -> napi::Result<String> {
        let session = id_of(session, "the session")?;
        Ok(outcome_text(self.inner().and_then(|inner| inner.session_audio(session, &samples).map_err(embedded_error)).map(|()| Value::Null)))
    }

    /// End the audio, wait for the last parts, and give the text of the session.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn stop_session<'env>(&self, env: &'env Env, session: f64) -> napi::Result<Object<'env>> {
        let session = id_of(session, "the session")?;
        let state = self.state.clone();
        spawn_promise(env, "stop", move || Ok(json!(inner_of(&state)?.stop_session(session).map_err(embedded_error)?)))
    }

    /// Stop the session and forget the parts that wait. Returns at once.
    #[napi]
    pub fn cancel_session(&self) {
        if let Ok(inner) = self.inner() {
            inner.cancel_session();
        }
    }

    fn capture_microphone(&self) -> napi::Result<Arc<CaptureMicrophone>> {
        let state = self.state.lock();
        let connected = state.as_ref().ok_or_else(|| napi::Error::from_reason(closed().message))?;
        connected.microphone.clone().ok_or_else(|| napi::Error::from_reason("the app gave its own microphone, so the microphone of the SDK has no settings"))
    }

    /// Use this input device from the next session on. Null: the default device.
    #[napi]
    pub fn set_device(&self, device: Option<String>) -> napi::Result<()> {
        self.capture_microphone()?.set_device(device);
        Ok(())
    }

    /// Call `on_level` with the display level (0 to 1) of each frame of the next sessions. Null: no calls.
    #[napi]
    pub fn set_level(&self, on_level: Option<Function<'_, f64, ()>>) -> napi::Result<()> {
        let microphone = self.capture_microphone()?;
        match on_level {
            None => microphone.set_level(None),
            Some(function) => {
                let level: LevelFn = function.build_threadsafe_function::<f64>().callee_handled::<false>().weak::<true>().build_callback(|ctx| Ok(ctx.value))?;
                microphone.set_level(Some(Arc::new(move |value: f32| {
                    level.call(f64::from(value), ThreadsafeFunctionCallMode::NonBlocking);
                })));
            }
        }
        Ok(())
    }

    /// Start the capture now, before the models of a session load, so the first words are not lost.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn open_early<'env>(&self, env: &'env Env) -> napi::Result<Object<'env>> {
        let microphone = self.capture_microphone()?;
        spawn_promise(env, "open-early", move || microphone.open_early().map(|()| Value::Null).map_err(|message| error(Some(ErrorCode::Permission), message)))
    }

    /// Stop a capture of `openEarly` that no session took.
    #[napi]
    pub fn close_early(&self) -> napi::Result<()> {
        self.capture_microphone()?.close_early();
        Ok(())
    }

    /// Stop the session, stop the engine, and stop the events after the last one.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn close<'env>(&self, env: &'env Env) -> napi::Result<Object<'env>> {
        let connected = self.state.lock().take();
        let pump = self.pump.lock().take();
        spawn_promise(env, "close", move || {
            close_connected(connected, pump);
            Ok(Value::Null)
        })
    }
}

fn close_connected(connected: Option<Connected>, pump: Option<Pump>) {
    if let Some(connected) = &connected {
        connected.inner.cancel_session();
        if let Some(microphone) = &connected.microphone {
            microphone.close_early();
        }
        if let Some(program) = &connected.program {
            program.stop();
        }
    }
    if let Some(pump) = pump {
        let _ = pump.stop.send(());
        let _ = pump.thread.join();
    }
    drop(connected);
}

impl Default for NativeEmbedded {
    fn default() -> NativeEmbedded {
        NativeEmbedded::new()
    }
}

impl Drop for NativeEmbedded {
    fn drop(&mut self) {
        let connected = self.state.lock().take();
        let pump = self.pump.lock().take();
        if connected.is_some() || pump.is_some() {
            let _ = std::thread::Builder::new().name("sayso-node-drop".into()).spawn(move || close_connected(connected, pump));
        }
    }
}
