//! The free functions of `sayso-sdk`: `find_engine`, `model_store`, `permission`, the
//! devices of the microphone, and the start of Sayso.

use crate::{error, outcome_text};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use sayso_protocol::ErrorCode;
use sayso_sdk::permission::{self, Permission};
use serde_json::{Value, json};

/// The engine program in the folder of a release file, as an outcome.
#[napi]
pub fn find_engine(folder: String) -> String {
    outcome_text(sayso_sdk::find_engine(&folder).map(|path| json!(path)).map_err(|e| error(Some(ErrorCode::Unsupported), e.to_string())))
}

/// The shared model store of this user.
#[napi]
pub fn model_store() -> String {
    sayso_sdk::model_store().to_string_lossy().into_owned()
}

/// `granted`, `not_determined`, or `denied`. It does not ask the user.
#[napi]
pub fn microphone_permission_status() -> &'static str {
    match permission::status() {
        Permission::Granted => "granted",
        Permission::NotDetermined => "not_determined",
        Permission::Denied => "denied",
    }
}

/// Show the question of the system when it has not asked yet. A process with no
/// `NSMicrophoneUsageDescription` gets no question, and the answer false.
#[napi(ts_return_type = "Promise<boolean>")]
pub fn microphone_permission_request(env: &Env) -> Result<Object<'_>> {
    let (deferred, promise) = env.create_deferred::<bool, _>()?;
    permission::request(move |granted| deferred.resolve(move |_| Ok(granted)));
    Ok(promise)
}

/// The page of the system settings with this permission, or null.
#[napi]
pub fn microphone_settings_url() -> Option<&'static str> {
    permission::settings_url()
}

/// The input devices, as JSON text: `[{ id, name, isDefault }]`.
#[napi]
pub fn devices() -> String {
    let devices: Vec<Value> = sayso_sdk::embedded::devices().into_iter().map(|d| json!({ "id": d.id, "name": d.name, "isDefault": d.is_default })).collect();
    Value::Array(devices).to_string()
}

/// Start the installed Sayso, as an outcome.
#[napi]
pub fn start_sayso() -> String {
    outcome_text(sayso_sdk::client::start_sayso().map(|()| Value::Null).map_err(|e| error(Some(ErrorCode::HostNotRunning), format!("cannot start Sayso: {e}"))))
}

/// The ways to start Sayso on this system, as JSON text: `[{ program, args }]`.
#[napi]
pub fn launchers() -> String {
    let launchers: Vec<Value> = sayso_sdk::client::launchers().into_iter().map(|l| json!({ "program": l.program, "args": l.args })).collect();
    Value::Array(launchers).to_string()
}

/// The constants of the client module, as JSON text.
#[napi]
pub fn client_constants() -> String {
    json!({
        "appId": sayso_sdk::client::APP_ID,
        "sdk": sayso_sdk::client::SDK,
        "startWaitMs": sayso_sdk::client::START_WAIT.as_millis() as u64,
    })
    .to_string()
}
