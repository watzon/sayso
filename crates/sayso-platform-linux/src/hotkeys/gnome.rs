//! Chords as GNOME custom shortcuts, for GNOME before version 48.
//!
//! Older GNOME has no GlobalShortcuts portal. Without the `input` group,
//! Sayso then has no way to see keys in other apps. GNOME itself can run a
//! command for a key, so Sayso adds one custom shortcut for each chord, with
//! the command `sayso --toggle` (or `--paste-last`, `--cycle-style`). The
//! command reaches the running Sayso through its socket (see `crate::ipc`).
//!
//! The shortcuts show in Settings › Keyboard › Custom Shortcuts, named
//! "Sayso: …". Sayso owns only the entries whose path contains `/sayso-`.
//! A custom shortcut has no key-up, so push-to-talk cannot work this way.

use sayso_core::hotkey::{Hotkey, Key};
use std::process::Command;

const LIST_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
const LIST_KEY: &str = "custom-keybindings";
const ENTRY_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
const BASE: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/";

/// One shortcut that Sayso adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    /// `toggle`, `paste-last`, or `cycle-style`: also the command flag.
    pub id: &'static str,
    pub name: &'static str,
    /// A GNOME accelerator, for example `<Control><Alt>space`.
    pub binding: String,
}

/// True when this GNOME has custom shortcuts (the `gsettings` tool and the schema exist).
pub fn available() -> bool {
    Command::new("gsettings")
        .args(["list-keys", LIST_SCHEMA])
        .output()
        .is_ok_and(|out| out.status.success() && String::from_utf8_lossy(&out.stdout).lines().any(|l| l == LIST_KEY))
}

/// Make Sayso's custom shortcuts exactly `shortcuts`: add, change, or remove
/// entries. Entries of other apps stay as they are.
pub fn apply(shortcuts: &[Shortcut]) -> Result<(), String> {
    let program = program()?;
    let current = parse_list(&gsettings(&["get", LIST_SCHEMA, LIST_KEY])?);
    let ours: Vec<String> = shortcuts.iter().map(|s| path(s.id)).collect();
    for old in current.iter().filter(|p| is_ours(p) && !ours.contains(p)) {
        for key in ["name", "command", "binding"] {
            let _ = gsettings(&["reset", &format!("{ENTRY_SCHEMA}:{old}"), key]);
        }
    }
    for shortcut in shortcuts {
        let schema = format!("{ENTRY_SCHEMA}:{}", path(shortcut.id));
        gsettings(&["set", &schema, "name", &gvariant_string(&format!("Sayso: {}", shortcut.name))])?;
        gsettings(&["set", &schema, "command", &gvariant_string(&command_line(&program, shortcut.id))])?;
        gsettings(&["set", &schema, "binding", &gvariant_string(&shortcut.binding)])?;
    }
    let list = merge_list(&current, &ours);
    if list != current {
        gsettings(&["set", LIST_SCHEMA, LIST_KEY, &format_list(&list)])?;
    }
    Ok(())
}

/// Remove Sayso's custom shortcuts, when another backend takes the chords.
/// Does nothing when there are none.
pub fn clear() {
    let Ok(text) = gsettings(&["get", LIST_SCHEMA, LIST_KEY]) else { return };
    if parse_list(&text).iter().any(|p| is_ours(p))
        && let Err(e) = apply(&[])
    {
        log::warn!("cannot remove the Sayso shortcuts from GNOME: {e}");
    }
}

fn gsettings(args: &[&str]) -> Result<String, String> {
    let out = Command::new("gsettings").args(args).output().map_err(|e| format!("cannot run gsettings: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!("gsettings {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// The program GNOME starts: the AppImage when Sayso runs from one, else this executable.
fn program() -> Result<String, String> {
    std::env::var_os("APPIMAGE")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| "cannot find the Sayso program".into())
}

/// The GNOME accelerator of a chord: `<Control><Alt>space`. None for a
/// modifier held alone, which GNOME cannot bind.
pub fn accelerator(hotkey: &Hotkey) -> Option<String> {
    let Hotkey::Chord { modifiers, key } = hotkey else { return None };
    let mut out = String::new();
    for (on, name) in [
        (modifiers.control, "<Control>"),
        (modifiers.option, "<Alt>"),
        (modifiers.shift, "<Shift>"),
        (modifiers.command, "<Super>"),
    ] {
        if on {
            out.push_str(name);
        }
    }
    out.push_str(&match key {
        Key::Space => "space".to_string(),
        Key::Escape => "Escape".to_string(),
        Key::Return => "Return".to_string(),
        Key::Tab => "Tab".to_string(),
        Key::Backspace => "BackSpace".to_string(),
        Key::Letter(c) => c.to_ascii_lowercase().to_string(),
        Key::Digit(d) => d.to_string(),
        Key::F(n) => format!("F{n}"),
    });
    Some(out)
}

pub fn path(id: &str) -> String {
    format!("{BASE}sayso-{id}/")
}

fn is_ours(path: &str) -> bool {
    path.starts_with(BASE) && path[BASE.len()..].starts_with("sayso-")
}

/// The command for a shortcut. GNOME splits it like a shell, so a path with
/// spaces gets single quotes.
pub fn command_line(program: &str, id: &str) -> String {
    let program = if program.contains([' ', '\'', '"']) { format!("'{}'", program.replace('\'', "'\\''")) } else { program.to_string() };
    format!("{program} --{id}")
}

/// A string in GVariant text form: `'it\'s'`.
pub fn gvariant_string(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// The paths in `['/a/', '/b/']` or `@as []`.
pub fn parse_list(text: &str) -> Vec<String> {
    text.split('\'').skip(1).step_by(2).map(str::to_string).collect()
}

pub fn format_list(paths: &[String]) -> String {
    if paths.is_empty() {
        return "@as []".into();
    }
    let items: Vec<String> = paths.iter().map(|p| gvariant_string(p)).collect();
    format!("[{}]", items.join(", "))
}

/// Other apps' entries in their order, then ours.
pub fn merge_list(current: &[String], ours: &[String]) -> Vec<String> {
    current.iter().filter(|p| !is_ours(p)).cloned().chain(ours.iter().cloned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_parse_and_format() {
        assert_eq!(parse_list("@as []"), Vec::<String>::new());
        let list = parse_list("['/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/custom0/', '/x/']");
        assert_eq!(list, vec![format!("{BASE}custom0/"), "/x/".to_string()]);
        assert_eq!(format_list(&list), format!("['{BASE}custom0/', '/x/']"));
        assert_eq!(format_list(&[]), "@as []");
    }

    #[test]
    fn merge_keeps_other_entries_and_replaces_ours() {
        let current = vec![format!("{BASE}custom0/"), path("toggle"), path("cycle-style")];
        let merged = merge_list(&current, &[path("toggle"), path("paste-last")]);
        assert_eq!(merged, vec![format!("{BASE}custom0/"), path("toggle"), path("paste-last")]);
        assert!(is_ours(&path("toggle")) && !is_ours(&format!("{BASE}custom0/")));
    }

    #[test]
    fn accelerators_use_gnome_names() {
        let hk = |s: &str| s.parse::<Hotkey>().unwrap();
        assert_eq!(accelerator(&hk("ctrl+alt+space")).as_deref(), Some("<Control><Alt>space"));
        assert_eq!(accelerator(&hk("super+shift+v")).as_deref(), Some("<Shift><Super>v"));
        assert_eq!(accelerator(&hk("ctrl+f5")).as_deref(), Some("<Control>F5"));
        assert_eq!(accelerator(&hk("right_control")), None);
    }

    #[test]
    fn commands_and_strings_are_quoted() {
        assert_eq!(command_line("/home/a/.local/lib/sayso/sayso", "toggle"), "/home/a/.local/lib/sayso/sayso --toggle");
        assert_eq!(command_line("/home/a b/Sayso.AppImage", "paste-last"), "'/home/a b/Sayso.AppImage' --paste-last");
        assert_eq!(gvariant_string("it's \\ ok"), "'it\\'s \\\\ ok'");
    }
}
