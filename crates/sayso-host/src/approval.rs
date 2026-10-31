//! The approval question (`docs/sdk.md`, "The question"). `hello` of an
//! identity without a record, or of an approved app that asks for a new
//! capability, waits here on a worker of its connection. The reader of the
//! connection goes on, so a connection that closes takes back its question.
//!
//! The limits, so an app cannot fill the screen with questions:
//!
//! - One question is on the screen at a time, for all apps.
//! - The connections of one identity share one question. At most four of them
//!   wait, and a fifth gets `busy` at once.
//! - At most four identities wait, which includes the one on the screen. A
//!   fifth gets `busy` at once, so one app cannot block the others.
//! - After a question that its app took back, the identity gets `busy` with no
//!   question for 30 seconds: an app that connects and closes in a loop does
//!   not take the focus each time.

use crate::apps::{AppRecord, askable, unanswered};
use crate::identity::{Caller, Identity};
use crate::responder::Responder;
use crate::Shared;
use parking_lot::{Condvar, Mutex};
use sayso_protocol::{Capability, ErrorCode, Hello, HelloReply, PROTOCOL, Reply};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The most connections of one identity that wait for a question.
const MAX_CONNECTIONS: usize = 4;

/// The most identities that wait for a question.
const MAX_IDENTITIES: usize = 4;

/// How long an identity gets no new question after it took one back.
pub(crate) const COOLDOWN: Duration = Duration::from_secs(30);

/// How often a connection that waits looks whether it is still open.
const LOOK: Duration = Duration::from_millis(50);

/// The optional parts of the Host API that this host serves.
const FEATURES: [&str; 0] = [];

/// The question to the user about one identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalQuestion {
    /// The key of the app in the host.
    pub app_id: String,
    pub identity: Identity,
    /// The name from the system.
    pub name: String,
    /// The organization on the certificate of a signed app.
    pub signer: Option<String>,
    /// The name that the app gave, when it differs from `name`. It is text
    /// from the app, cleaned and cut to 40 characters.
    pub claimed_name: Option<String>,
    /// The executable is a known interpreter, so the answer covers each of its scripts.
    pub interpreter: bool,
    /// The capabilities to answer. For an approved app, only the new ones.
    pub capabilities: Vec<Capability>,
    /// True when the app is approved already, and this question is about new capabilities.
    pub approved: bool,
}

enum Done {
    /// The user answered, and the answer is in `apps.toml`.
    Answered,
    /// No answer that counts: the question was taken back, or the answer cannot be kept.
    NoAnswer,
}

struct Entry {
    number: u64,
    app_id: String,
    /// One flag for each connection that waits: false after it closed.
    waiters: Vec<Arc<AtomicBool>>,
    done: Option<Done>,
}

#[derive(Default)]
struct Queue {
    entries: VecDeque<Entry>,
    /// A question is on the screen.
    showing: bool,
    /// The identities that took a question back, and when they can ask again.
    cooling: HashMap<String, Instant>,
}

/// The connections that wait for an answer, in the order of their questions.
pub(crate) struct Approvals {
    queue: Mutex<Queue>,
    changed: Condvar,
    next: AtomicU64,
    cooldown: Duration,
}

enum Wait {
    Done,
    Busy,
    Left,
}

impl Approvals {
    pub fn new(cooldown: Duration) -> Approvals {
        Approvals { queue: Mutex::default(), changed: Condvar::new(), next: AtomicU64::new(0), cooldown }
    }

    /// Wait until the identity of `question` has an answer. One waiter shows
    /// the question, on its own thread, and writes the answer.
    fn wait(&self, shared: &Shared, caller: &Caller, question: &ApprovalQuestion, open: &Arc<AtomicBool>) -> Wait {
        let mut queue = self.queue.lock();
        let now = Instant::now();
        queue.cooling.retain(|_, until| *until > now);
        if queue.cooling.contains_key(&question.app_id) {
            return Wait::Busy;
        }
        let live = |entry: &Entry| entry.waiters.iter().filter(|flag| flag.load(Ordering::SeqCst)).count();
        let identities = queue.entries.iter().filter(|entry| entry.done.is_none() && live(entry) > 0).count();
        let number = match queue.entries.iter_mut().find(|entry| entry.done.is_none() && entry.app_id == question.app_id) {
            Some(entry) if live(entry) >= MAX_CONNECTIONS => return Wait::Busy,
            Some(entry) => {
                entry.waiters.push(open.clone());
                entry.number
            }
            None if identities >= MAX_IDENTITIES => return Wait::Busy,
            None => {
                let number = self.next.fetch_add(1, Ordering::SeqCst);
                queue.entries.push_back(Entry { number, app_id: question.app_id.clone(), waiters: vec![open.clone()], done: None });
                number
            }
        };
        loop {
            let Some(at) = queue.entries.iter().position(|entry| entry.number == number) else { return Wait::Left };
            if let Some(done) = &queue.entries[at].done {
                let result = match done {
                    Done::Answered => Wait::Done,
                    Done::NoAnswer => Wait::Left,
                };
                leave(&mut queue, number, open);
                self.changed.notify_all();
                return result;
            }
            if !open.load(Ordering::SeqCst) || shared.is_closing() {
                leave(&mut queue, number, open);
                self.changed.notify_all();
                return Wait::Left;
            }
            let next_up = queue.entries.iter().find(|entry| entry.done.is_none()).is_some_and(|entry| entry.number == number);
            if !queue.showing && next_up {
                queue.showing = true;
                drop(queue);
                let done = self.show(shared, caller, question, number);
                queue = self.queue.lock();
                queue.showing = false;
                if let Some(entry) = queue.entries.iter_mut().find(|entry| entry.number == number) {
                    entry.done = Some(done);
                }
                self.changed.notify_all();
                continue;
            }
            self.changed.wait_for(&mut queue, LOOK);
        }
    }

    fn show(&self, shared: &Shared, caller: &Caller, question: &ApprovalQuestion, number: u64) -> Done {
        log::info!("host: asking the user about {} for {:?}", question.name, question.capabilities);
        // The question goes away when each connection that waits for it has closed.
        let withdrawn = || {
            let queue = self.queue.lock();
            shared.is_closing() || queue.entries.iter().find(|entry| entry.number == number).is_none_or(|entry| entry.waiters.iter().all(|flag| !flag.load(Ordering::SeqCst)))
        };
        let Some(allow) = shared.services.ask_approval(question, &withdrawn) else {
            if !shared.is_closing() && withdrawn() {
                self.queue.lock().cooling.insert(question.app_id.clone(), Instant::now() + self.cooldown);
            }
            return Done::NoAnswer;
        };
        log::info!("host: the user said {} to {}", if allow { "allow" } else { "don't allow" }, question.name);
        let written = if question.approved { shared.apps.answer_new(&question.app_id, &question.capabilities, allow) } else { shared.apps.answer_first(caller, &question.capabilities, allow) };
        match written {
            Ok(()) => Done::Answered,
            Err(e) => {
                log::warn!("host: cannot keep the answer for {}: {e}", question.name);
                Done::NoAnswer
            }
        }
    }
}

fn leave(queue: &mut Queue, number: u64, open: &Arc<AtomicBool>) {
    let Some(at) = queue.entries.iter().position(|entry| entry.number == number) else { return };
    let entry = &mut queue.entries[at];
    entry.waiters.retain(|flag| !Arc::ptr_eq(flag, open));
    if entry.waiters.is_empty() {
        queue.entries.remove(at);
    }
}

/// What `hello` of a connection gets now.
pub(crate) enum HelloStep {
    /// Reply at once, with the grants of the record.
    Reply(HelloReply),
    /// No identity, a refused app, or a broken `apps.toml`: `not_approved`.
    Refuse,
    /// The user has to answer first.
    Ask(ApprovalQuestion),
}

/// What `hello` gets from the record that `apps.toml` has now.
pub(crate) fn hello_step(shared: &Shared, caller: &Caller, request: &Hello) -> HelloStep {
    if shared.apps.is_broken() {
        return HelloStep::Refuse;
    }
    let apps = shared.apps.current();
    match apps.find(&caller.identity) {
        Some(record) if !record.allowed => HelloStep::Refuse,
        Some(record) => {
            let new = unanswered(record, &request.capabilities);
            if new.is_empty() { HelloStep::Reply(reply(shared, record, &request.capabilities)) } else { HelloStep::Ask(question(caller, request, new, true)) }
        }
        None => HelloStep::Ask(question(caller, request, askable(&request.capabilities), false)),
    }
}

fn question(caller: &Caller, request: &Hello, capabilities: Vec<Capability>, approved: bool) -> ApprovalQuestion {
    let claimed_name = request.app.as_ref().and_then(|app| crate::identity::clean_name(&app.name)).filter(|claimed| !claimed.eq_ignore_ascii_case(&caller.name));
    ApprovalQuestion {
        app_id: caller.identity.id(),
        identity: caller.identity.clone(),
        name: caller.name.clone(),
        signer: caller.signer.clone(),
        claimed_name,
        interpreter: caller.interpreter,
        capabilities,
        approved,
    }
}

fn reply(shared: &Shared, record: &AppRecord, asked: &[Capability]) -> HelloReply {
    let mut denied = record.denied_capabilities();
    // A capability that this version of Sayso does not know: the app does not have it.
    denied.extend(asked.iter().filter(|capability| matches!(capability, Capability::Other(_))).cloned());
    HelloReply { host_version: shared.host_version.clone(), protocol: PROTOCOL, features: FEATURES.iter().map(|feature| feature.to_string()).collect(), capabilities: record.capabilities(), denied }
}

pub(crate) fn not_approved() -> Reply {
    Reply::error(Some(ErrorCode::NotApproved), "the user did not approve this app in Sayso")
}

/// The rest of `hello` after [`hello_step`] said [`HelloStep::Ask`], on a worker: ask, and
/// reply when the user answered. Close the connection after `not_approved`.
pub(crate) fn ask_and_reply(shared: &Shared, caller: &Caller, request: &Hello, mut question: ApprovalQuestion, open: &Arc<AtomicBool>, asking: &AtomicBool, responder: Responder) {
    loop {
        let waited = shared.approvals.wait(shared, caller, &question, open);
        let next = match waited {
            Wait::Busy => {
                let approved = question.approved;
                let notifier = responder.notifier();
                asking.store(false, Ordering::SeqCst);
                responder.reply(Reply::error(Some(ErrorCode::Busy), "Sayso cannot ask about this app now; try again later"));
                // An approved app keeps its connection and its old grants.
                if !approved {
                    notifier.close();
                }
                return;
            }
            Wait::Done => hello_step(shared, caller, request),
            // No answer that counts: what the record says now, without a new question.
            Wait::Left => match hello_step(shared, caller, request) {
                HelloStep::Ask(asked) if asked.approved => match shared.apps.current().find(&caller.identity) {
                    Some(record) => HelloStep::Reply(reply(shared, record, &request.capabilities)),
                    None => HelloStep::Refuse,
                },
                HelloStep::Ask(_) => HelloStep::Refuse,
                other => other,
            },
        };
        match next {
            HelloStep::Reply(hello) => {
                // Before the reply: the next request of the app can come at once.
                asking.store(false, Ordering::SeqCst);
                responder.reply(Reply::ok_with(&hello));
                return;
            }
            HelloStep::Refuse => {
                let notifier = responder.notifier();
                asking.store(false, Ordering::SeqCst);
                responder.reply(not_approved());
                notifier.close();
                return;
            }
            // Another connection of this identity asked for less, so this one has more to ask.
            HelloStep::Ask(asked) => question = asked,
        }
    }
}
