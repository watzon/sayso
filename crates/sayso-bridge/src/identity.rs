//! Who a process is on Linux (`docs/sdk.md`, "Identity").
//!
//! A process in the PID namespace of this process has the path of its
//! executable as its identity. A process in another PID namespace has an
//! identity only when it is in a Flatpak sandbox: its app id, from
//! `.flatpak-info` at the root of the sandbox. The way to read that file is the
//! way of `xdg-desktop-portal`. All reads go through one handle of
//! `/proc/<pid>`, so a process that ends gives an error, and another process
//! with the same pid is never read. In a real sandbox, `seccomp` refuses each
//! call that can change the file: `mount`, `unshare`, `pivot_root`, and a clone
//! with a new user namespace (checked on Flatpak 1.14.6, also with
//! `--allow=devel`).
//!
//! A program outside a sandbox can make a private namespace with a false file.
//! So the app id must also match the folder where Flatpak installs that app,
//! and the process must run a file of that folder or of its runtime. The host
//! finds these folders itself. It takes no folder from the false file that it
//! does not check.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

/// What the system says about a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Who {
    /// The canonical path of the executable.
    Path(PathBuf),
    /// A Flatpak sandbox with this app id.
    Flatpak(String),
}

/// A Linux link to a file that was deleted or replaced after the start.
pub fn deleted_link(link: &Path) -> bool {
    link.as_os_str().as_encoded_bytes().ends_with(b" (deleted)")
}

/// The identity of the process `pid`.
pub fn identify(pid: libc::pid_t) -> Result<Who, String> {
    // Who has a capability in a PID namespace can write the pid of another
    // process of it in `SCM_CREDENTIALS`. In the namespace of the host, only the true pid passes.
    let namespace = |pid: &str| std::fs::read_link(format!("/proc/{pid}/ns/pid")).map_err(|e| format!("no PID namespace of {pid}: {e}"));
    if namespace(&pid.to_string())? == namespace("self")? { in_this_namespace(pid) } else { in_a_sandbox(pid) }
}

fn in_this_namespace(pid: libc::pid_t) -> Result<Who, String> {
    let exe = format!("/proc/{pid}/exe");
    let link = std::fs::read_link(&exe).map_err(|e| format!("no {exe}: {e}"))?;
    if deleted_link(&link) {
        return Err(format!("{} was deleted or replaced after it started", link.display()));
    }
    // A private mount namespace can show another file at the path of an approved app.
    if file_of(exe.as_ref())? != file_of(&link)? {
        return Err(format!("process {pid} does not run the file at {}", link.display()));
    }
    Ok(Who::Path(link))
}

fn file_of(path: &Path) -> Result<(u64, u64), String> {
    std::fs::metadata(path).map(|meta| (meta.dev(), meta.ino())).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

fn in_a_sandbox(pid: libc::pid_t) -> Result<Who, String> {
    let process = open_process(pid).map_err(|e| format!("cannot open process {pid}: {e}"))?;
    let at = |name: &str| PathBuf::from(format!("/proc/self/fd/{}/{name}", std::os::fd::AsRawFd::as_raw_fd(&process)));
    let status = std::fs::read_to_string(at("status")).map_err(|e| format!("cannot read the status of process {pid}: {e}"))?;
    check_status(&status).map_err(|e| format!("process {pid} is in another PID namespace, and {e}"))?;
    let info = read_info(&at("root/.flatpak-info")).map_err(|e| format!("process {pid} is in another PID namespace, and it has no Flatpak info: {e}"))?;
    let info = FlatpakInfo::parse(&info).map_err(|e| format!("process {pid}: {e}"))?;
    let roots = install_roots(&Env);
    let app = deployed(&roots, "app", Some(&info.app_id), &info.app_path).map_err(|e| format!("process {pid} says that it is {}, and {e}", info.app_id))?;
    let runtime = deployed(&roots, "runtime", None, &info.runtime_path).map_err(|e| format!("process {pid} says that it is {}, and {e}", info.app_id))?;
    if file_of(&at("root/app"))? != file_of(&app)? {
        return Err(format!("process {pid} says that it is {}, and its /app is another folder", info.app_id));
    }
    let link = std::fs::read_link(at("exe")).map_err(|e| format!("no executable of process {pid}: {e}"))?;
    if deleted_link(&link) {
        return Err(format!("{} was deleted or replaced after it started", link.display()));
    }
    let on_host = file_on_host(&link, &app, &runtime).ok_or_else(|| format!("process {pid} says that it is {}, and it runs {}, which is not a file of the app or its runtime", info.app_id, link.display()))?;
    if file_of(&at("exe"))? != file_of(&on_host)? {
        return Err(format!("process {pid} says that it is {}, and it does not run the file at {}", info.app_id, on_host.display()));
    }
    Ok(Who::Flatpak(info.app_id))
}

/// A handle of `/proc/<pid>`. It stays with this one process.
pub(crate) fn open_process(pid: libc::pid_t) -> io::Result<File> {
    std::fs::OpenOptions::new().read(true).custom_flags(libc::O_PATH | libc::O_DIRECTORY).open(format!("/proc/{pid}"))
}

/// The state of each process in a Flatpak sandbox: one PID namespace below
/// this one, no capability, and a `seccomp` filter with no way to get
/// privileges. A deeper namespace is not a sandbox of `flatpak run`, and it is
/// where a parent with a capability can name a child in `SCM_CREDENTIALS`.
fn check_status(status: &str) -> Result<(), String> {
    let field = |name: &str| status.lines().find_map(|line| line.strip_prefix(name)?.strip_prefix(':')).map(str::trim);
    let levels = field("NSpid").map_or(0, |pids| pids.split_whitespace().count());
    if levels != 2 {
        return Err(format!("it is {} PID namespaces below this one", levels.saturating_sub(1)));
    }
    let no_capability = |name: &str| field(name).is_some_and(|mask| !mask.is_empty() && mask.bytes().all(|byte| byte == b'0'));
    if !no_capability("CapEff") || !no_capability("CapBnd") {
        return Err("it has capabilities".into());
    }
    if field("NoNewPrivs") != Some("1") || field("Seccomp") != Some("2") {
        return Err("it has no seccomp filter".into());
    }
    Ok(())
}

/// The longest `.flatpak-info` that the host reads.
const MAX_INFO: u64 = 1024 * 1024;

fn read_info(path: &Path) -> io::Result<String> {
    // No link, and no pipe that blocks the host.
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY).open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("it is not a regular file"));
    }
    let mut text = String::new();
    file.take(MAX_INFO).read_to_string(&mut text)?;
    Ok(text)
}

/// The parts of `.flatpak-info` that the host uses.
#[derive(Debug, PartialEq, Eq)]
pub struct FlatpakInfo {
    pub app_id: String,
    pub app_path: PathBuf,
    pub runtime_path: PathBuf,
}

impl FlatpakInfo {
    pub fn parse(text: &str) -> Result<FlatpakInfo, String> {
        let mut values: HashMap<(&str, &str), &str> = HashMap::new();
        let mut group = "";
        for line in text.lines() {
            if let Some(name) = line.strip_prefix('[').and_then(|line| line.strip_suffix(']')) {
                group = name;
            } else if let Some((key, value)) = line.split_once('=') {
                values.insert((group, key), value);
            }
        }
        let value = |group, key| values.get(&(group, key)).copied().ok_or_else(|| format!("the Flatpak info has no {key}"));
        let app_id = value("Application", "name")?;
        if !valid_app_id(app_id) {
            return Err(format!("the Flatpak info has the app id {app_id:?}, which is not valid"));
        }
        // `flatpak run --app-path` runs other files with this app id.
        if values.contains_key(&("Instance", "original-app-path")) {
            return Err(format!("{app_id} runs with another app folder"));
        }
        Ok(FlatpakInfo { app_id: app_id.to_string(), app_path: value("Instance", "app-path")?.into(), runtime_path: value("Instance", "runtime-path")?.into() })
    }
}

/// The rules of Flatpak for an app id: at least three parts, and each part
/// starts with a letter or `_` and has letters, digits, `_`, and `-`.
pub fn valid_app_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    id.len() <= 255
        && parts.len() >= 3
        && parts.iter().all(|part| {
            part.chars().next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_') && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

/// What [`install_roots`] reads from the system. A trait, so tests do not
/// touch the real environment.
pub trait InstallEnv {
    fn var(&self, key: &str) -> Option<String>;
    /// The text of each file in `/etc/flatpak/installations.d`.
    fn installations(&self) -> Vec<String>;
}

struct Env;

impl InstallEnv for Env {
    fn var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|value| !value.is_empty())
    }

    fn installations(&self) -> Vec<String> {
        let Ok(folder) = std::fs::read_dir("/etc/flatpak/installations.d") else { return Vec::new() };
        folder.flatten().filter_map(|entry| std::fs::read_to_string(entry.path()).ok()).collect()
    }
}

/// The folders where Flatpak installs apps for this user and for all users.
pub fn install_roots(env: &dyn InstallEnv) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    roots.extend(env.var("FLATPAK_USER_DIR").map(PathBuf::from));
    roots.extend(env.var("XDG_DATA_HOME").map(|data| PathBuf::from(data).join("flatpak")));
    roots.extend(env.var("HOME").map(|home| PathBuf::from(home).join(".local/share/flatpak")));
    roots.extend(env.var("FLATPAK_SYSTEM_DIR").map(PathBuf::from));
    roots.push(PathBuf::from("/var/lib/flatpak"));
    for text in env.installations() {
        roots.extend(text.lines().filter_map(|line| line.strip_prefix("Path=")).map(PathBuf::from));
    }
    roots
}

/// `path`, when it is the `files` folder of a deployed `kind` (`app` or
/// `runtime`) under one of `roots`: `<root>/<kind>/<id>/<arch>/<branch>/<commit>/files`.
/// With `id`, the folder must have this id.
fn deployed(roots: &[PathBuf], kind: &str, id: Option<&str>, path: &Path) -> Result<PathBuf, String> {
    let canonical = path.canonicalize().map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let canonical_roots: Vec<PathBuf> = roots.iter().filter_map(|root| root.canonicalize().ok()).collect();
    if under_roots(&canonical_roots, kind, id, &canonical) { Ok(canonical) } else { Err(format!("{} is not in a folder where Flatpak installs", path.display())) }
}

fn under_roots(roots: &[PathBuf], kind: &str, id: Option<&str>, path: &Path) -> bool {
    roots.iter().any(|root| {
        let Ok(rest) = path.strip_prefix(root.join(kind)) else { return false };
        let parts: Vec<&std::ffi::OsStr> = rest.components().filter_map(|part| if let Component::Normal(part) = part { Some(part) } else { None }).collect();
        parts.len() == rest.components().count() && parts.len() == 5 && parts[4] == "files" && id.is_none_or(|id| parts[0] == id)
    })
}

/// Where the host sees the file that the sandbox shows at `link`.
fn file_on_host(link: &Path, app: &Path, runtime: &Path) -> Option<PathBuf> {
    if let Ok(rest) = link.strip_prefix("/app") {
        Some(app.join(rest))
    } else {
        link.strip_prefix("/usr").ok().map(|rest| runtime.join(rest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = "[Application]\nname=dev.example.Notes\nruntime=runtime/org.freedesktop.Platform/aarch64/26.08\n\n[Instance]\ninstance-id=1\napp-path=/home/a/.local/share/flatpak/app/dev.example.Notes/aarch64/stable/abc/files\nruntime-path=/var/lib/flatpak/runtime/org.freedesktop.Platform/aarch64/26.08/def/files\n";

    #[test]
    fn a_link_to_a_deleted_file_is_found() {
        assert!(deleted_link(Path::new("/opt/app/bin/app (deleted)")));
        assert!(!deleted_link(Path::new("/opt/app/bin/app")));
    }

    #[test]
    fn the_info_gives_the_id_and_the_folders() {
        let info = FlatpakInfo::parse(INFO).unwrap();
        assert_eq!(info.app_id, "dev.example.Notes");
        assert_eq!(info.app_path, PathBuf::from("/home/a/.local/share/flatpak/app/dev.example.Notes/aarch64/stable/abc/files"));
        assert_eq!(info.runtime_path, PathBuf::from("/var/lib/flatpak/runtime/org.freedesktop.Platform/aarch64/26.08/def/files"));
    }

    #[test]
    fn an_info_without_a_part_or_with_a_bad_id_gives_no_identity() {
        assert!(FlatpakInfo::parse("").is_err());
        assert!(FlatpakInfo::parse("[Runtime]\nname=org.freedesktop.Platform\n").is_err(), "a runtime is no app");
        assert!(FlatpakInfo::parse(&INFO.replace("dev.example.Notes\n", "../../etc/passwd\n")).is_err());
        assert!(FlatpakInfo::parse(&INFO.replace("[Instance]\n", "[Instance]\noriginal-app-path=/x\n")).is_err(), "flatpak run --app-path");
        assert!(FlatpakInfo::parse(&INFO.replace("runtime-path=", "x=")).is_err());
    }

    #[test]
    fn an_app_id_follows_the_rules_of_flatpak() {
        for good in ["dev.example.Notes", "org._7zip.App", "a.b.c-d"] {
            assert!(valid_app_id(good), "{good}");
        }
        for bad in ["", "Notes", "dev.example", "dev..Notes", "dev.example.7x", "dev.example.No tes", "dev/example/Notes", "dev.example.\u{202E}x", &format!("a.b.{}", "c".repeat(255))] {
            assert!(!valid_app_id(bad), "{bad}");
        }
    }

    #[test]
    fn the_status_of_a_real_sandbox_passes() {
        // From a process of `flatpak run` on Flatpak 1.14.6.
        let real = "Name:\tsh\nNSpid:\t3056\t2\nCapEff:\t0000000000000000\nCapBnd:\t0000000000000000\nNoNewPrivs:\t1\nSeccomp:\t2\n";
        assert_eq!(check_status(real), Ok(()));
        for (bad, why) in [
            (real.replace("3056\t2", "3056"), "no namespace below this one"),
            (real.replace("3056\t2", "3056\t7\t2"), "a parent can name this process in its credentials"),
            (real.replace("CapEff:\t0000000000000000", "CapEff:\t000001ffffffffff"), "capabilities"),
            (real.replace("CapBnd:\t0000000000000000", "CapBnd:\t000001ffffffffff"), "it can get capabilities"),
            (real.replace("NoNewPrivs:\t1", "NoNewPrivs:\t0"), "it can get privileges"),
            (real.replace("Seccomp:\t2", "Seccomp:\t0"), "no filter"),
            (real.replace("CapEff:\t0000000000000000\n", ""), "no value"),
        ] {
            assert!(check_status(&bad).is_err(), "{why}");
        }
    }

    struct FakeEnv(&'static [(&'static str, &'static str)], &'static [&'static str]);

    impl InstallEnv for FakeEnv {
        fn var(&self, key: &str) -> Option<String> {
            self.0.iter().find(|(name, _)| *name == key).map(|(_, value)| value.to_string())
        }
        fn installations(&self) -> Vec<String> {
            self.1.iter().map(|text| text.to_string()).collect()
        }
    }

    #[test]
    fn the_roots_are_the_folders_of_flatpak() {
        let env = FakeEnv(&[("HOME", "/home/a"), ("XDG_DATA_HOME", "/d")], &["[Installation \"x\"]\nPath=/mnt/flatpak\n"]);
        let roots = install_roots(&env);
        for root in ["/d/flatpak", "/home/a/.local/share/flatpak", "/var/lib/flatpak", "/mnt/flatpak"] {
            assert!(roots.contains(&PathBuf::from(root)), "{root}");
        }
    }

    #[test]
    fn a_folder_counts_only_where_flatpak_installs_that_id() {
        let roots = [PathBuf::from("/var/lib/flatpak")];
        let good = Path::new("/var/lib/flatpak/app/dev.example.Notes/aarch64/stable/abc/files");
        assert!(under_roots(&roots, "app", Some("dev.example.Notes"), good));
        assert!(!under_roots(&roots, "app", Some("dev.example.Other"), good), "the folder of another app");
        assert!(!under_roots(&roots, "runtime", None, good), "an app is no runtime");
        assert!(under_roots(&roots, "runtime", None, Path::new("/var/lib/flatpak/runtime/org.x.Platform/aarch64/26.08/def/files")));
        for bad in ["/tmp/flatpak/app/dev.example.Notes/aarch64/stable/abc/files", "/var/lib/flatpak/app/dev.example.Notes/aarch64/stable/abc/files/bin", "/var/lib/flatpak/app/dev.example.Notes/files", "/var/lib/flatpak/app/dev.example.Notes/aarch64/stable/abc/other"] {
            assert!(!under_roots(&roots, "app", Some("dev.example.Notes"), Path::new(bad)), "{bad}");
        }
    }

    #[test]
    fn a_program_maps_to_the_app_or_the_runtime() {
        let (app, runtime) = (Path::new("/a/files"), Path::new("/r/files"));
        assert_eq!(file_on_host(Path::new("/app/bin/notes"), app, runtime), Some(PathBuf::from("/a/files/bin/notes")));
        assert_eq!(file_on_host(Path::new("/usr/bin/python3"), app, runtime), Some(PathBuf::from("/r/files/bin/python3")));
        for other in ["/home/a/.var/app/x/evil", "/tmp/evil", "/application/bin/x"] {
            assert_eq!(file_on_host(Path::new(other), app, runtime), None, "{other}");
        }
    }

    #[test]
    fn this_process_is_known_by_its_path() {
        let path = std::env::current_exe().unwrap().canonicalize().unwrap();
        assert_eq!(identify(std::process::id() as libc::pid_t), Ok(Who::Path(path)));
    }
}
