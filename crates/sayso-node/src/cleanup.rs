//! `sayso_sdk::cleanup` for JavaScript: the local cleanup models, their download, and the
//! cleanup runtime (`docs/sdk-typescript.md`, "API": `cleanup`).

use crate::{error, outcome_text, spawn_promise};
use napi::bindgen_prelude::{Env, Function, Object, Status};
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use sayso_protocol::{ErrorCode, ErrorReply};
use sayso_sdk::cleanup::{self, CleanError, Cleaner, CleanupModelInfo, CleanupRequest, CleanupSettings, Runtime, RuntimeCleaner, cpu};
use sayso_sdk::cleanup::download::DownloadError;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The name of the runtime program, without the CPU level and the extension.
const PROGRAM: &str = "sayso-cleanup";

fn info_value(info: &CleanupModelInfo) -> Value {
    json!({
        "id": info.id,
        "name": info.name,
        "publisher": info.publisher,
        "sizeBytes": info.size_bytes,
        "languages": info.languages,
        "tones": info.tones,
        "layouts": info.layouts.iter().map(|l| l.code()).collect::<Vec<_>>(),
        "formats": info.formats.iter().map(|f| f.code()).collect::<Vec<_>>(),
        "context": info.context,
    })
}

fn find(model: &str) -> Result<&'static CleanupModelInfo, ErrorReply> {
    cleanup::find(model).ok_or_else(|| error(Some(ErrorCode::UnknownModel), format!("there is no cleanup model {model}")))
}

fn download_error(e: DownloadError) -> ErrorReply {
    let code = match &e {
        DownloadError::Cancelled => Some(ErrorCode::Declined),
        DownloadError::Busy => Some(ErrorCode::Busy),
        _ => None,
    };
    error(code, e.to_string())
}

/// The cleanup models that the SDK can download and run, as JSON text.
#[napi]
pub fn cleanup_catalog() -> String {
    Value::Array(cleanup::catalog().iter().map(info_value).collect()).to_string()
}

/// True when the model is on disk and complete.
#[napi]
pub fn cleanup_is_ready(models_dir: String, model: String) -> String {
    outcome_text(find(&model).map(|info| json!(cleanup::download::is_ready(Path::new(&models_dir), info))))
}

/// The model file, when the model is ready, as an outcome (a path or null).
#[napi]
pub fn cleanup_model_path(models_dir: String, model: String) -> String {
    outcome_text(find(&model).map(|info| json!(cleanup::download::model_path(Path::new(&models_dir), info))))
}

/// Delete the files of the model.
#[napi(ts_return_type = "Promise<string>")]
pub fn cleanup_delete(env: &Env, models_dir: String, model: String) -> napi::Result<Object<'_>> {
    spawn_promise(env, "cleanup-delete", move || {
        cleanup::download::delete(Path::new(&models_dir), find(&model)?).map_err(download_error)?;
        Ok(Value::Null)
    })
}

/// The best build of the runtime in `folder` that this processor runs, or null.
#[napi]
pub fn cleanup_best_runtime(folder: String) -> Option<String> {
    cpu::best_in(Path::new(&folder), PROGRAM).map(|path| path.to_string_lossy().into_owned())
}

type ProgressFn = ThreadsafeFunction<String, (), String, Status, false, true>;

/// A download that the app can cancel. Its promise gives the path of the model file.
#[napi]
pub struct CleanupDownload {
    cancel: Arc<AtomicBool>,
}

#[napi]
impl CleanupDownload {
    /// Stop the download. Its promise fails with `declined`. The partial file stays for a later resume.
    #[napi]
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

/// Start a download of `model` to `models_dir`. `on_progress` gets the JSON text of
/// `{ bytesDone, bytesTotal }` about eight times each second.
#[napi(ts_return_type = "[CleanupDownload, Promise<string>]")]
pub fn cleanup_download<'env>(
    env: &'env Env,
    models_dir: String,
    model: String,
    endpoint: Option<String>,
    on_progress: Option<Function<'_, String, ()>>,
) -> napi::Result<(CleanupDownload, Object<'env>)> {
    let cancel = Arc::new(AtomicBool::new(false));
    let progress: Option<ProgressFn> = on_progress.map(|f| f.build_threadsafe_function::<String>().callee_handled::<false>().weak::<true>().build_callback(|ctx| Ok(ctx.value))).transpose()?;
    let flag = cancel.clone();
    let promise = spawn_promise(env, "cleanup-download", move || {
        let info = find(&model)?;
        let endpoint = endpoint.unwrap_or_else(|| cleanup::download::DEFAULT_ENDPOINT.to_string());
        let path = cleanup::download::download(Path::new(&models_dir), info, &endpoint, &flag, &mut |p| {
            if let Some(progress) = &progress {
                progress.call(json!({ "bytesDone": p.bytes_done, "bytesTotal": p.bytes_total }).to_string(), ThreadsafeFunctionCallMode::NonBlocking);
            }
        })
        .map_err(download_error)?;
        Ok(json!(path))
    })?;
    Ok((CleanupDownload { cancel }, promise))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JsClean {
    model: String,
    text: String,
    #[serde(default)]
    settings: CleanupSettings,
    app: Option<String>,
    site: Option<String>,
    timeout_ms: Option<u64>,
}

fn clean_error(e: CleanError) -> ErrorReply {
    let code = match &e {
        CleanError::Timeout(_) => Some(ErrorCode::Timeout),
        CleanError::NotConfigured(_) => Some(ErrorCode::Unavailable),
        CleanError::Runtime(_) | CleanError::InvalidOutput(_) => None,
    };
    error(code, e.to_string())
}

/// `RuntimeCleaner` with its runtime: the program `sayso-cleanup` and a folder of models.
#[napi]
pub struct NativeCleaner {
    runtime: Arc<Runtime>,
    cleaner: Arc<RuntimeCleaner>,
}

#[napi]
impl NativeCleaner {
    #[napi(constructor)]
    pub fn new(runtime: String, models_dir: String) -> NativeCleaner {
        let runtime = Runtime::new(PathBuf::from(runtime));
        let cleaner = Arc::new(RuntimeCleaner::new(runtime.clone(), PathBuf::from(models_dir)));
        NativeCleaner { runtime, cleaner }
    }

    /// Clean a text with a cleanup model. The request is JSON text; the promise gives the response.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn clean<'env>(&self, env: &'env Env, request_json: String) -> napi::Result<Object<'env>> {
        let js: JsClean = crate::parse_arg(&request_json, "the cleanup request")?;
        let cleaner = self.cleaner.clone();
        spawn_promise(env, "cleanup", move || {
            let settings = js.settings.known().ok_or_else(|| error(Some(ErrorCode::BadRequest), "a cleanup setting has a value that this SDK does not know"))?;
            let request = CleanupRequest { model: js.model, text: js.text, settings, app: js.app, site: js.site, timeout: Duration::from_millis(js.timeout_ms.unwrap_or(0)) };
            let response = cleaner.clean(&request).map_err(clean_error)?;
            Ok(json!({
                "text": response.text,
                "model": response.model,
                "elapsedMs": response.elapsed_ms,
                "loadMs": response.load_ms,
                "parts": response.parts,
                "tokensIn": response.tokens_in,
                "tokensOut": response.tokens_out,
            }))
        })
    }

    /// Load the model before the text is ready. A hint: it gives no error.
    #[napi]
    pub fn preload(&self, model: String) {
        self.cleaner.preload(&model);
    }

    /// True while the runtime process runs.
    #[napi]
    pub fn is_running(&self) -> bool {
        self.runtime.is_running()
    }

    /// Stop the runtime process, which frees the memory of the model. The next cleanup starts it again.
    #[napi]
    pub fn stop(&self) {
        self.runtime.stop();
    }
}

impl Drop for NativeCleaner {
    fn drop(&mut self) {
        self.runtime.stop();
    }
}
