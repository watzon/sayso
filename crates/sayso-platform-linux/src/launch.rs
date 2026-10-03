//! The command that starts Sayso again, for the autostart entry and for the
//! GNOME custom shortcuts. It depends on how Sayso is installed.

/// The words of the command: `flatpak run <id>` in a Flatpak, the AppImage
/// when Sayso runs from one, else this executable. None when the executable
/// cannot be found.
pub fn command() -> Option<Vec<String>> {
    command_from(
        sayso_core::flatpak::app_id(),
        std::env::var("APPIMAGE").ok(),
        std::env::current_exe().ok().map(|p| p.to_string_lossy().into_owned()),
    )
}

fn command_from(flatpak_id: Option<String>, appimage: Option<String>, exe: Option<String>) -> Option<Vec<String>> {
    if let Some(id) = flatpak_id {
        return Some(sayso_core::flatpak::run_command(&id));
    }
    appimage.filter(|path| !path.is_empty()).or(exe).map(|program| vec![program])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_install_kind_sets_the_command() {
        let exe = || Some("/usr/lib/sayso/sayso".to_string());
        assert_eq!(command_from(None, None, exe()).unwrap(), ["/usr/lib/sayso/sayso"]);
        assert_eq!(command_from(None, Some(String::new()), exe()).unwrap(), ["/usr/lib/sayso/sayso"]);
        assert_eq!(command_from(None, Some("/home/a/Sayso.AppImage".into()), exe()).unwrap(), ["/home/a/Sayso.AppImage"]);
        // In a Flatpak the executable is a path in the sandbox, which the desktop cannot start.
        assert_eq!(command_from(Some("dev.sayso.Sayso".into()), None, Some("/app/lib/sayso/sayso".into())).unwrap(), ["flatpak", "run", "dev.sayso.Sayso"]);
        assert_eq!(command_from(None, None, None), None);
    }
}
