//! [`LoginItem`] through an XDG autostart entry
//! (`~/.config/autostart/dev.sayso.Sayso.desktop`).

use sayso_platform::{LoginItem, LoginItemState, PlatformError, Result};
use std::path::PathBuf;

pub struct LinuxLoginItem;

const FILE_NAME: &str = "dev.sayso.Sayso.desktop";

/// The autostart file of the desktop. In a Flatpak, `XDG_CONFIG_HOME` is a
/// folder of the sandbox, and `HOST_XDG_CONFIG_HOME` has the value of the
/// host when the host sets one.
fn autostart_file() -> Option<PathBuf> {
    let variable = if sayso_core::flatpak::app_id().is_some() { "HOST_XDG_CONFIG_HOME" } else { "XDG_CONFIG_HOME" };
    let config = std::env::var_os(variable)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(config.join("autostart").join(FILE_NAME))
}

/// The autostart entry for the command `words` (see [`crate::launch`]). A
/// word with spaces is quoted, as the desktop entry format asks.
pub fn entry(words: &[String]) -> String {
    let quoted: Vec<String> = words.iter().map(|w| if w.contains(' ') { format!("\"{w}\"") } else { w.clone() }).collect();
    let exec = quoted.join(" ");
    format!(
        "[Desktop Entry]\nType=Application\nName=Sayso\nComment=Voice dictation\nExec={exec}\nIcon=dev.sayso.Sayso\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    )
}

/// An entry is on unless it says `Hidden=true` or turns GNOME autostart off.
pub fn is_enabled(text: &str) -> bool {
    !text.lines().map(str::trim).any(|l| l == "Hidden=true" || l == "X-GNOME-Autostart-enabled=false")
}

impl LoginItem for LinuxLoginItem {
    fn state(&self) -> LoginItemState {
        match autostart_file().and_then(|f| std::fs::read_to_string(f).ok()) {
            Some(text) if is_enabled(&text) => LoginItemState::Enabled,
            _ => LoginItemState::Disabled,
        }
    }

    fn set_enabled(&self, enabled: bool) -> Result<LoginItemState> {
        let file = autostart_file().ok_or_else(|| PlatformError::Failed("no home folder".into()))?;
        let fail = |e: std::io::Error| PlatformError::Failed(format!("{}: {e}", file.display()));
        if enabled {
            let command = crate::launch::command().ok_or_else(|| PlatformError::Failed("cannot find the Sayso program".into()))?;
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir).map_err(fail)?;
            }
            std::fs::write(&file, entry(&command)).map_err(fail)?;
        } else {
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(fail(e)),
            }
        }
        Ok(self.state())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_quotes_paths_with_spaces() {
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(entry(&words(&["/opt/sayso/sayso"])).contains("\nExec=/opt/sayso/sayso\n"));
        assert!(entry(&words(&["/home/a b/Sayso.AppImage"])).contains("\nExec=\"/home/a b/Sayso.AppImage\"\n"));
        // Each word is one argument: the quotes do not go around the whole command.
        assert!(entry(&words(&["flatpak", "run", "dev.sayso.Sayso"])).contains("\nExec=flatpak run dev.sayso.Sayso\n"));
    }

    #[test]
    fn hidden_or_disabled_entries_are_off() {
        assert!(is_enabled(&entry(&["/x".to_string()])));
        assert!(!is_enabled("[Desktop Entry]\nHidden=true\n"));
        assert!(!is_enabled("[Desktop Entry]\nX-GNOME-Autostart-enabled=false\n"));
    }
}
