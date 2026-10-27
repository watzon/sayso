//! Start Sayso when it does not run (`docs/extensions.md`, "Lifecycle").
//!
//! - macOS: `open -g -b dev.sayso.Sayso`. LaunchServices finds the app
//!   wherever the user put it, and `-g` keeps the extension in front.
//! - Windows: `Sayso.exe` where the installer puts it: in
//!   `%LOCALAPPDATA%\Programs\Sayso` (for the user, the default) or in
//!   `%ProgramFiles%\Sayso` (for all users).
//! - Linux: in a Flatpak of Sayso, `/app/bin/sayso`. Else the folders of the
//!   tarball (`~/.local/lib/sayso`, `/usr/local/lib/sayso`) and of the
//!   packages (`/usr/lib/sayso`), then the Flatpak of Sayso
//!   (`flatpak run dev.sayso.Sayso`), then `sayso` on the `PATH` (an AppImage
//!   or Nix).
//!
//! Sayso starts as a process of its own, in its own process group, without
//! the input and output of the extension.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The bundle id of Sayso on macOS, and its app id on Linux.
pub const APP_ID: &str = "dev.sayso.Sayso";

/// One way to start Sayso: a program and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl Launcher {
    fn new(program: impl Into<PathBuf>, args: &[&str]) -> Launcher {
        Launcher { program: program.into(), args: args.iter().map(|arg| arg.to_string()).collect() }
    }
}

/// What the rules read from the system. A trait, so tests do not touch the
/// real system.
pub(crate) trait StartEnv {
    fn var(&self, key: &str) -> Option<OsString>;
    fn exists(&self, path: &Path) -> bool;
}

struct SystemEnv;

impl StartEnv for SystemEnv {
    fn var(&self, key: &str) -> Option<OsString> {
        std::env::var_os(key).filter(|value| !value.is_empty())
    }
    fn exists(&self, path: &Path) -> bool {
        path.is_file() || path.is_dir()
    }
}

/// The ways to start Sayso on this system, in the order to try them. Empty
/// when Sayso is not installed in a place that the rules know.
pub fn launchers() -> Vec<Launcher> {
    launchers_for(std::env::consts::OS, &SystemEnv)
}

pub(crate) fn launchers_for(os: &str, env: &dyn StartEnv) -> Vec<Launcher> {
    let found = |path: PathBuf| env.exists(&path).then(|| Launcher::new(path, &[]));
    match os {
        "macos" => vec![Launcher::new("/usr/bin/open", &["-g", "-b", APP_ID])],
        "windows" => {
            let mut folders = Vec::new();
            if let Some(local) = env.var("LOCALAPPDATA") {
                folders.push(PathBuf::from(local).join("Programs"));
            }
            folders.extend(env.var("ProgramFiles").map(PathBuf::from));
            folders.into_iter().filter_map(|folder| found(folder.join("Sayso").join("Sayso.exe"))).collect()
        }
        _ => {
            if env.var("FLATPAK_ID").is_some_and(|id| id == APP_ID) {
                return found(PathBuf::from("/app/bin/sayso")).into_iter().collect();
            }
            let mut folders = Vec::new();
            if let Some(home) = env.var("HOME") {
                folders.push(PathBuf::from(home).join(".local/lib/sayso"));
            }
            folders.extend([PathBuf::from("/usr/local/lib/sayso"), PathBuf::from("/usr/lib/sayso")]);
            let mut launchers: Vec<Launcher> = folders.into_iter().filter_map(|folder| found(folder.join("sayso"))).collect();
            let mut flatpaks = vec![PathBuf::from("/var/lib/flatpak/app").join(APP_ID)];
            if let Some(data) = env.var("XDG_DATA_HOME").map(PathBuf::from).or_else(|| env.var("HOME").map(|home| PathBuf::from(home).join(".local/share"))) {
                flatpaks.insert(0, data.join("flatpak/app").join(APP_ID));
            }
            if flatpaks.iter().any(|path| env.exists(path)) {
                launchers.push(Launcher::new("flatpak", &["run", APP_ID]));
            }
            if let Some(path) = env.var("PATH") {
                launchers.extend(std::env::split_paths(&path).find_map(|dir| found(dir.join("sayso"))));
            }
            launchers
        }
    }
}

/// Start Sayso with the first launcher that works, and do not wait for it.
pub fn start_sayso() -> io::Result<()> {
    let launchers = launchers();
    if launchers.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "Sayso is not installed in a place that the SDK knows. Start Sayso, then try again"));
    }
    let mut last = None;
    for launcher in launchers {
        match spawn(&launcher) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(io::Error::new(e.kind(), format!("cannot start {}: {e}", launcher.program.display()))),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("cannot start Sayso")))
}

fn spawn(launcher: &Launcher) -> io::Result<()> {
    let mut command = Command::new(&launcher.program);
    command.args(&launcher.args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // The socket of this extension is not the business of Sayso.
    command.env_remove(sayso_protocol::socket::SOCKET_ENV);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let mut child = command.spawn()?;
    if cfg!(target_os = "macos") {
        // `open` ends when LaunchServices has started the app, or failed.
        let status = child.wait()?;
        return if status.success() { Ok(()) } else { Err(io::Error::other(format!("open ended with {status}; is Sayso installed?"))) };
    }
    // Wait for Sayso in the background, so its process does not stay as a
    // zombie when it ends before the extension.
    let _ = std::thread::Builder::new().name("sayso-extension-start".into()).spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[derive(Default)]
    struct Fake {
        vars: HashMap<&'static str, &'static str>,
        files: HashSet<PathBuf>,
    }

    impl StartEnv for Fake {
        fn var(&self, key: &str) -> Option<OsString> {
            self.vars.get(key).map(OsString::from)
        }
        fn exists(&self, path: &Path) -> bool {
            self.files.contains(path)
        }
    }

    fn programs(launchers: Vec<Launcher>) -> Vec<String> {
        launchers.into_iter().map(|l| format!("{} {}", l.program.display(), l.args.join(" ")).trim().to_string()).collect()
    }

    #[test]
    fn macos_opens_the_bundle_id() {
        assert_eq!(programs(launchers_for("macos", &Fake::default())), vec!["/usr/bin/open -g -b dev.sayso.Sayso"]);
    }

    #[test]
    fn windows_uses_the_folders_of_the_installer() {
        let mut env = Fake::default();
        env.vars.insert("LOCALAPPDATA", r"C:\Users\me\AppData\Local");
        env.vars.insert("ProgramFiles", r"C:\Program Files");
        assert!(launchers_for("windows", &env).is_empty(), "not installed");
        let user = PathBuf::from(r"C:\Users\me\AppData\Local").join("Programs").join("Sayso").join("Sayso.exe");
        let all = PathBuf::from(r"C:\Program Files").join("Sayso").join("Sayso.exe");
        env.files.extend([user.clone(), all.clone()]);
        let found: Vec<PathBuf> = launchers_for("windows", &env).into_iter().map(|l| l.program).collect();
        assert_eq!(found, vec![user, all], "the install of the user first");
    }

    // The rule splits PATH and joins paths with the separators of the system it
    // runs on, and it only runs on Linux, so Windows cannot test it.
    #[test]
    #[cfg(unix)]
    fn linux_finds_the_tarball_the_packages_the_flatpak_and_the_path() {
        let mut env = Fake::default();
        env.vars.insert("HOME", "/home/me");
        env.vars.insert("PATH", "/usr/bin:/home/me/bin");
        assert!(launchers_for("linux", &env).is_empty(), "not installed");
        env.files.extend(["/home/me/.local/lib/sayso/sayso", "/usr/lib/sayso/sayso", "/home/me/.local/share/flatpak/app/dev.sayso.Sayso", "/home/me/bin/sayso"].map(PathBuf::from));
        assert_eq!(
            programs(launchers_for("linux", &env)),
            vec!["/home/me/.local/lib/sayso/sayso", "/usr/lib/sayso/sayso", "flatpak run dev.sayso.Sayso", "/home/me/bin/sayso"]
        );
        // In the Flatpak of Sayso, only the Sayso of the sandbox.
        env.vars.insert("FLATPAK_ID", "dev.sayso.Sayso");
        env.files.insert(PathBuf::from("/app/bin/sayso"));
        assert_eq!(programs(launchers_for("linux", &env)), vec!["/app/bin/sayso"]);
    }
}
