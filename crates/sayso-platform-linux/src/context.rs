//! [`ContextProvider`]: which app the user dictates into, and its icon.
//!
//! An app's id is the name of its desktop entry without `.desktop` (for
//! example `org.gnome.TextEditor` or `firefox`), the Linux form of a bundle
//! id. On X11 the active window gives the app. A Wayland session has no
//! common way to ask for the focused app, so the answer there is None.

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
        if self.session.kind != SessionKind::X11 {
            return None;
        }
        let window = x11_active_window()?;
        let entries = desktop_entries();
        let entry = window.class.as_deref().and_then(|c| entries.for_class(c));
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
}

// ---------------------------------------------------------------------------
// X11 active window
// ---------------------------------------------------------------------------

struct ActiveWindow {
    /// The class part of `WM_CLASS`.
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
// Desktop entries
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub wm_class: Option<String>,
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
    Some(DesktopEntry {
        id: id.to_string(),
        name,
        icon: values.get("Icon").map(|s| s.to_string()),
        wm_class: values.get("StartupWMClass").map(|s| s.to_string()),
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
    fn class_lookup_uses_wm_class_then_the_id() {
        let entry = |id: &str, wm: Option<&str>| DesktopEntry { id: id.into(), name: id.into(), icon: None, wm_class: wm.map(Into::into) };
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
