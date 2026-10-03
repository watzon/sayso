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
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
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
            Command::PushToTalkDown => HotkeyEvent::PushToTalkDown,
            Command::PushToTalkUp => HotkeyEvent::PushToTalkUp,
            Command::Open => return None,
        })
    }
}

/// `$XDG_RUNTIME_DIR/sayso.sock`, or a per-user file in the temp folder.
pub fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("sayso.sock"),
        // SAFETY: getuid has no failure mode.
        None => std::env::temp_dir().join(format!("sayso-{}.sock", unsafe { libc::getuid() })),
    }
}

/// Send a command to the running Sayso.
pub fn send(command: Command) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(socket_path())?;
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

/// Become the running Sayso, or find the one that runs.
pub fn claim() -> Claim {
    let path = socket_path();
    if UnixStream::connect(&path).is_ok() {
        return Claim::Running;
    }
    // A socket file that nobody answers is left from a crash.
    let _ = std::fs::remove_file(&path);
    match UnixListener::bind(&path) {
        Ok(listener) => {
            let spawned = std::thread::Builder::new().name("sayso-ipc".into()).spawn(move || serve(listener));
            if let Err(e) = spawned {
                log::error!("cannot start the command thread: {e}");
            }
        }
        // Without the socket Sayso still runs; only the commands are missing.
        Err(e) => log::warn!("cannot listen on {}: {e}", path.display()),
    }
    Claim::First
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
    fn words_round_trip_and_flags_parse() {
        for c in [Command::Toggle, Command::Cancel, Command::PasteLast, Command::CycleStyle, Command::Open] {
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
