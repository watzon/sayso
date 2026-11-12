//! `sayso_sdk::client` and `sayso_sdk::deepgram` for JavaScript.

use crate::bridge::{Args, Bridge};
use crate::{error, outcome_text, parse_arg, spawn_promise, to_value};
use crossbeam_channel::{Sender, bounded, select};
use napi::bindgen_prelude::{Env, Float32Array, Function, Object, Status};
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use parking_lot::{Mutex, RwLock};
use sayso_protocol::{Capability, ErrorCode, ErrorReply, Fields};
use sayso_sdk::client::{AppHello, Client, Error as ClientError, Event};
use sayso_sdk::deepgram::{self, Deepgram};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// How to connect: to which socket, as which app, and whether to start Sayso.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JsConnect {
    path: Option<PathBuf>,
    app: Option<JsAppHello>,
    #[serde(default)]
    start: bool,
    wait_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JsAppHello {
    name: Option<String>,
    #[serde(default)]
    capabilities: Vec<Capability>,
}

fn client_error(e: ClientError) -> ErrorReply {
    match e {
        ClientError::Host(reply) => reply,
        ClientError::Io(e) if e.kind() == std::io::ErrorKind::TimedOut => error(Some(ErrorCode::HostNotRunning), e.to_string()),
        ClientError::Io(e) => error(Some(ErrorCode::HostNotRunning), format!("cannot talk to Sayso: {e}")),
        ClientError::Gone => error(Some(ErrorCode::ConnectionLost), "the connection to Sayso is gone"),
        ClientError::Protocol(message) => error(None, message),
    }
}

fn event_value(event: Event) -> crate::Outcome {
    Ok(match event {
        Event::Host { id, event } => json!({ "kind": "host", "id": id, "event": to_value(&event)? }),
        Event::Error(e) => json!({ "kind": "error", "error": e }),
        Event::HostGone => json!({ "kind": "host_gone" }),
    })
}

fn fields_of(text: &str) -> napi::Result<Fields> {
    parse_arg::<Fields>(text, "the fields")
}

type EventFn = ThreadsafeFunction<String, (), String, Status, false, true>;

struct Pump {
    stop: Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

type Slot = Arc<RwLock<Option<Client>>>;

fn gone() -> ErrorReply {
    error(Some(ErrorCode::ConnectionLost), "this Client is closed or not connected")
}

/// `sayso_sdk::client::Client`. Make it, then `connect`.
#[napi]
pub struct NativeClient {
    client: Slot,
    pump: Mutex<Option<Pump>>,
}

#[napi]
impl NativeClient {
    #[napi(constructor)]
    pub fn new() -> NativeClient {
        NativeClient { client: Arc::default(), pump: Mutex::new(None) }
    }

    /// Connect and send `hello`. With `start`, start Sayso when it does not run: with the
    /// function `start` of JavaScript, or else as `start_sayso` does.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn connect<'env>(&self, env: &'env Env, how_json: String, start: Option<Function<'_, Args, ()>>) -> napi::Result<Object<'env>> {
        let how: JsConnect = parse_arg(&how_json, "the connect options")?;
        let start = start.map(Bridge::new).transpose()?;
        if self.client.read().is_some() {
            return Err(napi::Error::from_reason("this Client is connected already"));
        }
        let slot = self.client.clone();
        spawn_promise(env, "client-connect", move || {
            let path = match how.path {
                Some(path) => path,
                None => sayso_protocol::socket::client_socket_path().map_err(|e| error(Some(ErrorCode::HostNotRunning), e.to_string()))?,
            };
            let app = how.app.map(|app| AppHello { name: app.name, capabilities: app.capabilities }).unwrap_or_default();
            let client = if how.start {
                let wait = how.wait_ms.map(Duration::from_millis).unwrap_or(sayso_sdk::client::START_WAIT);
                let start_fn = || match &start {
                    Some(bridge) => bridge.call("start", "{}".into(), None).map(|_| ()).map_err(|e| std::io::Error::other(e.message)),
                    None => sayso_sdk::client::start_sayso(),
                };
                Client::connect_or_start_as(&path, &app, wait, start_fn)
            } else {
                Client::connect_as(&path, &app)
            }
            .map_err(client_error)?;
            let hello = to_value(client.hello())?;
            *slot.write() = Some(client);
            Ok(hello)
        })
    }

    /// Start to give the events to `callback`, as JSON text of an outcome. Once.
    #[napi]
    pub fn listen(&self, callback: Function<'_, String, ()>) -> napi::Result<()> {
        let events = self.client.read().as_ref().map(Client::events).ok_or_else(|| napi::Error::from_reason(gone().message))?;
        let mut slot = self.pump.lock();
        if slot.is_some() {
            return Err(napi::Error::from_reason("this Client has a listener already"));
        }
        let callback: EventFn = callback.build_threadsafe_function::<String>().callee_handled::<false>().weak::<true>().build_callback(|ctx| Ok(ctx.value))?;
        let (stop, stopped) = bounded::<()>(1);
        let thread = std::thread::Builder::new()
            .name("sayso-node-client-events".into())
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

    /// The reply of the host to `hello`, as an outcome.
    #[napi]
    pub fn hello(&self) -> String {
        outcome_text(self.client.read().as_ref().ok_or_else(gone).and_then(|client| to_value(client.hello())))
    }

    /// Send the request `kind` with `fields` (JSON text of an object), and wait for its reply: the fields of `ok`.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn request<'env>(&self, env: &'env Env, kind: String, fields_json: String) -> napi::Result<Object<'env>> {
        let fields = fields_of(&fields_json)?;
        let slot = self.client.clone();
        spawn_promise(env, "request", move || {
            let client = slot.read();
            let reply = client.as_ref().ok_or_else(gone)?.request(&kind, fields).map_err(client_error)?;
            to_value(&reply.fields)
        })
    }

    /// Send the request `kind` with no `id`, and do not wait. An `error` comes as an event.
    #[napi]
    pub fn send(&self, kind: String, fields_json: String) -> napi::Result<String> {
        let fields = fields_of(&fields_json)?;
        let client = self.client.read();
        Ok(outcome_text(client.as_ref().ok_or_else(gone).and_then(|client| client.send(&kind, fields).map_err(client_error)).map(|()| Value::Null)))
    }

    /// Connect again after `host_gone`, and send `hello` again. Gives the new reply of `hello`.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn reconnect<'env>(&self, env: &'env Env) -> napi::Result<Object<'env>> {
        let slot = self.client.clone();
        spawn_promise(env, "reconnect", move || {
            let mut client = slot.write();
            let hello = client.as_mut().ok_or_else(gone)?.reconnect().map_err(client_error)?;
            to_value(hello)
        })
    }

    /// Send `bye` and close the connection, then stop the events after the last one.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn close<'env>(&self, env: &'env Env) -> napi::Result<Object<'env>> {
        let slot = self.client.clone();
        let pump = self.pump.lock().take();
        spawn_promise(env, "close", move || {
            let client = slot.write().take();
            let result = client.map(Client::close).transpose();
            if let Some(pump) = pump {
                let _ = pump.stop.send(());
                let _ = pump.thread.join();
            }
            // A connection that is gone already needs no bye.
            match result {
                Ok(_) | Err(ClientError::Gone) | Err(ClientError::Io(_)) => Ok(Value::Null),
                Err(e) => Err(client_error(e)),
            }
        })
    }
}

impl Default for NativeClient {
    fn default() -> NativeClient {
        NativeClient::new()
    }
}

impl Drop for NativeClient {
    fn drop(&mut self) {
        let client = self.client.write().take();
        let pump = self.pump.lock().take();
        if client.is_none() && pump.is_none() {
            return;
        }
        let _ = std::thread::Builder::new().name("sayso-node-drop".into()).spawn(move || {
            drop(client);
            if let Some(pump) = pump {
                let _ = pump.stop.send(());
                let _ = pump.thread.join();
            }
        });
    }
}

/// `sayso_sdk::deepgram::Deepgram`: a small client of the HTTP API of Deepgram. It keeps the key in memory only.
#[napi]
pub struct NativeDeepgram {
    client: Deepgram,
}

fn deepgram_error(e: sayso_sdk::embedded::EngineError) -> ErrorReply {
    ErrorReply { code: e.code, message: e.message, capability: None, reason: None }
}

#[napi]
impl NativeDeepgram {
    #[napi(constructor)]
    pub fn new(key: String, base_url: Option<String>) -> NativeDeepgram {
        NativeDeepgram { client: base_url.map_or_else(|| Deepgram::new(key.clone()), |url| Deepgram::with_base_url(key.clone(), url)) }
    }

    /// Ask Deepgram if it knows the key.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn check_key<'env>(&self, env: &'env Env) -> napi::Result<Object<'env>> {
        let client = self.client.clone();
        spawn_promise(env, "deepgram-key", move || client.check_key().map(|()| Value::Null).map_err(deepgram_error))
    }

    /// The text of 16 kHz mono samples from -1 to 1.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn transcribe<'env>(&self, env: &'env Env, samples: Float32Array, language: String) -> napi::Result<Object<'env>> {
        let client = self.client.clone();
        let samples = samples.to_vec();
        spawn_promise(env, "deepgram", move || client.transcribe(&samples, &language).map(|text| json!(text)).map_err(deepgram_error))
    }
}

/// The constants of `sayso_sdk::deepgram`, as JSON text.
#[napi]
pub fn deepgram_constants() -> String {
    json!({ "model": deepgram::MODEL, "codeConnection": deepgram::CODE_CONNECTION, "codeKey": deepgram::CODE_KEY }).to_string()
}
