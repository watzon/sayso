//! `download_model` (`docs/extensions.md`, "Requests"): the host asks the
//! user, starts the download in the engine, and follows it.
//!
//! The rules:
//! - A model that is on disk gets `ok` at once, with no question.
//! - A model that downloads now (started by the user, or by another request)
//!   is followed, with no second download and no question.
//! - An extension has one question at a time. A second `download_model` of
//!   the same extension waits for the answer to the first, and looks at the
//!   state again after that, so it can follow the download that the first
//!   one started.
//! - Events (`model_state`, `download_progress`) have the id of the request.
//!   The reply comes after the last of them.

use crate::models::{no_engine, state};
use crate::responder::Responder;
use crate::{DownloadQuestion, Shared};
use sayso_core::models::{self as core, ModelId};
use sayso_core::stt::{EngineEvent, ModelStatus};
use sayso_platform::{EngineStart, SttBackend};
use sayso_protocol::{DownloadModel, DownloadProgress, ErrorCode, ErrorReply, Event, ModelState, ModelStateEvent, Reply};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the engine may take to answer at its start.
const ENGINE_START_WAIT: Duration = Duration::from_secs(10);
/// How long the engine may take to report a download that the host started.
const DOWNLOAD_START_WAIT: Duration = Duration::from_secs(30);
/// How often a wait looks at the connection and at the host.
const TICK: Duration = Duration::from_millis(250);
/// The reason of the app is text from another program. The question to
/// the user shows only the start of it.
const MAX_REASON_CHARS: usize = 300;

/// The engine, when it runs and has answered. Waits a little at the start of
/// Sayso, because until then the statuses of the models are not known. The
/// error is the reply to send.
pub(crate) fn ready_engine(shared: &Shared, responder: &Responder) -> Result<Arc<dyn SttBackend>, Reply> {
    let Some(engine) = &shared.engine else { return Err(no_engine()) };
    let deadline = Instant::now() + ENGINE_START_WAIT;
    loop {
        match engine.start_state() {
            EngineStart::Started => return Ok(engine.clone()),
            EngineStart::Failed => return Err(Reply::error(Some(ErrorCode::Unsupported), "the speech engine did not start")),
            EngineStart::Starting if Instant::now() >= deadline || !responder.is_open() || shared.is_closing() => {
                return Err(Reply::error(Some(ErrorCode::Unsupported), "the speech engine is still starting. Try again in a moment"));
            }
            EngineStart::Starting => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// `download_model`. Runs on a worker thread.
pub(crate) fn download_model(shared: &Shared, app_id: &str, request: &DownloadModel, responder: Responder) {
    let reply = run(shared, app_id, request, &responder);
    responder.reply(reply);
}

fn run(shared: &Shared, app_id: &str, request: &DownloadModel, responder: &Responder) -> Reply {
    let id = ModelId::new(&request.model);
    let Some(info) = core::find_local(&id) else {
        return Reply::error(Some(ErrorCode::UnknownModel), format!("there is no model {} for this computer", request.model));
    };
    let engine = match ready_engine(shared, responder) {
        Ok(engine) => engine,
        Err(reply) => return reply,
    };
    // Watch before the first look at the status, so no change is lost.
    let events = shared.bus.watch();

    match look(shared, &engine, &id) {
        Look::OnDisk => return Reply::ok(),
        Look::Running => return follow(shared, &engine, &events, &id, responder),
        Look::Idle => {}
    }
    let started = {
        let gate = shared.question_gate(app_id);
        let _one_question = gate.lock();
        // The request before this one may have started the download.
        match look(shared, &engine, &id) {
            Look::OnDisk => Err(Reply::ok()),
            Look::Running => Ok(StartGuard { shared, id: None }),
            Look::Idle => ask_and_start(shared, app_id, request, &info, &engine, responder),
        }
    };
    // The question is closed, so the next one of this extension can open.
    let guard = match started {
        Ok(guard) => guard,
        Err(reply) => return reply,
    };
    let reply = follow(shared, &engine, &events, &id, responder);
    drop(guard);
    reply
}

/// Ask the user, and start the download when the answer is yes. The error is
/// the reply to send: declined, a failed start, or `ok` when the model is on
/// disk after all.
fn ask_and_start<'a>(
    shared: &'a Shared,
    app_id: &str,
    request: &DownloadModel,
    info: &core::ModelInfo,
    engine: &Arc<dyn SttBackend>,
    responder: &Responder,
) -> Result<StartGuard<'a>, Reply> {
    if !responder.is_open() {
        return Err(Reply::error(None, "the connection is closed"));
    }
    let app_name = shared.apps.current().record(app_id).map_or_else(|| app_id.to_string(), |record| record.name.clone());
    let question = DownloadQuestion {
        app_id: app_id.to_string(),
        app_name,
        model_id: info.id.to_string(),
        model_name: info.name.clone(),
        size_bytes: info.size_bytes,
        reason: short(&request.reason),
    };
    log::info!("host: {app_id} asks to download {}", info.id);
    // The question closes when the app disconnects or Sayso closes.
    let withdrawn = || !responder.is_open() || shared.is_closing();
    let accepted = shared.services.ask_download(&question, &withdrawn);
    log::info!("host: the user said {} to {app_id} for {}", if accepted { "yes" } else { "no" }, info.id);
    if !accepted {
        return Err(Reply::error(Some(ErrorCode::Declined), "the user said no to the download"));
    }
    // The question can stay open for a long time. The extension may be gone.
    if !responder.is_open() {
        return Err(Reply::error(None, "the connection is closed"));
    }
    // Start under the lock of the downloads that the host started, so two
    // answers of "Download" for one model (from two extensions) start one
    // download.
    let mut starting = shared.downloads.lock();
    match look_with(engine, &info.id, &starting) {
        Look::OnDisk => return Err(Reply::ok()),
        // Another request started it while this one asked. The caller follows
        // the model with the events, as for a running download.
        Look::Running => return Ok(StartGuard { shared, id: None }),
        Look::Idle => {}
    }
    if let Err(e) = engine.download(&info.id) {
        return Err(Reply::error(None, format!("the engine cannot start the download: {e}")));
    }
    log::info!("host: download of {} starts for {app_id}", info.id);
    starting.insert(info.id.clone());
    Ok(StartGuard { shared, id: Some(info.id.clone()) })
}

/// Removes a model from the downloads that the host started, when the request
/// that started it ends.
struct StartGuard<'a> {
    shared: &'a Shared,
    /// None when another request started the download.
    id: Option<ModelId>,
}

impl Drop for StartGuard<'_> {
    fn drop(&mut self) {
        if let Some(id) = &self.id {
            self.shared.downloads.lock().remove(id);
        }
    }
}

enum Look {
    OnDisk,
    Running,
    Idle,
}

fn look(shared: &Shared, engine: &Arc<dyn SttBackend>, id: &ModelId) -> Look {
    look_with(engine, id, &shared.downloads.lock())
}

/// A model is running when the engine says so, or when the host just asked
/// the engine to download it and the engine did not report it yet.
fn look_with(engine: &Arc<dyn SttBackend>, id: &ModelId, starting: &HashSet<ModelId>) -> Look {
    let status = engine.status(id);
    if status.is_on_disk() {
        Look::OnDisk
    } else if matches!(status, ModelStatus::Downloading { .. }) || starting.contains(id) {
        Look::Running
    } else {
        Look::Idle
    }
}

fn short(reason: &str) -> String {
    let reason = reason.trim();
    if reason.chars().count() <= MAX_REASON_CHARS {
        return reason.to_string();
    }
    let mut cut: String = reason.chars().take(MAX_REASON_CHARS).collect();
    cut.push_str("...");
    cut
}

/// Send the events of the download to the app until it ends, and give
/// the reply.
///
/// The old status counts only when it is `downloading`. A model that the host
/// just asked for can still have the status of an earlier try (`failed`), and
/// only the new events say what happens now.
fn follow(shared: &Shared, engine: &Arc<dyn SttBackend>, events: &Receiver<EngineEvent>, id: &ModelId, responder: &Responder) -> Reply {
    let model = id.to_string();
    let mut progress = Progress { model: &model, responder, announced: None };
    let mut seen_downloading = false;
    let current = engine.status(id);
    if matches!(current, ModelStatus::Downloading { .. }) {
        seen_downloading = true;
        if let Some(reply) = progress.status(&current, seen_downloading) {
            return reply;
        }
    }
    let give_up_at = Instant::now() + DOWNLOAD_START_WAIT;
    loop {
        if !responder.is_open() || shared.is_closing() {
            // The extension is gone, or Sayso quits. The engine goes on.
            return Reply::error(None, "the connection is closed");
        }
        match events.recv_timeout(TICK) {
            Ok(EngineEvent::ModelStatus { model, status }) if model == *id => {
                if let Some(reply) = progress.status(&status, seen_downloading) {
                    return reply;
                }
                seen_downloading |= matches!(status, ModelStatus::Downloading { .. });
            }
            Ok(EngineEvent::Crashed { .. } | EngineEvent::StartFailed { .. }) => {
                progress.announce(ModelState::NotDownloaded);
                return Reply::error(None, "the speech engine stopped during the download");
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {
                if !seen_downloading && Instant::now() >= give_up_at {
                    return Reply::error(None, "the engine did not report the download");
                }
            }
            Err(RecvTimeoutError::Disconnected) => return Reply::error(None, "Sayso is closing"),
        }
    }
}

/// What the app heard so far about one download.
struct Progress<'a> {
    model: &'a str,
    responder: &'a Responder,
    /// The last `model_state` that the app got.
    announced: Option<ModelState>,
}

impl Progress<'_> {
    fn announce(&mut self, state: ModelState) {
        self.state_event(state, None, None);
    }

    fn state_event(&mut self, state: ModelState, message: Option<String>, reason: Option<String>) {
        if self.announced.as_ref() == Some(&state) && message.is_none() {
            return;
        }
        self.responder.event(&Event::ModelState(ModelStateEvent { model: self.model.to_string(), state: state.clone(), load_ms: None, preview: None, message, reason }));
        self.announced = Some(state);
    }

    /// Send what a status means. The reply, when the download ended.
    fn status(&mut self, status: &ModelStatus, seen_downloading: bool) -> Option<Reply> {
        match status {
            ModelStatus::Downloading { fraction, bytes_done, bytes_total } => {
                self.announce(ModelState::Downloading);
                self.responder.event(&Event::DownloadProgress(DownloadProgress {
                    model: self.model.to_string(),
                    fraction: f64::from(*fraction),
                    bytes_done: *bytes_done,
                    bytes_total: *bytes_total,
                }));
                None
            }
            ModelStatus::Downloaded | ModelStatus::Optimizing | ModelStatus::Ready => {
                self.announce(state(status));
                Some(Reply::ok())
            }
            ModelStatus::Failed { message, reason } => {
                let reason = reason.map(|reason| reason.code().to_string());
                self.state_event(ModelState::Failed, Some(message.clone()), reason.clone());
                Some(Reply::Error(ErrorReply { code: None, message: format!("the download failed: {message}"), capability: None, reason }))
            }
            // The engine restarted, or the download stopped.
            ModelStatus::NotDownloaded if seen_downloading => {
                self.announce(ModelState::NotDownloaded);
                Some(Reply::error(None, "the download stopped"))
            }
            ModelStatus::NotDownloaded => None,
        }
    }
}
