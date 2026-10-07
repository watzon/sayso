//! [`ContextProvider`]: which app the user dictates into, and its icon.
//!
//! An app's id is the name of its desktop entry without `.desktop` (for
//! example `org.gnome.TextEditor` or `firefox`), the Linux form of a bundle
//! id. On X11 the active window gives the app. On a Wayland compositor with
//! the wlr foreign-toplevel protocol (sway, Hyprland, and others), the
//! activated toplevel gives it. GNOME and KDE have no way to ask for the
//! focused app, so the answer there is None.

use crate::session::{Session, SessionKind};
use sayso_platform::{AppInfo, ContextProvider};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

pub struct LinuxContext {
    session: &'static Session,
}

impl LinuxContext {
    pub fn new(session: &'static Session) -> Self {
        Self { session }
    }
}

impl ContextProvider for LinuxContext {
    fn frontmost_app(&self) -> Option<AppInfo> {
        let window = match self.session.kind {
            SessionKind::X11 => x11_active_window()?,
            SessionKind::Wayland if self.session.has_global(toplevel::MANAGER) => {
                ActiveWindow { class: Some(toplevel::active_app_id()?), pid: None }
            }
            _ => return None,
        };
        let entries = desktop_entries();
        // A Wayland app id is the id of the desktop entry. An X11 class goes through `StartupWMClass`.
        let entry = window.class.as_deref().and_then(|c| entries.by_id.get(c).or_else(|| entries.for_class(c)));
        let bundle_id = entry.map(|e| e.id.clone()).or_else(|| window.class.clone())?;
        let name = entry.map(|e| e.name.clone()).or_else(|| window.class.clone()).unwrap_or_else(|| bundle_id.clone());
        Some(AppInfo { bundle_id, name, pid: window.pid.unwrap_or(0) })
    }

    fn running_apps(&self) -> Vec<AppInfo> {
        // Only macOS uses running apps (for hotkey conflict hints).
        Vec::new()
    }

    fn app_icon_png(&self, bundle_id: &str, size: u32) -> Option<Vec<u8>> {
        let entries = desktop_entries();
        let icon = entries.by_id.get(bundle_id).and_then(|e| e.icon.clone()).unwrap_or_else(|| bundle_id.to_string());
        let path = find_icon(&icon, size)?;
        std::fs::read(path).ok()
    }

    fn installed_apps(&self) -> Vec<AppInfo> {
        listed_apps(desktop_entries())
    }
}

/// The apps that the launcher of the desktop shows, by name.
pub fn listed_apps(entries: &DesktopEntries) -> Vec<AppInfo> {
    let mut apps: Vec<AppInfo> =
        entries.by_id.values().filter(|e| e.listed).map(|e| AppInfo { bundle_id: e.id.clone(), name: e.name.clone(), pid: 0 }).collect();
    apps.sort_by_key(|a| (a.name.to_lowercase(), a.bundle_id.clone()));
    apps
}

// ---------------------------------------------------------------------------
// X11 active window
// ---------------------------------------------------------------------------

struct ActiveWindow {
    /// The class part of `WM_CLASS`, or the app id of a Wayland toplevel.
    class: Option<String>,
    pid: Option<i32>,
}

fn x11_active_window() -> Option<ActiveWindow> {
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let atom = |name: &[u8]| conn.intern_atom(false, name).ok()?.reply().ok().map(|r| r.atom);
    let active = conn
        .get_property(false, root, atom(b"_NET_ACTIVE_WINDOW")?, AtomEnum::WINDOW, 0, 1)
        .ok()?
        .reply()
        .ok()?
        .value32()?
        .next()
        .filter(|w| *w != 0)?;
    let class = conn
        .get_property(false, active, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)
        .ok()
        .and_then(|c| c.reply().ok())
        .and_then(|r| parse_wm_class(&r.value));
    let pid = atom(b"_NET_WM_PID")
        .and_then(|a| conn.get_property(false, active, a, AtomEnum::CARDINAL, 0, 1).ok())
        .and_then(|c| c.reply().ok())
        .and_then(|r| r.value32().and_then(|mut v| v.next()))
        .map(|p| p as i32);
    Some(ActiveWindow { class, pid })
}

/// `WM_CLASS` is "instance\0class\0". The class names the app.
pub fn parse_wm_class(value: &[u8]) -> Option<String> {
    let mut parts = value.split(|b| *b == 0).filter(|p| !p.is_empty());
    let instance = parts.next();
    let class = parts.next().or(instance)?;
    Some(String::from_utf8_lossy(class).into_owned())
}

// ---------------------------------------------------------------------------
// Wayland activated toplevel
// ---------------------------------------------------------------------------

/// The app id of the activated window, from the wlr foreign-toplevel protocol.
mod toplevel {
    use std::collections::HashMap;
    use wayland_client::backend::ObjectId;
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_registry::WlRegistry;
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};
    use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::{self as handle, ZwlrForeignToplevelHandleV1 as Handle};
    use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::{self as manager, ZwlrForeignToplevelManagerV1 as Manager};

    /// The name of the global that this needs.
    pub const MANAGER: &str = "zwlr_foreign_toplevel_manager_v1";

    #[derive(Default)]
    struct Toplevel {
        app_id: Option<String>,
        activated: bool,
    }

    #[derive(Default)]
    struct State {
        toplevels: HashMap<ObjectId, Toplevel>,
    }

    /// One short connection: the compositor sends all toplevels with their
    /// state when a client binds the manager. None when no window is active.
    pub fn active_app_id() -> Option<String> {
        let conn = Connection::connect_to_env().ok()?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
        let _manager: Manager = globals.bind(&queue.handle(), 1..=3, ()).ok()?;
        let mut state = State::default();
        // The first round gives the handles, the second their app ids and states.
        queue.roundtrip(&mut state).ok()?;
        queue.roundtrip(&mut state).ok()?;
        state.toplevels.into_values().find(|t| t.activated).and_then(|t| t.app_id).filter(|id| !id.is_empty())
    }

    /// The `state` event is an array of 32-bit values in the byte order of this computer.
    pub(super) fn is_activated(state: &[u8]) -> bool {
        state.as_chunks::<4>().0.iter().any(|v| u32::from_ne_bytes(*v) == handle::State::Activated as u32)
    }

    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
    }

    impl Dispatch<Manager, ()> for State {
        fn event(_: &mut Self, _: &Manager, _: manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}

        event_created_child!(State, Manager, [manager::EVT_TOPLEVEL_OPCODE => (Handle, ())]);
    }

    impl Dispatch<Handle, ()> for State {
        fn event(state: &mut Self, toplevel: &Handle, event: handle::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
            let entry = state.toplevels.entry(toplevel.id()).or_default();
            match event {
                handle::Event::AppId { app_id } => entry.app_id = Some(app_id),
                handle::Event::State { state } => entry.activated = is_activated(&state),
                handle::Event::Closed => {
                    state.toplevels.remove(&toplevel.id());
                }
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Desktop entries
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub wm_class: Option<String>,
    /// An app that the launcher shows: not `NoDisplay`, not `Hidden`, and of type `Application`.
    pub listed: bool,
}

pub struct DesktopEntries {
    pub by_id: HashMap<String, DesktopEntry>,
}

impl DesktopEntries {
    /// The entry for a `WM_CLASS` class: its `StartupWMClass`, or an id that
    /// matches the class or ends with it (`org.gnome.Nautilus` for `Nautilus`).
    pub fn for_class(&self, class: &str) -> Option<&DesktopEntry> {
        let lower = class.to_ascii_lowercase();
        self.by_id
            .values()
            .find(|e| e.wm_class.as_deref().is_some_and(|w| w.eq_ignore_ascii_case(class)))
            .or_else(|| self.by_id.values().find(|e| e.id.to_ascii_lowercase() == lower))
            .or_else(|| self.by_id.values().find(|e| e.id.to_ascii_lowercase().ends_with(&format!(".{lower}"))))
    }
}

/// The folders that hold `applications/` and `icons/`, most specific first.
pub fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let mut dirs = vec![
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share")),
        home.join(".local/share/flatpak/exports/share"),
        PathBuf::from("/var/lib/flatpak/exports/share"),
    ];
    let system = std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty()).unwrap_or("/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').map(PathBuf::from));
    dirs
}

fn desktop_entries() -> &'static DesktopEntries {
    static ENTRIES: OnceLock<DesktopEntries> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let mut by_id = HashMap::new();
        for dir in data_dirs() {
            let Ok(files) = std::fs::read_dir(dir.join("applications")) else { continue };
            for file in files.flatten() {
                let path = file.path();
                let Some(id) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".desktop")) else {
                    continue;
                };
                // The first folder wins, as in the XDG lookup.
                if by_id.contains_key(id) {
                    continue;
                }
                if let Some(entry) = std::fs::read_to_string(&path).ok().and_then(|text| parse_desktop_entry(id, &text)) {
                    by_id.insert(id.to_string(), entry);
                }
            }
        }
        DesktopEntries { by_id }
    })
}

/// Read the `[Desktop Entry]` group of a desktop file.
pub fn parse_desktop_entry(id: &str, text: &str) -> Option<DesktopEntry> {
    let mut in_group = false;
    let mut values: HashMap<&str, &str> = HashMap::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            values.entry(key.trim()).or_insert(value.trim());
        }
    }
    let name = values.get("Name")?.to_string();
    let is_true = |key: &str| values.get(key).is_some_and(|v| v.eq_ignore_ascii_case("true"));
    let listed = values.get("Type").is_none_or(|t| *t == "Application") && !is_true("NoDisplay") && !is_true("Hidden");
    Some(DesktopEntry {
        id: id.to_string(),
        name,
        icon: values.get("Icon").map(|s| s.to_string()),
        wm_class: values.get("StartupWMClass").map(|s| s.to_string()),
        listed,
    })
}

/// A PNG file for an icon name or path, close to `size` pixels.
fn find_icon(icon: &str, size: u32) -> Option<PathBuf> {
    let direct = Path::new(icon);
    if direct.is_absolute() {
        return (direct.extension().is_some_and(|e| e == "png") && direct.exists()).then(|| direct.to_path_buf());
    }
    let mut sizes = vec![size, 48, 64, 128, 32, 256, 512, 24, 16];
    sizes.dedup();
    for dir in data_dirs() {
        for s in &sizes {
            let path = dir.join(format!("icons/hicolor/{s}x{s}/apps/{icon}.png"));
            if path.exists() {
                return Some(path);
            }
        }
    }
    let pixmap = PathBuf::from(format!("/usr/share/pixmaps/{icon}.png"));
    pixmap.exists().then_some(pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wm_class_names_the_class() {
        assert_eq!(parse_wm_class(b"navigator\0firefox\0").as_deref(), Some("firefox"));
        assert_eq!(parse_wm_class(b"code\0").as_deref(), Some("code"));
        assert_eq!(parse_wm_class(b""), None);
    }

    #[test]
    fn desktop_entry_reads_only_its_group() {
        let text = "[Desktop Entry]\nName=Text Editor\nIcon=org.gnome.TextEditor\n# c\nStartupWMClass=gnome-text-editor\n\n[Desktop Action new]\nName=New Window\n";
        let e = parse_desktop_entry("org.gnome.TextEditor", text).unwrap();
        assert_eq!(e.name, "Text Editor");
        assert_eq!(e.icon.as_deref(), Some("org.gnome.TextEditor"));
        assert_eq!(e.wm_class.as_deref(), Some("gnome-text-editor"));
        assert_eq!(parse_desktop_entry("x", "[Other]\nName=Y\n"), None);
    }

    #[test]
    fn the_app_list_has_only_what_the_launcher_shows() {
        let text = |extra: &str| format!("[Desktop Entry]\nName=Files\n{extra}");
        assert!(parse_desktop_entry("a", &text("Type=Application\n")).unwrap().listed);
        assert!(parse_desktop_entry("a", &text("")).unwrap().listed);
        assert!(!parse_desktop_entry("a", &text("NoDisplay=true\n")).unwrap().listed);
        assert!(!parse_desktop_entry("a", &text("Hidden=true\n")).unwrap().listed);
        assert!(!parse_desktop_entry("a", &text("Type=Link\n")).unwrap().listed);

        let entry = |id: &str, name: &str, listed: bool| (id.to_string(), DesktopEntry { id: id.into(), name: name.into(), icon: None, wm_class: None, listed });
        let entries = DesktopEntries { by_id: [entry("b", "beta", true), entry("a", "Alpha", true), entry("c", "Helper", false)].into_iter().collect() };
        let names: Vec<String> = listed_apps(&entries).into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["Alpha", "beta"]);
    }

    #[test]
    fn the_activated_state_is_found_in_the_state_array() {
        let array = |values: &[u32]| values.iter().flat_map(|v| v.to_ne_bytes()).collect::<Vec<u8>>();
        // 0 maximized, 1 minimized, 2 activated, 3 fullscreen.
        assert!(toplevel::is_activated(&array(&[0, 2])));
        assert!(!toplevel::is_activated(&array(&[0, 3])));
        assert!(!toplevel::is_activated(&[]));
    }

    /// Needs a wlroots compositor with one focused window:
    /// `SAYSO_EXPECT_APP_ID=foot cargo test -p sayso-platform-linux live_wayland_active_app -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_wayland_active_app() {
        let expected = std::env::var("SAYSO_EXPECT_APP_ID").expect("set SAYSO_EXPECT_APP_ID");
        let started = std::time::Instant::now();
        let app = toplevel::active_app_id();
        println!("active app id: {app:?} in {:?}", started.elapsed());
        assert_eq!(app.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn class_lookup_uses_wm_class_then_the_id() {
        let entry = |id: &str, wm: Option<&str>| DesktopEntry { id: id.into(), name: id.into(), icon: None, wm_class: wm.map(Into::into), listed: true };
        let entries = DesktopEntries {
            by_id: [
                ("org.gnome.Nautilus".to_string(), entry("org.gnome.Nautilus", None)),
                ("code".to_string(), entry("code", Some("Code"))),
                ("firefox".to_string(), entry("firefox", None)),
            ]
            .into_iter()
            .collect(),
        };
        assert_eq!(entries.for_class("Code").unwrap().id, "code");
        assert_eq!(entries.for_class("firefox").unwrap().id, "firefox");
        assert_eq!(entries.for_class("Nautilus").unwrap().id, "org.gnome.Nautilus");
        assert!(entries.for_class("unknown").is_none());
    }
}
