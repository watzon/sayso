//! Key presses through the XDG remote desktop portal, for GNOME and KDE
//! Plasma on Wayland.
//!
//! The portal asks the user once, in a system dialog. Sayso asks for the
//! keyboard only, with the persist mode "until revoked", and keeps the
//! restore token in `$XDG_STATE_HOME/sayso/remote-desktop.token`. With the
//! token, later starts need no dialog. The session stays open for the life
//! of the app, on its own thread.
//!
//! The portal takes keysyms, so the paste key works with every layout.
//! Typing works for the characters that the user's layout has: GNOME and
//! KDE look up each keysym in the active keymap.

use super::KeyInjector;
use super::keys::{self, PasteKey};
use ashpd::desktop::remote_desktop::{DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions, StartOptions};
use ashpd::desktop::{PersistMode, Session};
use ashpd::enumflags2::BitFlags;
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use futures_lite::future::block_on;
use sayso_platform::InsertError;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The longest wait for the portal. The first start shows a dialog, and the
/// user needs time to read it.
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// `AvailableDeviceTypes` bit of the keyboard.
const KEYBOARD: u32 = 1;

static PORTAL: OnceLock<Portal> = OnceLock::new();
static AVAILABLE: OnceLock<bool> = OnceLock::new();
/// The portal session runs.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// The file of the restore token, from `XDG_STATE_HOME` and `HOME`.
pub fn token_path_from(state_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let state = match state_home.map(PathBuf::from).filter(|p| p.is_absolute()) {
        Some(state) => state,
        None => PathBuf::from(home.filter(|h| !h.is_empty())?).join(".local/state"),
    };
    Some(state.join("sayso").join("remote-desktop.token"))
}

pub fn token_path() -> Option<PathBuf> {
    token_path_from(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"))
}

pub fn load_token(path: &Path) -> Option<String> {
    let token = std::fs::read_to_string(path).ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// Save the token, readable by the user only.
pub fn save_token(path: &Path, token: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    file.write_all(token.as_bytes())
}

/// Whether the session has a remote desktop portal with a keyboard. The
/// answer is kept after the first call.
pub fn available() -> bool {
    *AVAILABLE.get_or_init(|| {
        let types = available_device_types();
        log::info!("remote desktop portal: device types {types:?}");
        types.is_some_and(|t| t & KEYBOARD != 0)
    })
}

fn available_device_types() -> Option<u32> {
    let conn = zbus::blocking::Connection::session().ok()?;
    let reply = conn
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.portal.RemoteDesktop", "AvailableDeviceTypes"),
        )
        .ok()?;
    let value: zbus::zvariant::OwnedValue = reply.body().deserialize().ok()?;
    u32::try_from(value).ok()
}

/// Whether the portal can send keys without a dialog: the session runs, or
/// a saved token can start it.
pub fn ready() -> bool {
    RUNNING.load(Ordering::Relaxed) || token_path().is_some_and(|p| load_token(&p).is_some())
}

enum Command {
    Start(Option<Sender<Result<(), String>>>),
    Keys(Vec<(u32, bool)>, Sender<Result<(), String>>),
}

/// The handle of the portal thread.
pub struct Portal {
    tx: Sender<Command>,
}

impl Portal {
    pub fn get() -> &'static Portal {
        PORTAL.get_or_init(|| {
            let (tx, rx) = unbounded();
            let spawned = std::thread::Builder::new().name("sayso-portal".into()).spawn(move || run(rx));
            if let Err(e) = spawned {
                log::warn!("cannot start the portal thread: {e}");
            }
            Portal { tx }
        })
    }

    /// Start the session in the background. The first start shows the dialog.
    pub fn request(&self) {
        let _ = self.tx.send(Command::Start(None));
    }

    fn keys(&self, keys: Vec<(u32, bool)>) -> Result<(), InsertError> {
        let (reply, answer) = bounded(1);
        self.tx.send(Command::Keys(keys, reply)).map_err(|_| InsertError::NotTrusted)?;
        match answer.recv_timeout(START_TIMEOUT) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                log::warn!("remote desktop portal: {e}");
                Err(InsertError::NotTrusted)
            }
            Err(_) => Err(InsertError::Failed("the remote desktop portal does not answer".into())),
        }
    }
}

impl KeyInjector for Portal {
    fn paste_key(&self, key: PasteKey) -> Result<(), InsertError> {
        let mut presses: Vec<(u32, bool)> = key.modifiers().iter().map(|m| (m.keysym, true)).collect();
        presses.push((key.key().keysym, true));
        presses.push((key.key().keysym, false));
        presses.extend(key.modifiers().iter().rev().map(|m| (m.keysym, false)));
        self.keys(presses)
    }

    fn type_text(&self, text: &str) -> Result<(), InsertError> {
        let syms = keys::text_keysyms(text).map_err(|c| InsertError::Failed(format!("cannot type {c:?}")))?;
        self.keys(syms.into_iter().flat_map(|s| [(s, true), (s, false)]).collect())
    }
}

struct Active {
    proxy: RemoteDesktop,
    session: Session<RemoteDesktop>,
}

fn run(rx: Receiver<Command>) {
    let mut active: Option<Active> = None;
    for command in rx {
        match command {
            Command::Start(reply) => {
                let result = block_on(ensure(&mut active));
                if let Err(e) = &result {
                    log::warn!("cannot start the remote desktop session: {e}");
                }
                if let Some(reply) = reply {
                    let _ = reply.send(result);
                }
            }
            Command::Keys(keys, reply) => {
                let mut result = block_on(send(&mut active, &keys));
                if result.is_err() && active.is_none() {
                    // The session was closed (for example revoked): start it once more.
                    result = block_on(send(&mut active, &keys));
                }
                let _ = reply.send(result);
            }
        }
    }
}

async fn ensure(active: &mut Option<Active>) -> Result<(), String> {
    if active.is_some() {
        return Ok(());
    }
    let proxy = RemoteDesktop::new().await.map_err(|e| e.to_string())?;
    let session = proxy.create_session(Default::default()).await.map_err(|e| e.to_string())?;
    let path = token_path();
    let token = path.as_deref().and_then(load_token);
    let options = SelectDevicesOptions::default()
        .set_devices(BitFlags::from_flag(DeviceType::Keyboard))
        .set_persist_mode(PersistMode::ExplicitlyRevoked)
        .set_restore_token(token.as_deref());
    proxy.select_devices(&session, options).await.map_err(|e| e.to_string())?.response().map_err(|e| e.to_string())?;
    let started = proxy.start(&session, None, StartOptions::default()).await.map_err(|e| e.to_string())?;
    let selected = started.response().map_err(|e| e.to_string())?;
    if !selected.devices().contains(DeviceType::Keyboard) {
        return Err("the user did not allow the keyboard".into());
    }
    // A token works once; each start gives a new one.
    if let (Some(path), Some(token)) = (path, selected.restore_token())
        && let Err(e) = save_token(&path, token)
    {
        log::warn!("cannot save the remote desktop token: {e}");
    }
    RUNNING.store(true, Ordering::Relaxed);
    *active = Some(Active { proxy, session });
    Ok(())
}

async fn send(active: &mut Option<Active>, keys: &[(u32, bool)]) -> Result<(), String> {
    ensure(active).await?;
    let Some(a) = active.as_ref() else { return Err("no session".into()) };
    for &(sym, down) in keys {
        let state = if down { KeyState::Pressed } else { KeyState::Released };
        let sent = a.proxy.notify_keyboard_keysym(&a.session, sym as i32, state, Default::default()).await;
        if let Err(e) = sent {
            *active = None;
            RUNNING.store(false, Ordering::Relaxed);
            return Err(e.to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_lives_under_the_state_home() {
        assert_eq!(
            token_path_from(Some("/s".into()), Some("/home/u".into())),
            Some(PathBuf::from("/s/sayso/remote-desktop.token"))
        );
        assert_eq!(
            token_path_from(None, Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.local/state/sayso/remote-desktop.token"))
        );
        // A relative XDG path is invalid and is ignored (XDG Base Directory spec).
        assert_eq!(
            token_path_from(Some("rel".into()), Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.local/state/sayso/remote-desktop.token"))
        );
        assert_eq!(token_path_from(None, None), None);
    }

    #[test]
    fn a_saved_token_is_read_back_and_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sayso/remote-desktop.token");
        assert_eq!(load_token(&path), None);
        save_token(&path, "abc-123").unwrap();
        assert_eq!(load_token(&path).as_deref(), Some("abc-123"));
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        save_token(&path, "new").unwrap();
        assert_eq!(load_token(&path).as_deref(), Some("new"), "a new token replaces the old one");
        std::fs::write(&path, "  \n").unwrap();
        assert_eq!(load_token(&path), None, "an empty file is no token");
    }
}
