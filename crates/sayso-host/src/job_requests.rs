//! `transcribe_file` and `cancel_job` (`docs/apps.md`, "Requests").
//!
//! Both answer on the reader thread: the checks read the settings and the
//! state of the engine, and do not wait. The job then runs on the thread of
//! the queue ([`crate::jobs`]). Its events have `job` and no `id`, because
//! they go on after the `ok {job}` reply (see "Code" in the docs).

use crate::Shared;
use crate::jobs::SaysoModels;
use crate::responder::{Notifier, Responder};
use crate::sandbox::OpenedFile;
use parking_lot::Mutex;
use sayso_protocol::{CancelJob, ErrorCode, JobReply, Reply, TranscribeFile};
use sayso_protocol::Event;
use sayso_speech::{EventSink, ModelResolver};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The jobs with an open file that one connection can have. The queue has no
/// limit, and each of these jobs holds a file handle of Sayso while it waits.
const MAX_JOBS_WITH_FILE: usize = 16;

/// The events of a job, and the file that the path of the job opens again.
/// The queue keeps the sink until the job ends, so the file stays open as long.
struct SinkWithFile {
    notifier: Notifier,
    _file: OpenedFile,
    jobs_with_file: Arc<AtomicUsize>,
}

impl Drop for SinkWithFile {
    fn drop(&mut self) {
        self.jobs_with_file.fetch_sub(1, Ordering::SeqCst);
    }
}

/// One more job with a file, or false when the connection has the most it can.
fn take_job_with_file(count: &AtomicUsize) -> bool {
    count.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |jobs| (jobs < MAX_JOBS_WITH_FILE).then_some(jobs + 1)).is_ok()
}

impl EventSink for SinkWithFile {
    fn event(&self, event: Event) {
        self.notifier.event(event);
    }
}

/// Check the request and queue the job. The reply is `ok {job}` or `error {code}`.
/// `owned` gets the id of the job, for `cancel_job`. `file` is the file of the
/// path of the request, when it is a path that only this process can open.
pub(crate) fn transcribe_file(shared: &Shared, connection: u64, owned: &Mutex<HashSet<u64>>, jobs_with_file: &Arc<AtomicUsize>, request: &TranscribeFile, file: Option<OpenedFile>, responder: Responder) {
    let (Some(engine), Some(queue)) = (&shared.engine, &shared.jobs) else {
        responder.reply(Reply::error(Some(ErrorCode::Unsupported), "the speech engine of Sayso does not run"));
        return;
    };
    let config = shared.services.config();
    let job = match SaysoModels::with(&config, &**engine, |models| models.job(request)) {
        Ok(job) => job,
        Err(refusal) => {
            responder.reply(Reply::error(Some(refusal.code), refusal.message));
            return;
        }
    };
    let sink: Arc<dyn EventSink> = match file {
        Some(file) => {
            if !take_job_with_file(jobs_with_file) {
                responder.reply(Reply::error(Some(ErrorCode::Busy), "this connection has too many files waiting"));
                return;
            }
            Arc::new(SinkWithFile { notifier: responder.notifier(), _file: file, jobs_with_file: jobs_with_file.clone() })
        }
        None => Arc::new(responder.notifier()),
    };
    // The reply goes out before the first event of the job. After a close of
    // the host, the dropped responder replies with an error.
    if let Some(id) = queue.submit(connection, job, sink, move |job| responder.reply(Reply::ok_with(&JobReply { job }))) {
        owned.lock().insert(id);
        // The user can have taken the grant back during the checks.
        shared.stop_work_without_grant();
    }
}

/// Cancel a job of this connection. A job of this connection that already
/// ended gets `ok` too, so a cancel that crosses the end of the job is not an
/// error. A job that this connection did not start gets `bad_request`.
pub(crate) fn cancel_job(shared: &Shared, connection: u64, owned: &Mutex<HashSet<u64>>, request: &CancelJob, responder: Responder) {
    if !owned.lock().contains(&request.job) {
        responder.reply(Reply::error(Some(ErrorCode::BadRequest), format!("this connection has no job {}", request.job)));
        return;
    }
    if let Some(queue) = &shared.jobs {
        queue.cancel(connection, request.job);
    }
    responder.reply(Reply::ok());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_has_a_limit_of_jobs_with_a_file() {
        let count = AtomicUsize::new(0);
        for _ in 0..MAX_JOBS_WITH_FILE {
            assert!(take_job_with_file(&count));
        }
        assert!(!take_job_with_file(&count));
        count.fetch_sub(1, Ordering::SeqCst);
        assert!(take_job_with_file(&count));
    }
}
