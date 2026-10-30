//! `session_start`, `session_audio`, and `session_stop` (`docs/apps.md`,
//! "Requests"). The sessions themselves are in [`crate::live`].
//!
//! `session_start` and `session_stop` take time (a load, the last parts,
//! diarization), so they run on a worker thread. `session_audio` is quick and
//! runs on the reader thread. The `partial`, `segment`, and `speakers` events
//! of a session have no `id`, like the events of a job: they go on after the
//! reply of `session_start`.

use crate::Shared;
use crate::jobs::{EventSink, SaysoModels};
use crate::live::Owner;
use crate::responder::{Notifier, Responder};
use parking_lot::Mutex;
use sayso_protocol::{ErrorCode, Event, Reply, SessionAudio, SessionReply, SessionStart, SessionStop, TextReply};
use sayso_speech::ModelResolver;
use std::sync::Arc;

fn no_engine() -> Reply {
    Reply::error(Some(ErrorCode::Unsupported), "the speech engine of Sayso does not run")
}

/// Check the models, start the session, and reply `ok {session}`. Runs on a worker.
pub(crate) fn session_start(shared: &Shared, connection: u64, app_id: &str, request: &SessionStart, responder: Responder) {
    let (Some(engine), Some(live)) = (&shared.engine, &shared.live) else {
        responder.reply(no_engine());
        return;
    };
    let config = shared.services.config();
    let spec = match SaysoModels::with(&config, &**engine, |models| models.session(request)) {
        Ok(spec) => spec,
        Err(refusal) => {
            responder.reply(Reply::error(Some(refusal.code), refusal.message));
            return;
        }
    };
    // The events of the session wait until the app has its id.
    let sink = Arc::new(AfterReply::new(responder.notifier()));
    match live.start(Owner { connection, app_id: app_id.to_string() }, spec, sink.clone()) {
        Ok(session) => {
            responder.reply(Reply::ok_with(&SessionReply { session }));
            sink.release();
        }
        Err(e) => responder.reply(e.reply()),
    }
}

/// Audio for the source `client`. Without an `id` there is no reply, and an
/// error goes out without an `id`. With an `id`, the reply is `ok` or `error`.
pub(crate) fn session_audio(shared: &Shared, connection: u64, request: &SessionAudio, responder: Responder) {
    let Some(live) = &shared.live else {
        responder.reply(no_engine());
        return;
    };
    match live.audio(connection, request.session, &request.pcm) {
        Ok(()) if responder.has_id() => responder.reply(Reply::ok()),
        Ok(()) => responder.no_reply(),
        Err(e) => responder.reply(e.reply()),
    }
}

/// Stop the session and reply `ok {text}` after its last events. Runs on a worker.
pub(crate) fn session_stop(shared: &Shared, connection: u64, request: &SessionStop, responder: Responder) {
    let Some(live) = &shared.live else {
        responder.reply(no_engine());
        return;
    };
    match live.stop(connection, request.session) {
        Ok(text) => responder.reply(Reply::ok_with(&TextReply { text })),
        Err(e) => responder.reply(e.reply()),
    }
}

/// Sends the events of a session to its connection, without an `id`. Until
/// [`AfterReply::release`], it keeps them, so none comes before `ok {session}`.
struct AfterReply {
    notifier: Notifier,
    held: Mutex<Option<Vec<Event>>>,
}

impl AfterReply {
    fn new(notifier: Notifier) -> AfterReply {
        AfterReply { notifier, held: Mutex::new(Some(Vec::new())) }
    }

    fn release(&self) {
        let mut held = self.held.lock();
        for event in held.take().unwrap_or_default() {
            self.notifier.send(&event);
        }
    }
}

impl EventSink for AfterReply {
    fn event(&self, event: Event) {
        let mut held = self.held.lock();
        match held.as_mut() {
            Some(waiting) => waiting.push(event),
            None => self.notifier.send(&event),
        }
    }
}
