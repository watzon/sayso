//! One running Sayso, and commands from the shell.
//!
//! The first Sayso listens on a Unix socket. A second start sends `open` and
//! exits, so the Hub of the running Sayso opens. `sayso --toggle` and the other
//! commands below reach the running Sayso the same way. A desktop without
//! global shortcuts for apps can bind them to a key in its own settings.

use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use sayso_platform::HotkeyEvent;
use std::io::{BufRead, BufReader, Write};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Start or stop a dictation.
    Toggle,
    /// Stop a dictation without inserting.
    Cancel,
    PasteLast,
    CycleStyle,
    /// Turn incognito on or off.
    Incognito,
    /// Push-to-talk from a key that sends one command on press and one on release.
    PushToTalkDown,
    PushToTalkUp,
    /// Open the Hub (a second start of Sayso).
    Open,
}

impl Command {
    /// The command line flag and the word on the socket.
    pub fn word(self) -> &'static str {
        match self {
            Command::Toggle => "toggle",
            Command::Cancel => "cancel",
            Command::PasteLast => "paste-last",
            Command::CycleStyle => "cycle-style",
            Command::Incognito => "incognito",
            Command::PushToTalkDown => "push-to-talk-down",
            Command::PushToTalkUp => "push-to-talk-up",
            Command::Open => "open",
        }
    }

    pub fn parse(word: &str) -> Option<Command> {
        [
            Command::Toggle,
            Command::Cancel,
            Command::PasteLast,
            Command::CycleStyle,
            Command::Incognito,
            Command::PushToTalkDown,
            Command::PushToTalkUp,
            Command::Open,
        ]
        .into_iter()
        .find(|c| c.word() == word.trim())
    }

    /// The command in the program's arguments (`--toggle`), if any.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Option<Command> {
        args.into_iter().find_map(|a| a.strip_prefix("--").and_then(Command::parse).filter(|c| *c != Command::Open))
    }

    fn hotkey(self) -> Option<HotkeyEvent> {
        Some(match self {
            Command::Toggle => HotkeyEvent::Toggle,
            Command::Cancel => HotkeyEvent::Cancel,
            Command::PasteLast => HotkeyEvent::PasteLast,
            Command::CycleStyle => HotkeyEvent::CycleStyle,
            Command::Incognito => HotkeyEvent::ToggleIncognito,
            Command::PushToTalkDown => HotkeyEvent::PushToTalkDown,
            Command::PushToTalkUp => HotkeyEvent::PushToTalkUp,
            Command::Open => return None,
        })
    }
}

/// `$XDG_RUNTIME_DIR/sayso.sock`, or `sayso.sock` in a private folder of the
/// user in the temp folder. All users can write to the temp folder, so the
/// socket is not there directly: another user could make it first.
///
/// In a Flatpak each start of Sayso has its own sandbox with its own
/// `$XDG_RUNTIME_DIR`. All of them share `$XDG_RUNTIME_DIR/app/<app id>`, so
/// the socket is there, and `flatpak run <app id> --toggle` reaches the
/// running Sayso.
pub fn socket_path() -> std::io::Result<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()).map(PathBuf::from);
    if runtime.is_none() {
        private_dir(&fallback_dir())?;
    }
    Ok(socket_path_from(runtime, sayso_core::flatpak::app_id()))
}

fn fallback_dir() -> PathBuf {
    // SAFETY: getuid has no failure mode.
    std::env::temp_dir().join(format!("sayso-{}", unsafe { libc::getuid() }))
}

/// Make `dir`, for this user only. An entry that is there must be a real
/// folder that this user owns and that no other user can open.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: getuid has no failure mode.
    if meta.is_dir() && meta.uid() == unsafe { libc::getuid() } && meta.mode() & 0o077 == 0 {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("{} is not a private folder of this user", dir.display())))
    }
}

fn socket_path_from(runtime: Option<PathBuf>, flatpak_id: Option<String>) -> PathBuf {
    match (runtime, flatpak_id) {
        (Some(dir), Some(id)) => dir.join("app").join(id).join("sayso.sock"),
        (Some(dir), None) => dir.join("sayso.sock"),
        (None, _) => fallback_dir().join("sayso.sock"),
    }
}

/// Send a command to the running Sayso.
pub fn send(command: Command) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(socket_path()?)?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(format!("{}\n", command.word()).as_bytes())
}

/// Result of [`claim`].
pub enum Claim {
    /// This is the only Sayso. It now listens for commands.
    First,
    /// Another Sayso runs.
    Running,
}

static HOTKEYS: Mutex<Option<Sender<HotkeyEvent>>> = Mutex::new(None);
static APP: OnceLock<(Sender<Command>, Receiver<Command>)> = OnceLock::new();

fn app_channel() -> &'static (Sender<Command>, Receiver<Command>) {
    APP.get_or_init(unbounded)
}

/// The instance lock of this process. It stays for the life of the process.
static LOCK: OnceLock<File> = OnceLock::new();

/// Become the running Sayso, or find the one that runs.
///
/// This runs before logging starts, so problems go to the terminal.
pub fn claim() -> Claim {
    let path = match socket_path() {
        Ok(path) => path,
        // Without the socket Sayso still runs; only the commands are missing.
        Err(e) => {
            eprintln!("sayso: commands such as --toggle do not work: {e}");
            return Claim::First;
        }
    };
    let Some((lock, listener)) = claim_at(&path) else { return Claim::Running };
    if let Some(lock) = lock {
        let _ = LOCK.set(lock);
    }
    if let Some(listener) = listener {
        let spawned = std::thread::Builder::new().name("sayso-ipc".into()).spawn(move || serve(listener));
        if let Err(e) = spawned {
            eprintln!("sayso: cannot start the command thread: {e}");
        }
    }
    Claim::First
}

/// The lock and the listener of the first Sayso at `path`. None when another
/// Sayso runs. The first Sayso holds the lock (`sayso.lock` beside the
/// socket) while it runs, so of two starts at the same time only one cleans
/// up and binds the socket.
fn claim_at(path: &Path) -> Option<(Option<File>, Option<UnixListener>)> {
    let lock = match try_lock(&path.with_extension("lock")) {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => return None,
        Err(e) => {
            eprintln!("sayso: cannot lock {}, so a second Sayso can start: {e}", path.with_extension("lock").display());
            None
        }
    };
    // A Sayso without the lock: a version before the lock.
    if UnixStream::connect(path).is_ok() {
        return None;
    }
    // A socket file that nobody answers is left from a crash.
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).map_err(|e| eprintln!("sayso: cannot listen on {}: {e}", path.display())).ok();
    Some((lock, listener))
}

/// Take the exclusive lock on the file at `path`, without a wait. None when
/// another process has it.
fn try_lock(path: &Path) -> std::io::Result<Option<File>> {
    let file = File::options().create(true).truncate(false).write(true).open(path)?;
    // SAFETY: the descriptor is open for the life of `file`.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(file));
    }
    let e = std::io::Error::last_os_error();
    if e.kind() == std::io::ErrorKind::WouldBlock { Ok(None) } else { Err(e) }
}

/// Hotkey commands go to this sender (the hotkey event stream).
pub fn forward_hotkeys(tx: Sender<HotkeyEvent>) {
    *HOTKEYS.lock() = Some(tx);
}

/// Commands for the app itself: [`Command::Open`].
pub fn app_commands() -> Receiver<Command> {
    app_channel().1.clone()
}

fn serve(listener: UnixListener) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut line = String::new();
        if BufReader::new(stream).read_line(&mut line).is_err() {
            continue;
        }
        match Command::parse(&line) {
            Some(command) => dispatch(command),
            None => log::warn!("unknown command on the socket: {:?}", line.trim()),
        }
    }
}

fn dispatch(command: Command) {
    log::info!("command: {}", command.word());
    match command.hotkey() {
        Some(event) => match HOTKEYS.lock().as_ref() {
            Some(tx) => {
                let _ = tx.send(event);
            }
            None => log::warn!("command {} came before the hotkeys started", command.word()),
        },
        None => {
            let _ = app_channel().0.send(command);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_socket_of_a_flatpak_is_in_the_folder_that_its_sandboxes_share() {
        let runtime = || Some(PathBuf::from("/run/user/1000"));
        assert_eq!(socket_path_from(runtime(), None), PathBuf::from("/run/user/1000/sayso.sock"));
        assert_eq!(
            socket_path_from(runtime(), Some("dev.sayso.Sayso".into())),
            PathBuf::from("/run/user/1000/app/dev.sayso.Sayso/sayso.sock")
        );
    }

    #[test]
    fn only_one_of_many_starts_at_the_same_time_is_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sayso.sock");
        // A socket file left from a crash: each start wants to clean it up.
        drop(UnixListener::bind(&path).unwrap());
        let barrier = std::sync::Barrier::new(16);
        let claims: Vec<_> = std::thread::scope(|s| {
            let starts: Vec<_> = (0..16)
                .map(|_| {
                    s.spawn(|| {
                        barrier.wait();
                        claim_at(&path)
                    })
                })
                .collect();
            starts.into_iter().filter_map(|start| start.join().unwrap()).collect()
        });
        assert_eq!(claims.len(), 1);
        assert!(claims[0].1.is_some(), "the first start listens");
        assert!(UnixStream::connect(&path).is_ok());
        // The lock goes away with the process, and the next start is first again.
        drop(claims);
        assert!(claim_at(&path).is_some());
    }

    #[test]
    fn a_start_finds_a_sayso_without_the_lock_by_its_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sayso.sock");
        let _old = UnixListener::bind(&path).unwrap();
        assert!(claim_at(&path).is_none());
    }

    #[test]
    fn the_folder_in_the_temp_folder_must_be_private_and_of_this_user() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let ours = dir.path().join("sayso-1000");
        private_dir(&ours).unwrap();
        assert_eq!(std::fs::metadata(&ours).unwrap().permissions().mode() & 0o777, 0o700);
        private_dir(&ours).unwrap();
        std::fs::set_permissions(&ours, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_dir(&ours).is_err(), "other users can open it");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(dir.path(), &link).unwrap();
        assert!(private_dir(&link).is_err(), "a symlink is not followed");
        let file = dir.path().join("file");
        std::fs::write(&file, "").unwrap();
        assert!(private_dir(&file).is_err());
        // A folder of another user. The root user owns `/`.
        if unsafe { libc::getuid() } != 0 {
            assert!(private_dir(Path::new("/")).is_err());
        }
    }

    #[test]
    fn words_round_trip_and_flags_parse() {
        for c in [Command::Toggle, Command::Cancel, Command::PasteLast, Command::CycleStyle, Command::Incognito, Command::Open] {
            assert_eq!(Command::parse(c.word()), Some(c));
        }
        assert_eq!(Command::parse("toggle\n"), Some(Command::Toggle));
        assert_eq!(Command::parse("dance"), None);
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(Command::from_args(args(&["sayso", "--toggle"])), Some(Command::Toggle));
        assert_eq!(Command::from_args(args(&["sayso", "--hub"])), None);
        assert_eq!(Command::from_args(args(&["sayso", "--open"])), None, "open is not a command line flag");
    }

    #[test]
    fn hotkey_commands_reach_the_hotkey_stream() {
        let (tx, rx) = unbounded();
        forward_hotkeys(tx);
        dispatch(Command::Toggle);
        assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Toggle));
        dispatch(Command::Open);
        assert_eq!(app_commands().try_recv(), Ok(Command::Open));
    }
}
