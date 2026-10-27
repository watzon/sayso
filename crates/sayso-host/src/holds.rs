//! The work of extensions that holds Sayso open (`docs/extensions.md`,
//! "Lifecycle"): a live session, or a job that runs or waits. Sayso asks
//! before it quits, and does not install an update, while an extension has
//! such work.

use crate::Shared;
use crate::live::{LiveInfo, LiveObserver};
use parking_lot::Mutex;
use std::sync::Arc;

/// One extension with work that holds Sayso open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hold {
    pub extension_id: String,
    /// The name to show: the name in `state.toml`, else the id.
    pub name: String,
    pub work: Work,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Work {
    /// A live session: the extension records.
    LiveSession,
    /// A job that runs or waits in the queue.
    Job,
}

/// The holds now: the live session first, then the jobs in the order of
/// their connections, one entry for each extension and kind of work.
pub(crate) fn holds(shared: &Shared) -> Vec<Hold> {
    let mut found: Vec<(String, Work)> = Vec::new();
    if let Some(live) = shared.live.as_ref().and_then(crate::live::LiveSessions::current) {
        found.push((live.extension_id, Work::LiveSession));
    }
    if let Some(jobs) = &shared.jobs {
        let owners = jobs.connections();
        let connections = shared.connections.lock();
        for connection in owners {
            if let Some(id) = connections.get(&connection).and_then(|entry| entry.extension_id.clone()) {
                found.push((id, Work::Job));
            }
        }
    }
    let state = shared.state.current();
    let mut holds: Vec<Hold> = Vec::new();
    for (extension_id, work) in found {
        if holds.iter().any(|hold| hold.extension_id == extension_id && hold.work == work) {
            continue;
        }
        let name = state.record(&extension_id).map_or_else(|| extension_id.clone(), |record| record.display_name().to_string());
        holds.push(Hold { extension_id, name, work });
    }
    holds
}

type HoldsObserver = Arc<dyn Fn() + Send + Sync>;

/// The observers of the app. The live sessions and the queue of jobs tell
/// them about each change.
#[derive(Clone, Default)]
pub(crate) struct Observers {
    live: Arc<Mutex<Option<Arc<dyn LiveObserver>>>>,
    holds: Arc<Mutex<Option<HoldsObserver>>>,
}

impl Observers {
    pub fn set_live(&self, observer: Arc<dyn LiveObserver>) {
        *self.live.lock() = Some(observer);
    }

    pub fn set_holds(&self, observer: HoldsObserver) {
        *self.holds.lock() = Some(observer);
    }

    pub fn holds_changed(&self) {
        let observer = self.holds.lock().clone();
        if let Some(observer) = observer {
            observer();
        }
    }
}

impl LiveObserver for Observers {
    fn changed(&self, live: Option<&LiveInfo>) {
        let observer = self.live.lock().clone();
        if let Some(observer) = observer {
            observer.changed(live);
        }
        self.holds_changed();
    }
}
