//! Sayso in a Flatpak sandbox (Linux).
//!
//! The sandbox has its own file system and its own programs. The tools that
//! Sayso starts for the user (`gsettings`, and the `claude` and `codex`
//! command line tools) are on the host, so they run through
//! `flatpak-spawn --host`. The Flatpak has the permission for that
//! (`--talk-name=org.freedesktop.Flatpak`).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The application id when Sayso runs in a Flatpak, for example `dev.sayso.Sayso`.
pub fn app_id() -> Option<String> {
    app_id_from(std::env::var("FLATPAK_ID").ok())
}

fn app_id_from(value: Option<String>) -> Option<String> {
    value.filter(|id| !id.is_empty())
}

/// A command for a program of the user's system, with `envs` added to its
/// environment and `cwd` as its folder. In a Flatpak the program runs on the
/// host. When Sayso stops the command, the program stops too.
pub fn host_command(program: &Path, envs: &[(&str, &str)], cwd: Option<&Path>) -> Command {
    command_for(app_id().is_some(), program, envs, cwd)
}

fn command_for(sandboxed: bool, program: &Path, envs: &[(&str, &str)], cwd: Option<&Path>) -> Command {
    if !sandboxed {
        let mut command = Command::new(program);
        command.envs(envs.iter().copied());
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        return command;
    }
    // --watch-bus stops the program on the host when flatpak-spawn goes away.
    let mut command = Command::new("flatpak-spawn");
    command.args(["--host", "--watch-bus"]);
    for (name, value) in envs {
        command.arg(format!("--env={name}={value}"));
    }
    if let Some(cwd) = cwd {
        let mut arg = std::ffi::OsString::from("--directory=");
        arg.push(cwd);
        command.arg(arg);
    }
    command.arg(program);
    command
}

/// The folder for temporary files that a host program must read or write.
/// None outside a Flatpak, where the system folder is right. The sandbox has
/// its own `/tmp`, which the host cannot see. The cache folder of the app has
/// the same path on both sides.
pub fn shared_temp_root() -> Option<PathBuf> {
    app_id()?;
    std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The words of the command that starts this app from outside the sandbox.
pub fn run_command(app_id: &str) -> Vec<String> {
    vec!["flatpak".into(), "run".into(), app_id.into()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn words(command: &Command) -> Vec<String> {
        std::iter::once(command.get_program()).chain(command.get_args()).map(|w| w.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn an_empty_id_is_no_flatpak() {
        assert_eq!(app_id_from(None), None);
        assert_eq!(app_id_from(Some(String::new())), None);
        assert_eq!(app_id_from(Some("dev.sayso.Sayso".into())).as_deref(), Some("dev.sayso.Sayso"));
    }

    #[test]
    fn outside_a_flatpak_the_program_runs_directly() {
        let command = command_for(false, Path::new("/usr/bin/codex"), &[("A", "1")], Some(Path::new("/tmp/x")));
        assert_eq!(words(&command), ["/usr/bin/codex"]);
        assert_eq!(command.get_current_dir(), Some(Path::new("/tmp/x")));
        assert!(command.get_envs().any(|(k, v)| k == "A" && v == Some(OsStr::new("1"))));
    }

    #[test]
    fn in_a_flatpak_the_program_runs_on_the_host() {
        let command = command_for(true, Path::new("/home/a/.local/bin/claude"), &[("A", "1")], Some(Path::new("/c/t")));
        assert_eq!(
            words(&command),
            ["flatpak-spawn", "--host", "--watch-bus", "--env=A=1", "--directory=/c/t", "/home/a/.local/bin/claude"]
        );
        // The folder and the variable are for the host program, not for flatpak-spawn.
        assert_eq!(command.get_current_dir(), None);
        assert_eq!(command.get_envs().count(), 0);
    }
}
