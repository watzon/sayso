//! Chords through the XDG GlobalShortcuts portal, for Wayland sessions
//! (GNOME 48 and later, KDE Plasma, Hyprland).
//!
//! The portal asks the user to confirm the shortcuts in its own dialog the
//! first time, and the user can choose other keys there. `Activated` and
//! `Deactivated` signals give key press and release, so push-to-talk works
//! for chords too. The compositor takes the keys, so the focused app does not
//! get them.
//!
//! ashpd uses async-io, so the code runs on its own thread with
//! `futures_lite::future::block_on`. A new set of bindings closes the old
//! portal session and creates a new one.

use super::listen::Listen;
use super::ChordAction;
use ashpd::desktop::global_shortcuts::{
    Activated, BindShortcutsOptions, Deactivated, GlobalShortcuts, NewShortcut,
};
use ashpd::desktop::{CreateSessionOptions, Session};
use futures_lite::{StreamExt, future};
use sayso_platform::HotkeyEvent;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

/// How long `start` waits for the portal to answer.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// One shortcut to bind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// Stable id. The portal remembers the user's choice under it.
    pub id: &'static str,
    pub description: &'static str,
    /// The preferred trigger in the shortcuts specification format.
    pub trigger: String,
    pub action: ChordAction,
}

/// The portal thread. Dropping it ends the thread after its current step.
pub struct Portal {
    commands: async_channel::Sender<Vec<Binding>>,
}

impl Portal {
    /// Start the thread and check that the portal has the GlobalShortcuts
    /// interface. Fails when it does not answer in time.
    pub fn start(listen: Arc<Listen>) -> Result<Self, String> {
        let (commands, rx) = async_channel::unbounded();
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("sayso-portal-shortcuts".into())
            .spawn(move || {
                // A panic inside ashpd must not take down more than this thread.
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| future::block_on(run(listen, rx, ready_tx))));
                if result.is_err() {
                    log::error!("the portal thread panicked; portal shortcuts stop");
                }
            })
            .map_err(|e| format!("cannot start the portal thread: {e}"))?;
        match ready_rx.recv_timeout(PROBE_TIMEOUT) {
            Ok(Ok(())) => Ok(Self { commands }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("the GlobalShortcuts portal did not answer".into()),
        }
    }

    /// Replace the bound shortcuts. The result arrives later, after the
    /// portal dialog; failures go to the log.
    pub fn bind(&self, bindings: Vec<Binding>) {
        if self.commands.send_blocking(bindings).is_err() {
            log::warn!("the portal thread stopped; shortcuts are not bound");
        }
    }
}

async fn run(
    listen: Arc<Listen>,
    commands: async_channel::Receiver<Vec<Binding>>,
    ready: crossbeam_channel::Sender<Result<(), String>>,
) {
    let portal = match GlobalShortcuts::new().await {
        Ok(portal) => portal,
        Err(e) => {
            let _ = ready.send(Err(format!("the GlobalShortcuts portal is not available: {e}")));
            return;
        }
    };
    let streams = async { Ok::<_, ashpd::Error>((portal.receive_activated().await?, portal.receive_deactivated().await?)) };
    let (mut activated, mut deactivated) = match streams.await {
        Ok(streams) => streams,
        Err(e) => {
            let _ = ready.send(Err(format!("cannot listen to the GlobalShortcuts portal: {e}")));
            return;
        }
    };
    log::info!("GlobalShortcuts portal version {}", portal.version());
    let _ = ready.send(Ok(()));

    enum Next {
        Bind(Option<Vec<Binding>>),
        Activated(Option<Activated>),
        Deactivated(Option<Deactivated>),
    }
    let mut session: Option<Session<GlobalShortcuts>> = None;
    let mut bound: Vec<Binding> = Vec::new();
    let mut ptt_down = false;
    loop {
        let next = future::or(
            async { Next::Bind(commands.recv().await.ok()) },
            future::or(
                async { Next::Activated(activated.next().await) },
                async { Next::Deactivated(deactivated.next().await) },
            ),
        )
        .await;
        match next {
            Next::Bind(None) => break,
            Next::Bind(Some(bindings)) => {
                if ptt_down {
                    ptt_down = false;
                    listen.send(HotkeyEvent::PushToTalkUp);
                }
                if let Some(old) = session.take()
                    && let Err(e) = old.close().await
                {
                    log::debug!("cannot close the old portal session: {e}");
                }
                bound = bindings;
                if !bound.is_empty() {
                    session = bind(&portal, &bound).await;
                }
            }
            Next::Activated(Some(signal)) => {
                let Some(action) = action_of(&bound, signal.shortcut_id()) else { continue };
                if listen.chords_muted() {
                    continue;
                }
                match action {
                    ChordAction::Press(event) => listen.send(event),
                    ChordAction::PushToTalk if !ptt_down => {
                        ptt_down = true;
                        listen.send(HotkeyEvent::PushToTalkDown);
                    }
                    ChordAction::PushToTalk => {}
                }
            }
            Next::Deactivated(Some(signal)) => {
                if action_of(&bound, signal.shortcut_id()) == Some(ChordAction::PushToTalk) && ptt_down {
                    ptt_down = false;
                    listen.send(HotkeyEvent::PushToTalkUp);
                }
            }
            Next::Activated(None) | Next::Deactivated(None) => {
                log::warn!("the GlobalShortcuts portal went away; shortcuts stop");
                break;
            }
        }
    }
    if ptt_down {
        listen.send(HotkeyEvent::PushToTalkUp);
    }
    if let Some(old) = session {
        let _ = old.close().await;
    }
}

fn action_of(bound: &[Binding], id: &str) -> Option<ChordAction> {
    bound.iter().find(|b| b.id == id).map(|b| b.action)
}

/// Create a session and bind the shortcuts. Waits for the portal dialog.
async fn bind(portal: &GlobalShortcuts, bindings: &[Binding]) -> Option<Session<GlobalShortcuts>> {
    let session = match portal.create_session(CreateSessionOptions::default()).await {
        Ok(session) => session,
        Err(e) => {
            log::warn!("cannot create a GlobalShortcuts session: {e}");
            return None;
        }
    };
    let shortcuts: Vec<NewShortcut> = bindings
        .iter()
        .map(|b| NewShortcut::new(b.id, b.description).preferred_trigger(b.trigger.as_str()))
        .collect();
    let response = match portal.bind_shortcuts(&session, &shortcuts, None, BindShortcutsOptions::default()).await {
        Ok(request) => request.response(),
        Err(e) => Err(e),
    };
    match response {
        Ok(result) => {
            for b in bindings {
                match result.shortcuts().iter().find(|s| s.id() == b.id) {
                    Some(s) => log::info!("portal shortcut {} is {:?}", b.id, s.trigger_description()),
                    None => log::warn!("the portal did not bind {} ({})", b.id, b.trigger),
                }
            }
        }
        Err(e) => log::warn!("the portal did not bind the shortcuts: {e}"),
    }
    Some(session)
}
