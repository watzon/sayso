//! `sayso_sdk::Speech` for JavaScript. The TypeScript class `Speech` wraps this one.

use crate::bridge::Bridge;
use crate::capture::JsSource;
use crate::values::{self, event_value, features_value, mode_name};
use crate::{error, id_of, outcome_text, parse_arg, spawn_promise, to_value};
use crossbeam_channel::{Receiver, Sender, bounded, select};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use parking_lot::Mutex;
use sayso_protocol::{ErrorCode, ErrorReply};
use sayso_sdk::{CaptureSources, Event, Options, Speech as Inner};
use serde_json::{Value, json};
use std::sync::Arc;

fn options(text: &str, microphone: Option<Bridge>, system_audio: Option<Bridge>, start: Option<Bridge>) -> Result<Options> {
    let mut options = values::options(text).map_err(|e| Error::new(Status::InvalidArg, e))?;
    if microphone.is_some() || system_audio.is_some() {
        options = options.capture(capture_sources(microphone, system_audio));
    }
    if let Some(start) = start {
        options.start = Arc::new(move || start.call("start", "{}".into(), None).map(|_| ()).map_err(|e| std::io::Error::other(e.message)));
    }
    Ok(options)
}

/// The sources of the app. A source that the app does not give is the real capture.
fn capture_sources(microphone: Option<Bridge>, system_audio: Option<Bridge>) -> CaptureSources {
    use sayso_capture::live::{LiveMicrophone, LiveSystemAudio};
    CaptureSources {
        microphone: match microphone {
            Some(bridge) => Arc::new(JsSource::new(bridge)),
            None => Arc::new(LiveMicrophone { device: None }),
        },
        system_audio: match system_audio {
            Some(bridge) => Arc::new(JsSource::new(bridge)),
            // Local mode has one process, so the capture leaves out the app.
            None => Arc::new(LiveSystemAudio { this_process: "app" }),
        },
    }
}

type EventFn = ThreadsafeFunction<String, (), String, Status, false, true>;

/// The thread that gives the events of the SDK to JavaScript. It does not hold the event
/// loop: the TypeScript layer does that while work is active ("Events").
struct Pump {
    stop: Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

fn pump(events: Receiver<Event>, stop: Receiver<()>, callback: EventFn) {
    let deliver = |event: Event| {
        callback.call(outcome_text(event_value(event)), ThreadsafeFunctionCallMode::NonBlocking);
    };
    loop {
        select! {
            recv(events) -> event => match event {
                Ok(event) => deliver(event),
                Err(_) => return,
            },
            recv(stop) -> _ => {
                // The events that came before the close still reach the app.
                events.try_iter().for_each(deliver);
                return;
            }
        }
    }
}

/// `sayso_sdk::Speech`. Make it, then `connect`.
#[napi]
pub struct NativeSpeech {
    inner: Arc<Mutex<Option<Arc<Inner>>>>,
    pump: Mutex<Option<Pump>>,
}

fn not_connected() -> ErrorReply {
    error(Some(ErrorCode::ConnectionLost), "this Speech is closed or not connected")
}

#[napi]
impl NativeSpeech {
    #[napi(constructor)]
    pub fn new() -> NativeSpeech {
        NativeSpeech { inner: Arc::default(), pump: Mutex::new(None) }
    }

    fn inner(&self) -> std::result::Result<Arc<Inner>, ErrorReply> {
        self.inner.lock().clone().ok_or_else(not_connected)
    }

    /// Connect with the options (JSON text). The functions are the sources of the app and
    /// the start of Sayso; each is `undefined` for the default of the SDK.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn connect<'env>(
        &self,
        env: &'env Env,
        options_json: String,
        microphone: Option<Function<'_, crate::bridge::Args, ()>>,
        system_audio: Option<Function<'_, crate::bridge::Args, ()>>,
        start: Option<Function<'_, crate::bridge::Args, ()>>,
    ) -> Result<Object<'env>> {
        let microphone = microphone.map(Bridge::new).transpose()?;
        let system_audio = system_audio.map(Bridge::new).transpose()?;
        let start = start.map(Bridge::new).transpose()?;
        let options = options(&options_json, microphone, system_audio, start)?;
        if self.inner.lock().is_some() {
            return Err(Error::new(Status::GenericFailure, "this Speech is connected already"));
        }
        let slot = self.inner.clone();
        spawn_promise(env, "connect", move || {
            let speech = Inner::connect(options)?;
            let mode = mode_name(speech.mode());
            *slot.lock() = Some(Arc::new(speech));
            Ok(json!(mode))
        })
    }

    /// Start to give the events to `callback`, as JSON text of an outcome. Once.
    #[napi]
    pub fn listen(&self, callback: Function<'_, String, ()>) -> Result<()> {
        let inner = self.inner().map_err(|e| Error::from_reason(e.message))?;
        let mut slot = self.pump.lock();
        if slot.is_some() {
            return Err(Error::from_reason("this Speech has a listener already"));
        }
        let callback: EventFn = callback.build_threadsafe_function::<String>().callee_handled::<false>().weak::<true>().build_callback(|ctx| Ok(ctx.value))?;
        let (stop, stopped) = bounded(1);
        let events = inner.events();
        let thread = std::thread::Builder::new()
            .name("sayso-node-events".into())
            .spawn(move || pump(events, stopped, callback))
            .map_err(|e| Error::from_reason(format!("cannot start a thread: {e}")))?;
        *slot = Some(Pump { stop, thread });
        Ok(())
    }

    #[napi]
    pub fn mode(&self) -> String {
        outcome_text(self.inner().map(|inner| json!(mode_name(inner.mode()))))
    }

    #[napi]
    pub fn features(&self) -> String {
        outcome_text(self.inner().map(|inner| features_value(&inner.features())))
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn list_models<'env>(&self, env: &'env Env, role: Option<String>) -> Result<Object<'env>> {
        let role = role.map(|role| parse_arg::<sayso_protocol::Role>(&json!(role).to_string(), "the role")).transpose()?;
        let inner = self.inner.clone();
        spawn_promise(env, "list-models", move || {
            let models = current(&inner)?.list_models(role)?;
            to_value(&models)
        })
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn download_model<'env>(&self, env: &'env Env, model: String, reason: String) -> Result<Object<'env>> {
        let inner = self.inner.clone();
        spawn_promise(env, "download", move || {
            current(&inner)?.download_model(&model, &reason)?;
            Ok(Value::Null)
        })
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn transcribe_file<'env>(&self, env: &'env Env, request_json: String) -> Result<Object<'env>> {
        let request: sayso_protocol::TranscribeFile = parse_arg(&request_json, "the request")?;
        let inner = self.inner.clone();
        spawn_promise(env, "transcribe", move || Ok(json!(current(&inner)?.transcribe_file(request)?)))
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn cancel_job<'env>(&self, env: &'env Env, job: f64) -> Result<Object<'env>> {
        let job = id_of(job, "the job")?;
        let inner = self.inner.clone();
        spawn_promise(env, "cancel", move || {
            current(&inner)?.cancel_job(job)?;
            Ok(Value::Null)
        })
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn start_session<'env>(&self, env: &'env Env, request_json: String) -> Result<Object<'env>> {
        let request: sayso_protocol::SessionStart = parse_arg(&request_json, "the request")?;
        let inner = self.inner.clone();
        spawn_promise(env, "session", move || to_value(&current(&inner)?.start_session(request)?))
    }

    /// Audio of the source `client`. It only sends, so it does not wait.
    #[napi]
    pub fn session_audio(&self, session: f64, samples: Int16Array) -> Result<String> {
        let session = id_of(session, "the session")?;
        Ok(outcome_text(self.inner().and_then(|inner| inner.session_audio(session, &samples)).map(|()| Value::Null)))
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn stop_session<'env>(&self, env: &'env Env, session: f64) -> Result<Object<'env>> {
        let session = id_of(session, "the session")?;
        let inner = self.inner.clone();
        spawn_promise(env, "stop", move || Ok(json!(current(&inner)?.stop_session(session)?)))
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn enhance<'env>(&self, env: &'env Env, request_json: String) -> Result<Object<'env>> {
        let request: sayso_protocol::Enhance = parse_arg(&request_json, "the request")?;
        let inner = self.inner.clone();
        spawn_promise(env, "enhance", move || to_value(&current(&inner)?.enhance(request)?))
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn subscribe<'env>(&self, env: &'env Env, events: Vec<String>) -> Result<Object<'env>> {
        let inner = self.inner.clone();
        spawn_promise(env, "subscribe", move || {
            let events: Vec<&str> = events.iter().map(String::as_str).collect();
            Ok(json!(current(&inner)?.subscribe(&events)?))
        })
    }

    #[napi(ts_return_type = "Promise<string>")]
    pub fn reconnect<'env>(&self, env: &'env Env) -> Result<Object<'env>> {
        let inner = self.inner.clone();
        spawn_promise(env, "reconnect", move || {
            current(&inner)?.reconnect()?;
            Ok(Value::Null)
        })
    }

    /// End the work, close the connection or stop the engine, and stop the events after
    /// the last one. A second call does nothing.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn close<'env>(&self, env: &'env Env) -> Result<Object<'env>> {
        let inner = self.inner.lock().take();
        let pump = self.pump.lock().take();
        spawn_promise(env, "close", move || {
            if let Some(inner) = &inner {
                inner.close();
            }
            if let Some(pump) = pump {
                let _ = pump.stop.send(());
                let _ = pump.thread.join();
            }
            // The last reference of the SDK ends here, or with the last call that still runs.
            drop(inner);
            Ok(Value::Null)
        })
    }
}

impl Default for NativeSpeech {
    fn default() -> NativeSpeech {
        NativeSpeech::new()
    }
}

/// The garbage collector took a `Speech` without `close()`. The close of the SDK can wait
/// for an engine to stop, so it runs on a thread, not on the JavaScript thread.
impl Drop for NativeSpeech {
    fn drop(&mut self) {
        let inner = self.inner.lock().take();
        let pump = self.pump.lock().take();
        if inner.is_none() && pump.is_none() {
            return;
        }
        let _ = std::thread::Builder::new().name("sayso-node-drop".into()).spawn(move || {
            if let Some(inner) = &inner {
                inner.close();
            }
            if let Some(pump) = pump {
                let _ = pump.stop.send(());
                let _ = pump.thread.join();
            }
        });
    }
}

fn current(slot: &Mutex<Option<Arc<Inner>>>) -> std::result::Result<Arc<Inner>, ErrorReply> {
    slot.lock().clone().ok_or_else(not_connected)
}
