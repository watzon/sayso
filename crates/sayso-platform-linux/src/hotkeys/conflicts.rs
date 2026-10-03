//! Hotkey conflicts with common desktop shortcuts.
//!
//! On GNOME, Sayso reads the current values with the `gsettings` command and
//! uses the GNOME defaults when it cannot. On KDE Plasma, Xfce, Cinnamon, and
//! MATE, a small table of their defaults is the hint. Sway and Hyprland have
//! no fixed defaults, so they get no hints.

use crate::session::Desktop;
use parking_lot::Mutex;
use sayso_core::hotkey::{Hotkey, Key, Modifiers};
use sayso_platform::{ConflictSource, HotkeyConflict};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a read of the desktop settings stays valid.
const CACHE_TTL: Duration = Duration::from_secs(10);

/// One GNOME setting that holds shortcuts, with its default value.
struct GnomeKey {
    schema: &'static str,
    key: &'static str,
    name: &'static str,
    defaults: &'static [&'static str],
}

const WM: &str = "org.gnome.desktop.wm.keybindings";
const SHELL: &str = "org.gnome.shell.keybindings";
const MEDIA: &str = "org.gnome.settings-daemon.plugins.media-keys";

const GNOME: &[GnomeKey] = &[
    GnomeKey { schema: WM, key: "activate-window-menu", name: "the window menu", defaults: &["<Alt>space"] },
    GnomeKey { schema: WM, key: "switch-input-source", name: "Switch input source", defaults: &["<Super>space"] },
    GnomeKey {
        schema: WM,
        key: "switch-input-source-backward",
        name: "Switch input source",
        defaults: &["<Shift><Super>space"],
    },
    GnomeKey {
        schema: WM,
        key: "switch-applications",
        name: "Switch applications",
        defaults: &["<Super>Tab", "<Alt>Tab"],
    },
    GnomeKey { schema: WM, key: "close", name: "Close window", defaults: &["<Alt>F4"] },
    GnomeKey { schema: WM, key: "panel-run-dialog", name: "the Run a command dialog", defaults: &["<Alt>F2"] },
    GnomeKey {
        schema: SHELL,
        key: "toggle-message-tray",
        name: "the notification list",
        defaults: &["<Super>v", "<Super>m"],
    },
    GnomeKey { schema: SHELL, key: "toggle-application-view", name: "Show all apps", defaults: &["<Super>a"] },
    GnomeKey { schema: MEDIA, key: "terminal", name: "Launch terminal", defaults: &["<Primary><Alt>t"] },
    GnomeKey { schema: MEDIA, key: "screensaver", name: "Lock screen", defaults: &["<Super>l"] },
];

const KDE: &[(&str, &str)] = &[
    ("<Alt>space", "KRunner"),
    ("<Alt>F2", "KRunner"),
    ("<Alt>Tab", "Walk through windows"),
    ("<Alt>F3", "the window menu"),
    ("<Alt>F4", "Close window"),
    ("<Super>l", "Lock session"),
    ("<Super>v", "Clipboard history"),
    ("<Primary><Alt>t", "Launch Konsole"),
];

/// Xfce, Cinnamon, and MATE share these defaults.
const CLASSIC: &[(&str, &str)] = &[
    ("<Alt>space", "the window menu"),
    ("<Alt>Tab", "Switch windows"),
    ("<Alt>F2", "the Run dialog"),
    ("<Alt>F4", "Close window"),
    ("<Primary><Alt>t", "Launch terminal"),
    ("<Primary><Alt>l", "Lock screen"),
];

/// Desktop shortcuts and their names.
type Table = Vec<(Hotkey, &'static str)>;

/// The desktop's shortcuts, read at most once per [`CACHE_TTL`].
pub struct Conflicts {
    desktop: Desktop,
    cache: Mutex<Option<(Instant, Table)>>,
}

impl Conflicts {
    pub fn new(desktop: Desktop) -> Self {
        Self { desktop, cache: Mutex::new(None) }
    }

    pub fn conflicts(&self, hotkey: &Hotkey) -> Vec<HotkeyConflict> {
        let mut cache = self.cache.lock();
        if cache.as_ref().is_none_or(|(t, _)| t.elapsed() > CACHE_TTL) {
            *cache = Some((Instant::now(), desktop_shortcuts(&self.desktop)));
        }
        let Some((_, table)) = cache.as_ref() else { return Vec::new() };
        let mut out: Vec<HotkeyConflict> = Vec::new();
        for (hk, name) in table {
            let source = ConflictSource::System { name: (*name).to_string(), enabled: true };
            if hk == hotkey && !out.iter().any(|c| c.source == source) {
                out.push(HotkeyConflict { hotkey: *hotkey, source });
            }
        }
        out
    }
}

fn desktop_shortcuts(desktop: &Desktop) -> Table {
    let table: Vec<(&str, &'static str)> = match desktop {
        Desktop::Gnome => return gnome_shortcuts(read_gsettings()),
        Desktop::Kde => KDE.to_vec(),
        Desktop::Other(name) if is_classic(name) => CLASSIC.to_vec(),
        _ => Vec::new(),
    };
    table.into_iter().filter_map(|(accel, name)| parse_accelerator(accel).map(|hk| (hk, name))).collect()
}

fn is_classic(xdg_current_desktop: &str) -> bool {
    xdg_current_desktop
        .split(':')
        .map(str::to_ascii_lowercase)
        .any(|d| matches!(d.as_str(), "xfce" | "x-cinnamon" | "cinnamon" | "mate"))
}

/// The GNOME shortcuts: the read value of each key, else its default.
fn gnome_shortcuts(values: HashMap<(String, String), Vec<String>>) -> Table {
    let mut out = Vec::new();
    for k in GNOME {
        let accels: Vec<String> = match values.get(&(k.schema.to_string(), k.key.to_string())) {
            Some(v) => v.clone(),
            None => k.defaults.iter().map(|s| s.to_string()).collect(),
        };
        out.extend(accels.iter().filter_map(|a| parse_accelerator(a)).map(|hk| (hk, k.name)));
    }
    out
}

/// Read the three GNOME schemas with `gsettings list-recursively`. Missing
/// schemas or a missing command give no values, so the defaults apply.
fn read_gsettings() -> HashMap<(String, String), Vec<String>> {
    let mut values = HashMap::new();
    for schema in [WM, SHELL, MEDIA] {
        let output = super::gnome::host_gsettings().args(["list-recursively", schema]).output();
        match output {
            Ok(o) if o.status.success() => values.extend(parse_list_recursively(&String::from_utf8_lossy(&o.stdout))),
            Ok(o) => log::debug!("gsettings cannot read {schema}: {}", String::from_utf8_lossy(&o.stderr).trim()),
            Err(e) => {
                log::debug!("cannot run gsettings: {e}");
                break;
            }
        }
    }
    values
}

/// Parse `gsettings list-recursively` output: one `schema key value` per line.
pub fn parse_list_recursively(text: &str) -> HashMap<(String, String), Vec<String>> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let mut parts = line.splitn(3, ' ');
        let (Some(schema), Some(key), Some(value)) = (parts.next(), parts.next(), parts.next()) else { continue };
        if let Some(list) = parse_strv(value) {
            out.insert((schema.to_string(), key.to_string()), list);
        }
    }
    out
}

/// Parse a GVariant string array such as `['<Alt>space', '<Super>v']` or
/// `@as []`. `None` when the value is not a string array.
pub fn parse_strv(value: &str) -> Option<Vec<String>> {
    let value = value.trim();
    let value = value.strip_prefix("@as").map(str::trim_start).unwrap_or(value);
    let inner = value.strip_prefix('[')?.strip_suffix(']')?.trim();
    if inner.is_empty() {
        return Some(Vec::new());
    }
    // Shortcut names contain no quotes or commas, so a plain split is enough.
    inner
        .split(',')
        .map(|item| {
            let item = item.trim();
            item.strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .or_else(|| item.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
                .map(str::to_string)
        })
        .collect()
}

/// Parse a GTK accelerator such as `<Primary><Alt>t`. `None` for keys that a
/// Sayso hotkey cannot hold (for example `Print` or `XF86Keyboard`).
pub fn parse_accelerator(accel: &str) -> Option<Hotkey> {
    let mut modifiers = Modifiers::default();
    let mut rest = accel.trim();
    while let Some(after) = rest.strip_prefix('<') {
        let (name, tail) = after.split_once('>')?;
        match name.to_ascii_lowercase().as_str() {
            "primary" | "control" | "ctrl" | "ctl" => modifiers.control = true,
            "alt" | "mod1" => modifiers.option = true,
            "super" | "mod4" => modifiers.command = true,
            "shift" => modifiers.shift = true,
            _ => return None,
        }
        rest = tail;
    }
    let key = match rest {
        "space" => Key::Space,
        "Escape" => Key::Escape,
        "Return" => Key::Return,
        "Tab" => Key::Tab,
        "BackSpace" => Key::Backspace,
        _ if rest.len() == 1 && rest.chars().all(|c| c.is_ascii_alphabetic()) => {
            Key::Letter(rest.chars().next()?.to_ascii_lowercase())
        }
        _ if rest.len() == 1 && rest.chars().all(|c| c.is_ascii_digit()) => Key::Digit(rest.parse().ok()?),
        _ => {
            let n: u8 = rest.strip_prefix('F')?.parse().ok()?;
            if !(1..=20).contains(&n) {
                return None;
            }
            Key::F(n)
        }
    };
    Some(Hotkey::Chord { modifiers, key })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hk(s: &str) -> Hotkey {
        s.parse().unwrap()
    }

    #[test]
    fn accelerators_parse() {
        assert_eq!(parse_accelerator("<Alt>space"), Some(hk("opt+space")));
        assert_eq!(parse_accelerator("<Primary><Alt>t"), Some(hk("ctrl+opt+t")));
        assert_eq!(parse_accelerator("<Control><Alt>T"), Some(hk("ctrl+opt+t")));
        assert_eq!(parse_accelerator("<Shift><Super>space"), Some(hk("shift+cmd+space")));
        assert_eq!(parse_accelerator("<Super>Tab"), Some(hk("cmd+tab")));
        assert_eq!(parse_accelerator("<Alt>F4"), Some(hk("opt+f4")));
        assert_eq!(parse_accelerator("<Super>1"), Some(hk("cmd+1")));
        assert_eq!(parse_accelerator("XF86Keyboard"), None);
        assert_eq!(parse_accelerator("<Super>Print"), None);
        assert_eq!(parse_accelerator("<Hyper>a"), None);
        assert_eq!(parse_accelerator("<Alt>F40"), None);
        assert_eq!(parse_accelerator("<Alt"), None);
    }

    #[test]
    fn string_arrays_parse() {
        assert_eq!(parse_strv("['<Alt>space']"), Some(vec!["<Alt>space".to_string()]));
        assert_eq!(
            parse_strv("['<Super>space', 'XF86Keyboard']"),
            Some(vec!["<Super>space".to_string(), "XF86Keyboard".to_string()])
        );
        assert_eq!(parse_strv("@as []"), Some(Vec::new()));
        assert_eq!(parse_strv("[]"), Some(Vec::new()));
        assert_eq!(parse_strv("true"), None);
        assert_eq!(parse_strv("'<Alt>space'"), None);
    }

    #[test]
    fn list_recursively_output_parses() {
        let text = "org.gnome.desktop.wm.keybindings activate-window-menu @as []\n\
                    org.gnome.desktop.wm.keybindings switch-input-source ['<Super>space', 'XF86Keyboard']\n\
                    org.gnome.desktop.wm.keybindings move-to-center 'not a list'\n";
        let values = parse_list_recursively(text);
        assert_eq!(values.get(&(WM.into(), "activate-window-menu".into())), Some(&Vec::new()));
        assert_eq!(values.get(&(WM.into(), "switch-input-source".into())).map(Vec::len), Some(2));
        assert!(!values.contains_key(&(WM.into(), "move-to-center".into())));
    }

    #[test]
    fn gnome_uses_read_values_and_defaults() {
        // The user turned the window menu off; the rest are defaults.
        let mut values = HashMap::new();
        values.insert((WM.to_string(), "activate-window-menu".to_string()), Vec::new());
        let table = gnome_shortcuts(values);
        assert!(!table.iter().any(|(h, _)| *h == hk("opt+space")));
        assert!(table.contains(&(hk("cmd+space"), "Switch input source")));
        assert!(table.contains(&(hk("ctrl+opt+t"), "Launch terminal")));
        assert!(table.contains(&(hk("cmd+v"), "the notification list")));
        // Without values every default applies.
        assert!(gnome_shortcuts(HashMap::new()).contains(&(hk("opt+space"), "the window menu")));
    }

    #[test]
    fn tables_parse_completely() {
        for (accel, _) in KDE.iter().chain(CLASSIC) {
            assert!(parse_accelerator(accel).is_some(), "{accel}");
        }
        for k in GNOME {
            for accel in k.defaults {
                assert!(parse_accelerator(accel).is_some(), "{accel}");
            }
        }
    }

    #[test]
    fn conflicts_follow_the_desktop() {
        let kde = Conflicts::new(Desktop::Kde);
        let found = kde.conflicts(&hk("opt+space"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source, ConflictSource::System { name: "KRunner".into(), enabled: true });
        assert!(kde.conflicts(&hk("ctrl+opt+space")).is_empty());
        let xfce = Conflicts::new(Desktop::Other("XFCE".into()));
        assert_eq!(xfce.conflicts(&hk("ctrl+opt+l")).len(), 1);
        assert!(Conflicts::new(Desktop::Sway).conflicts(&hk("opt+space")).is_empty());
        assert!(Conflicts::new(Desktop::Other("X-Cinnamon".into())).conflicts(&hk("opt+tab")).len() == 1);
    }
}
