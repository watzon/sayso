//! What kind of desktop Sayso runs in, and which display server GPUI uses.
//!
//! Call [`prepare`] once, first thing in `main`, before GPUI starts and before
//! any other thread exists. On a Wayland compositor without the layer-shell
//! protocol (GNOME), it clears `WAYLAND_DISPLAY`, so GPUI connects to XWayland:
//! only an X11 window can float over other apps there without taking focus.
//! The services in this crate still use the Wayland session through
//! [`Session::wayland_display`] and the desktop portals.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::sync::OnceLock;

/// The display server of the desktop session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    X11,
    Wayland,
    /// No display server (a test run, or SSH without forwarding).
    None,
}

/// The display server that GPUI talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiBackend {
    X11,
    Wayland,
    Headless,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Desktop {
    Gnome,
    Kde,
    Sway,
    Hyprland,
    Other(String),
}

#[derive(Debug, Clone)]
pub struct Session {
    pub kind: SessionKind,
    pub ui: UiBackend,
    pub desktop: Desktop,
    /// The Wayland socket of the session, also when GPUI uses XWayland.
    pub wayland_display: Option<OsString>,
    pub x11_display: Option<OsString>,
    /// Interface names of the compositor's Wayland globals. Empty on X11.
    pub wayland_globals: BTreeSet<String>,
}

impl Session {
    pub fn is_wayland(&self) -> bool {
        self.kind == SessionKind::Wayland
    }

    pub fn has_global(&self, interface: &str) -> bool {
        self.wayland_globals.contains(interface)
    }

    /// The compositor can place surfaces over other apps (wlr layer shell).
    pub fn has_layer_shell(&self) -> bool {
        self.has_global("zwlr_layer_shell_v1")
    }

    /// True when X11 calls reach the windows the user types in. On a Wayland
    /// session, XWayland only reaches other X11 apps.
    pub fn x11_reaches_all_apps(&self) -> bool {
        self.kind == SessionKind::X11
    }

    /// Run `command` with the session's original environment, for programs
    /// that should open in the user's real session (a browser, a file manager).
    pub fn restore_env(&self, command: &mut std::process::Command) {
        if let Some(display) = &self.wayland_display {
            command.env("WAYLAND_DISPLAY", display);
        }
    }
}

static SESSION: OnceLock<Session> = OnceLock::new();

/// Detect the session and pick GPUI's backend. Safe to call more than once;
/// only the first call changes the environment.
///
/// `SAYSO_UI_BACKEND=x11` or `=wayland` overrides the choice.
pub fn prepare() -> &'static Session {
    SESSION.get_or_init(|| {
        let session = detect();
        if session.ui == UiBackend::X11 && session.kind == SessionKind::Wayland {
            log::info!("the compositor has no layer shell; the UI uses XWayland");
            // SAFETY: `prepare` runs at the start of `main`, before GPUI or any
            // other thread starts, so nothing reads the environment concurrently.
            unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
        }
        if session.kind != SessionKind::None {
            register_app_id();
        }
        session
    })
}

/// The app id the portals know Sayso by. The desktop entry has the same name.
pub const APP_ID: &str = "dev.sayso.Sayso";

/// Tell the portals which app this is, so their dialogs (shortcuts, remote
/// desktop) name Sayso. It must come before any other portal call. A portal
/// without the registry, or no portal at all, is fine.
fn register_app_id() {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let spawned = std::thread::Builder::new().name("sayso-portal-id".into()).spawn(move || {
        let result = ashpd::AppID::try_from(APP_ID)
            .map_err(|e| e.to_string())
            .and_then(|id| futures_lite::future::block_on(ashpd::register_host_app(id)).map_err(|e| e.to_string()));
        let _ = tx.send(result);
    });
    if spawned.is_err() {
        return;
    }
    // Do not hold up the start when the bus is slow.
    match rx.recv_timeout(std::time::Duration::from_secs(2)) {
        Ok(Ok(())) => log::info!("registered with the desktop portals as {APP_ID}"),
        Ok(Err(e)) => log::info!("the desktop portals did not take the app id: {e}"),
        Err(_) => log::warn!("the desktop portals did not answer"),
    }
}

/// The session. Calls [`prepare`] if nothing did yet.
pub fn current() -> &'static Session {
    prepare()
}

fn var(key: &str) -> Option<OsString> {
    std::env::var_os(key).filter(|v| !v.is_empty())
}

fn detect() -> Session {
    let wayland_display = var("WAYLAND_DISPLAY");
    let x11_display = var("DISPLAY");
    let kind = match (std::env::var("XDG_SESSION_TYPE").ok().as_deref(), &wayland_display, &x11_display) {
        (Some("wayland"), _, _) | (_, Some(_), _) => SessionKind::Wayland,
        (Some("x11"), _, _) | (_, None, Some(_)) => SessionKind::X11,
        _ => SessionKind::None,
    };
    let wayland_globals = match kind {
        SessionKind::Wayland => wayland_globals(wayland_display.as_ref()),
        _ => BTreeSet::new(),
    };
    let desktop = desktop(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref());
    let layer_shell = wayland_globals.contains("zwlr_layer_shell_v1");
    let ui = choose_ui(kind, layer_shell, x11_display.is_some(), std::env::var("SAYSO_UI_BACKEND").ok().as_deref());
    let session = Session { kind, ui, desktop, wayland_display, x11_display, wayland_globals };
    log::info!(
        "session: {:?}, desktop {:?}, UI backend {:?}, layer shell {}",
        session.kind,
        session.desktop,
        session.ui,
        layer_shell
    );
    session
}

/// GPUI's backend: native Wayland when the overlay can float (layer shell),
/// else XWayland when it exists.
pub fn choose_ui(kind: SessionKind, layer_shell: bool, has_x11: bool, forced: Option<&str>) -> UiBackend {
    match forced {
        Some("x11") if has_x11 => return UiBackend::X11,
        Some("wayland") if kind == SessionKind::Wayland => return UiBackend::Wayland,
        _ => {}
    }
    match kind {
        SessionKind::Wayland if layer_shell || !has_x11 => UiBackend::Wayland,
        SessionKind::Wayland | SessionKind::X11 => UiBackend::X11,
        SessionKind::None if has_x11 => UiBackend::X11,
        SessionKind::None => UiBackend::Headless,
    }
}

pub fn desktop(xdg_current_desktop: Option<&str>) -> Desktop {
    // A colon-separated list, for example "ubuntu:GNOME".
    let names: Vec<String> = xdg_current_desktop.unwrap_or_default().split(':').map(|s| s.to_ascii_lowercase()).collect();
    let has = |n: &str| names.iter().any(|x| x == n);
    if has("gnome") {
        Desktop::Gnome
    } else if has("kde") {
        Desktop::Kde
    } else if has("sway") {
        Desktop::Sway
    } else if has("hyprland") {
        Desktop::Hyprland
    } else {
        Desktop::Other(xdg_current_desktop.unwrap_or_default().to_string())
    }
}

/// The interface names the compositor offers. Empty when it cannot be reached.
fn wayland_globals(display: Option<&OsString>) -> BTreeSet<String> {
    use wayland_client::protocol::wl_registry;
    use wayland_client::{Connection, Dispatch, QueueHandle};

    struct Names(BTreeSet<String>);
    impl Dispatch<wl_registry::WlRegistry, ()> for Names {
        fn event(
            state: &mut Self,
            _: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let wl_registry::Event::Global { interface, .. } = event {
                state.0.insert(interface);
            }
        }
    }

    if display.is_none() {
        return BTreeSet::new();
    }
    let Ok(conn) = Connection::connect_to_env() else {
        log::warn!("cannot connect to the Wayland compositor");
        return BTreeSet::new();
    };
    let mut queue = conn.new_event_queue();
    let _registry = conn.display().get_registry(&queue.handle(), ());
    let mut names = Names(BTreeSet::new());
    if let Err(e) = queue.roundtrip(&mut names) {
        log::warn!("cannot list the Wayland globals: {e}");
    }
    names.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnome_wayland_uses_xwayland_and_wlroots_stays_native() {
        assert_eq!(choose_ui(SessionKind::Wayland, false, true, None), UiBackend::X11);
        assert_eq!(choose_ui(SessionKind::Wayland, true, true, None), UiBackend::Wayland);
        // Without XWayland there is no choice.
        assert_eq!(choose_ui(SessionKind::Wayland, false, false, None), UiBackend::Wayland);
        assert_eq!(choose_ui(SessionKind::X11, false, true, None), UiBackend::X11);
        assert_eq!(choose_ui(SessionKind::None, false, false, None), UiBackend::Headless);
    }

    #[test]
    fn the_override_wins_when_possible() {
        assert_eq!(choose_ui(SessionKind::Wayland, true, true, Some("x11")), UiBackend::X11);
        assert_eq!(choose_ui(SessionKind::Wayland, false, true, Some("wayland")), UiBackend::Wayland);
        assert_eq!(choose_ui(SessionKind::X11, false, true, Some("wayland")), UiBackend::X11);
    }

    #[test]
    fn desktops_are_read_from_the_colon_list() {
        assert_eq!(desktop(Some("ubuntu:GNOME")), Desktop::Gnome);
        assert_eq!(desktop(Some("KDE")), Desktop::Kde);
        assert_eq!(desktop(Some("sway")), Desktop::Sway);
        assert_eq!(desktop(Some("Hyprland")), Desktop::Hyprland);
        assert_eq!(desktop(Some("XFCE")), Desktop::Other("XFCE".into()));
        assert_eq!(desktop(None), Desktop::Other(String::new()));
    }
}
