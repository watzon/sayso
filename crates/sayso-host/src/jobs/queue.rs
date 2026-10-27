//! The queue of jobs (`docs/extensions.md`, "Scheduling"): one job runs at a
//! time, first in, first out, on a thread of the queue.
//!
//! A job that waits sends `job_progress` with `fraction: 0` when it enters
//! the queue, and again each time the queue moves (a job ahead of it starts),
//! so the extension sees that it is alive and waits.
//!
//! A job ends with one event: `job_done` or `job_failed`. A cancelled job
//! gets `job_failed` with the message [`CANCELLED`]: at once when it waits,
//! and after the engine call that runs when it runs.

use super::gate::DictationGate;
use super::plan::Stopped;
use super::{EventSink, JobSpec};
use parking_lot::{Condvar, Mutex};
use sayso_protocol::Event;
use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

/// The message of `job_failed` for a cancelled job.
pub const CANCELLED: &str = "The job was cancelled.";

/// Runs one job and sends its events, the last one `job_done` or `job_failed`.
pub trait RunJob: Send + Sync {
    fn run(&self, job: &JobSpec, run: &JobRun<'_>);
}

/// A job while it runs: its id, its events, and its cancel.
pub struct JobRun<'a> {
    pub id: u64,
    sink: &'a dyn EventSink,
    cancel: &'a AtomicBool,
    gate: &'a DictationGate,
}

impl JobRun<'_> {
    pub fn event(&self, event: Event) {
        self.sink.event(event);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Call before each call to the engine: waits while a dictation runs, and
    /// stops a cancelled job.
    pub fn before_engine_call(&self) -> Result<(), Stopped> {
        if self.gate.wait_idle(&|| self.cancelled()) { Ok(()) } else { Err(Stopped::Cancelled) }
    }

    /// The end event of a job that `stopped`.
    pub fn stopped(&self, stopped: Stopped) {
        let message = match stopped {
            Stopped::Cancelled => CANCELLED.to_string(),
            Stopped::Failed(message) => message,
        };
        self.event(Event::JobFailed { job: self.id, message, code: None });
    }
}

/// The queue. [`JobQueue::close`] (or drop) cancels the jobs and stops its thread.
pub struct JobQueue {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    gate: Arc<DictationGate>,
    runner: Arc<dyn RunJob>,
}

#[derive(Default)]
struct State {
    waiting: VecDeque<Queued>,
    running: Option<Running>,
    next_id: u64,
    closed: bool,
}

struct Queued {
    id: u64,
    connection: u64,
    job: JobSpec,
    sink: Arc<dyn EventSink>,
}

struct Running {
    id: u64,
    connection: u64,
    cancel: Arc<AtomicBool>,
}

impl JobQueue {
    /// Start the thread of the queue. `runner` runs each job, and `gate` is
    /// the dictation gate of the app.
    pub fn start(runner: Arc<dyn RunJob>, gate: Arc<DictationGate>) -> io::Result<JobQueue> {
        let shared = Arc::new(Shared { state: Mutex::new(State::default()), wake: Condvar::new(), gate, runner });
        let worker = {
            let shared = shared.clone();
            std::thread::Builder::new().name("sayso-host-jobs".into()).spawn(move || work(&shared))?
        };
        Ok(JobQueue { shared, worker: Mutex::new(Some(worker)) })
    }

    /// Add a job of `connection` at the end of the queue. `accepted` gets the
    /// id of the job before any event of the job, so the wiring sends the
    /// `ok {job}` reply there. The events go to `sink`, which must not block.
    /// None, without a call to `accepted`, after [`JobQueue::close`].
    pub fn submit(&self, connection: u64, job: JobSpec, sink: Arc<dyn EventSink>, accepted: impl FnOnce(u64)) -> Option<u64> {
        let mut state = self.shared.state.lock();
        if state.closed {
            return None;
        }
        state.next_id += 1;
        let id = state.next_id;
        accepted(id);
        sink.event(Event::JobProgress { job: id, fraction: 0.0 });
        state.waiting.push_back(Queued { id, connection, job, sink });
        self.shared.wake.notify_all();
        Some(id)
    }

    /// Cancel the job `job` of `connection`. A waiting job ends at once; a
    /// running job stops after its engine call. False when `connection` has
    /// no such job (it ended, it is not its job, or it is already cancelled).
    pub fn cancel(&self, connection: u64, job: u64) -> bool {
        self.cancel_where(|id, c| c == connection && id == job) > 0
    }

    /// Cancel every job of `connection`, for example when it closes.
    pub fn cancel_connection(&self, connection: u64) {
        self.cancel_where(|_, c| c == connection);
    }

    /// True while `connection` has a job that waits or runs.
    pub fn has_jobs(&self, connection: u64) -> bool {
        let state = self.shared.state.lock();
        state.running.as_ref().is_some_and(|r| r.connection == connection) || state.waiting.iter().any(|q| q.connection == connection)
    }

    /// Cancel every job and stop the thread. A second call does nothing.
    pub fn close(&self) {
        {
            let mut state = self.shared.state.lock();
            state.closed = true;
        }
        self.cancel_where(|_, _| true);
        self.shared.wake.notify_all();
        if let Some(worker) = self.worker.lock().take() {
            let _ = worker.join();
        }
    }

    /// Cancel the jobs for which `matches(job, connection)` is true. Gives how
    /// many were cancelled now.
    fn cancel_where(&self, matches: impl Fn(u64, u64) -> bool) -> usize {
        let (removed, running) = {
            let mut state = self.shared.state.lock();
            let (removed, kept): (Vec<Queued>, Vec<Queued>) =
                std::mem::take(&mut state.waiting).into_iter().partition(|q| matches(q.id, q.connection));
            state.waiting = kept.into();
            let running = state.running.as_ref().is_some_and(|r| matches(r.id, r.connection) && !r.cancel.swap(true, Ordering::SeqCst));
            (removed, running)
        };
        if running {
            // A job that waits for a dictation stops at once.
            self.shared.gate.wake();
        }
        for queued in &removed {
            queued.sink.event(Event::JobFailed { job: queued.id, message: CANCELLED.into(), code: None });
        }
        removed.len() + usize::from(running)
    }
}

impl Drop for JobQueue {
    fn drop(&mut self) {
        self.close();
    }
}

fn work(shared: &Shared) {
    loop {
        let (queued, cancel, moved) = {
            let mut state = shared.state.lock();
            let queued = loop {
                if state.closed {
                    return;
                }
                if let Some(queued) = state.waiting.pop_front() {
                    break queued;
                }
                shared.wake.wait(&mut state);
            };
            let cancel = Arc::new(AtomicBool::new(false));
            state.running = Some(Running { id: queued.id, connection: queued.connection, cancel: cancel.clone() });
            let moved: Vec<(u64, Arc<dyn EventSink>)> = state.waiting.iter().map(|q| (q.id, q.sink.clone())).collect();
            (queued, cancel, moved)
        };
        for (id, sink) in moved {
            sink.event(Event::JobProgress { job: id, fraction: 0.0 });
        }
        let run = JobRun { id: queued.id, sink: &*queued.sink, cancel: &cancel, gate: &shared.gate };
        shared.runner.run(&queued.job, &run);
        shared.state.lock().running = None;
    }
}
