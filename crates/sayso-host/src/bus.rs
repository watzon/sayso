//! One subscription to the events of the engine, shared by all the work of
//! the host that needs them (downloads now, jobs and live sessions later).
//!
//! The engine can be missing (`None`) and can restart. The client of the
//! engine keeps one subscription through a restart and reports `Crashed` and
//! `Restarted`, so the bus needs no new subscription.

use crossbeam_channel::{Receiver, Sender, bounded, select, unbounded};
use parking_lot::Mutex;
use sayso_core::stt::EngineEvent;
use sayso_platform::SttBackend;
use std::sync::Arc;
use std::thread::JoinHandle;

pub(crate) struct Bus {
    watchers: Arc<Mutex<Vec<Sender<EngineEvent>>>>,
    /// Dropping it stops the thread.
    stop: Mutex<Option<Sender<()>>>,
    /// Asks the thread to pass on the events that wait (see [`Bus::sync`]).
    syncs: Option<Sender<Sender<()>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Bus {
    /// Subscribe once to `engine`. Without an engine, nobody gets events.
    pub fn start(engine: Option<&Arc<dyn SttBackend>>) -> Bus {
        let watchers: Arc<Mutex<Vec<Sender<EngineEvent>>>> = Arc::default();
        let Some(engine) = engine else { return Bus::idle(watchers) };
        let events = engine.subscribe();
        let (stop, stopped) = bounded::<()>(0);
        let (syncs, synced) = unbounded::<Sender<()>>();
        let thread = {
            let watchers = watchers.clone();
            std::thread::Builder::new().name("sayso-host-engine".into()).spawn(move || fan_out(&events, &stopped, &synced, &watchers))
        };
        match thread {
            Ok(thread) => Bus { watchers, stop: Mutex::new(Some(stop)), syncs: Some(syncs), thread: Mutex::new(Some(thread)) },
            Err(e) => {
                log::warn!("host: cannot follow the events of the engine: {e}");
                Bus::idle(watchers)
            }
        }
    }

    fn idle(watchers: Arc<Mutex<Vec<Sender<EngineEvent>>>>) -> Bus {
        Bus { watchers, stop: Mutex::new(None), syncs: None, thread: Mutex::new(None) }
    }

    /// A new receiver of the events from now on. Watch before you look at a
    /// status, so a change between the look and the wait is not lost. The
    /// receiver ends when the bus stops.
    pub fn watch(&self) -> Receiver<EngineEvent> {
        let (tx, rx) = unbounded();
        self.watchers.lock().push(tx);
        rx
    }

    /// Return after the watchers got each event that the engine client sent
    /// before this call. The client sends the events of the engine before it
    /// gives the reply of a request, so after a reply and a sync, the events
    /// of that request are with the watchers.
    pub fn sync(&self) {
        let Some(syncs) = &self.syncs else { return };
        let (ack, acked) = bounded(1);
        if syncs.send(ack).is_ok() {
            let _ = acked.recv();
        }
    }

    /// Stop the thread and end every receiver.
    pub fn stop(&self) {
        *self.stop.lock() = None;
        if let Some(thread) = self.thread.lock().take() {
            let _ = thread.join();
        }
        self.watchers.lock().clear();
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        self.stop();
    }
}

fn fan_out(events: &Receiver<EngineEvent>, stopped: &Receiver<()>, synced: &Receiver<Sender<()>>, watchers: &Mutex<Vec<Sender<EngineEvent>>>) {
    let pass_on = |event: EngineEvent| watchers.lock().retain(|watcher| watcher.send(event.clone()).is_ok());
    loop {
        select! {
            recv(events) -> event => match event {
                // A watcher that dropped its receiver goes away.
                Ok(event) => pass_on(event),
                Err(_) => return,
            },
            recv(synced) -> ack => {
                for event in events.try_iter() {
                    pass_on(event);
                }
                if let Ok(ack) = ack {
                    let _ = ack.send(());
                }
            },
            // The sender is dropped: a receive fails at once.
            recv(stopped) -> _ => return,
        }
    }
}
