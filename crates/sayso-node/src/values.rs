//! The values that cross between JavaScript and Rust (`docs/sdk-typescript.md`, "Values").
//! No napi here, so `tests/values.rs` can test this file by itself: a test of a napi
//! addon cannot link without Node.

use sayso_protocol::{Capability, ErrorCode, ErrorReply};
use sayso_sdk::{ActiveMode, App, Engine, Event, Features, Mode, ModelChoices, Options, Timing};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

/// The result of a call, as the TypeScript layer reads it: `{"ok": value}` or `{"err": ErrorReply}`.
pub type Outcome = Result<Value, ErrorReply>;

/// The largest integer that a JavaScript number keeps exactly.
pub const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

pub fn error(code: Option<ErrorCode>, message: impl Into<String>) -> ErrorReply {
    ErrorReply { code, message: message.into(), capability: None, reason: None }
}

/// An error of the addon itself, not of the SDK. The TypeScript layer makes it a `SaysoError` with no code.
pub fn addon_error(message: impl Into<String>) -> ErrorReply {
    error(None, message)
}

/// A value for JavaScript, or an error when it has a number that JavaScript cannot keep.
pub fn to_value(value: &impl Serialize) -> Outcome {
    let value = serde_json::to_value(value).map_err(|e| addon_error(format!("cannot write a value for JavaScript: {e}")))?;
    check_safe(&value)?;
    Ok(value)
}

/// Fails for an integer that JavaScript would change in silence.
pub fn check_safe(value: &Value) -> Result<(), ErrorReply> {
    match value {
        Value::Number(n) => {
            let safe = match (n.as_u64(), n.as_i64()) {
                (Some(u), _) => u <= MAX_SAFE_INTEGER,
                (None, Some(i)) => i.unsigned_abs() <= MAX_SAFE_INTEGER,
                (None, None) => true,
            };
            if safe {
                Ok(())
            } else {
                Err(error(Some(ErrorCode::BadRequest), format!("the number {n} is larger than 2^53, so JavaScript cannot keep it exactly")))
            }
        }
        Value::Array(items) => items.iter().try_for_each(check_safe),
        Value::Object(map) => map.values().try_for_each(check_safe),
        _ => Ok(()),
    }
}

/// A number from JavaScript as a `u64`. Fails for a fraction, a negative number, or a number above 2^53.
pub fn integer(number: f64, what: &str) -> Result<u64, String> {
    if number.fract() == 0.0 && number >= 0.0 && number <= MAX_SAFE_INTEGER as f64 {
        Ok(number as u64)
    } else {
        Err(format!("{what} must be an integer from 0 to 2^53 - 1, not {number}"))
    }
}

/// An outcome as JSON text. A value with a number above 2^53 becomes an error.
pub fn outcome_text(outcome: Outcome) -> String {
    match outcome.and_then(|value| check_safe(&value).map(|()| value)) {
        Ok(value) => json!({ "ok": value }).to_string(),
        Err(e) => json!({ "err": e }).to_string(),
    }
}

/// Read a request from JavaScript (JSON text) as `T`.
pub fn parse<T: serde::de::DeserializeOwned>(text: &str, what: &str) -> Result<T, String> {
    serde_json::from_str(text).map_err(|e| format!("{what} is not valid: {e}"))
}

pub fn mode_name(mode: ActiveMode) -> &'static str {
    match mode {
        ActiveMode::Sayso => "sayso",
        ActiveMode::Local => "local",
    }
}

/// `Features` with the names of JavaScript.
pub fn features_value(features: &Features) -> Value {
    json!({
        "enhance": features.enhance,
        "subscribe": features.subscribe,
        "downloadQuestion": features.download_question,
        "systemAudio": features.system_audio,
        "capabilities": features.capabilities,
        "denied": features.denied,
        "host": features.host,
        "engine": features.engine,
    })
}

/// An event of `Speech` as the TypeScript type `SpeechEvent`.
pub fn event_value(event: Event) -> Outcome {
    Ok(match event {
        Event::Message(message) => json!({ "kind": "message", "message": to_value(&message)? }),
        Event::ModeChanged { mode, features } => json!({ "kind": "mode_changed", "mode": mode_name(mode), "features": features_value(&features) }),
        Event::Error(e) => json!({ "kind": "error", "error": e }),
        Event::Disconnected { code } => json!({ "kind": "disconnected", "code": code }),
    })
}

/// The options of `Speech.connect`, as the TypeScript layer writes them. The sources and
/// the start function are not JSON; the addon adds them.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JsOptions {
    mode: JsMode,
    app: JsApp,
    #[serde(default)]
    capabilities: Vec<Capability>,
    engine: Option<JsEngine>,
    #[serde(default)]
    models: JsModels,
    socket: Option<PathBuf>,
    #[serde(default)]
    timing: JsTiming,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum JsMode {
    Name(String),
    Sayso { sayso: JsSayso },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JsSayso {
    #[serde(default)]
    start: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JsApp {
    name: String,
}

/// Where the engine is: the folder of the release file or the program, and the folders of the models.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JsEngine {
    pub release: Option<PathBuf>,
    pub program: Option<PathBuf>,
    pub store: Option<PathBuf>,
    pub models_dir: Option<PathBuf>,
}

impl JsEngine {
    pub fn engine(self) -> Result<Engine, String> {
        let mut engine = match (self.release, self.program) {
            (Some(folder), None) => Engine::from_release(folder),
            (None, Some(program)) => Engine::from_program(program),
            _ => return Err("the engine needs one of release or program".into()),
        };
        if let Some(store) = self.store {
            engine = engine.store(store);
        }
        if let Some(folder) = self.models_dir {
            engine = engine.models_dir(folder);
        }
        Ok(engine)
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JsModels {
    final_model: Option<String>,
    preview_model: Option<String>,
    vad_model: Option<String>,
    diarization_model: Option<String>,
    language: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JsTiming {
    watch_ms: Option<u64>,
    retry_ms: Option<u64>,
    start_wait_ms: Option<u64>,
    hello_limit_ms: Option<u64>,
    keep_model_ms: Option<u64>,
}

impl JsTiming {
    fn timing(&self) -> Timing {
        let default = Timing::default();
        let ms = |value: Option<u64>, default: Duration| value.map(Duration::from_millis).unwrap_or(default);
        Timing {
            watch: ms(self.watch_ms, default.watch),
            retry: ms(self.retry_ms, default.retry),
            start_wait: ms(self.start_wait_ms, default.start_wait),
            hello_limit: ms(self.hello_limit_ms, default.hello_limit),
            keep_model: ms(self.keep_model_ms, default.keep_model),
        }
    }
}

/// The `Options` of `sayso-sdk` from the JSON text of the TypeScript layer.
pub fn options(text: &str) -> Result<Options, String> {
    let js: JsOptions = parse(text, "the options")?;
    let mode = match js.mode {
        JsMode::Name(name) => match name.as_str() {
            "local" => Mode::Local,
            "auto" => Mode::Auto,
            "sayso" => Mode::Sayso { start: false },
            other => return Err(format!("the mode must be local, auto, sayso, or {{ sayso: {{ start }} }}, not {other}")),
        },
        JsMode::Sayso { sayso } => Mode::Sayso { start: sayso.start },
    };
    let mut options = Options::new(mode, App::new(js.app.name)).capabilities(js.capabilities).timing(js.timing.timing()).models(ModelChoices {
        final_model: js.models.final_model,
        preview_model: js.models.preview_model,
        vad_model: js.models.vad_model,
        diarization_model: js.models.diarization_model,
        language: js.models.language,
    });
    if let Some(engine) = js.engine {
        options = options.engine(engine.engine()?);
    }
    if let Some(socket) = js.socket {
        options = options.socket(socket);
    }
    Ok(options)
}
