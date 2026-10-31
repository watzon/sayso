//! The program `sayso-bridge`: it listens where apps look for Sayso, runs the
//! checks of the host on each app, and relays the lines to the host in the
//! sandbox (`docs/sdk.md`, "Flatpak").
//!
//! It adds no trust. A client cannot send an identity: the bridge writes the
//! identity, on a channel that the client cannot reach. A read from a second
//! process closes the connection here, and the host never sees that read.

use crate::credentials;
use crate::files::View;
use crate::identity;
use crate::listen;
use crate::wire::{self, Attested, FILE_PREFIX, NO_FILE};
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// The exit code when another Sayso listens at the socket.
pub const EXIT_IN_USE: u8 = 3;

/// A client that does not read for this long loses its connection, as with the host.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
/// A longer line is not a request. The host answers it, so the bridge sends it on.
const MAX_LINE: usize = 16 * 1024 * 1024;
/// How long the last replies have to go out after Sayso closes.
const LAST_REPLIES: Duration = Duration::from_secs(3);

/// Listen at `socket` until Sayso closes `control`.
pub fn run(control: OwnedFd, socket: &Path) -> io::Result<()> {
    let listener = listen::bind(socket)?;
    credentials::pass_credentials(&listener)?;
    let file = std::fs::symlink_metadata(socket).map(|meta| (meta.dev(), meta.ino()))?;
    let control = Arc::new(control);
    let stopping = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicUsize::new(0));
    let watcher = {
        let (control, stopping, socket) = (control.clone(), stopping.clone(), socket.to_path_buf());
        std::thread::Builder::new().name("sayso-bridge-watch".into()).spawn(move || watch(&control, &stopping, &socket, file))?
    };
    for accepted in listener.incoming() {
        if stopping.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = accepted else {
            // Do not spin when the listener is broken.
            std::thread::sleep(Duration::from_millis(200));
            continue;
        };
        let (control, active) = (control.clone(), active.clone());
        active.fetch_add(1, Ordering::SeqCst);
        let spawned = std::thread::Builder::new().name("sayso-bridge-read".into()).spawn(move || {
            let _ = serve_client(stream, &control);
            active.fetch_sub(1, Ordering::SeqCst);
        });
        if let Err(e) = spawned {
            eprintln!("sayso-bridge: cannot start a connection thread: {e}");
        }
    }
    // It wakes the accept, then removes the socket: wait, or this process can end first.
    let _ = watcher.join();
    let deadline = Instant::now() + LAST_REPLIES;
    while active.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

/// Wait until Sayso has gone, then remove the socket when it is still ours and
/// wake the accept. So no stale bridge stays after Sayso, also after a crash.
fn watch(control: &OwnedFd, stopping: &AtomicBool, socket: &Path, file: (u64, u64)) {
    let mut wait = libc::pollfd { fd: control.as_raw_fd(), events: 0, revents: 0 };
    // SAFETY: `wait` lives for the call, and the count is one.
    while unsafe { libc::poll(&raw mut wait, 1, -1) } < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {}
    stopping.store(true, Ordering::SeqCst);
    let _ = UnixStream::connect(socket);
    if std::fs::symlink_metadata(socket).is_ok_and(|meta| (meta.dev(), meta.ino()) == file) {
        let _ = std::fs::remove_file(socket);
    }
}

fn serve_client(client: UnixStream, control: &OwnedFd) -> io::Result<()> {
    let writer = client.try_clone()?;
    writer.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let closer = client.try_clone()?;
    let (mut reader, writer_pid) = credentials::reader(client);
    let mut part = vec![0u8; 64 * 1024];
    let count = reader.read(&mut part)?;
    if count == 0 {
        return Ok(());
    }
    let (pid, who, view) = match writer_pid() {
        Ok(pid) => (pid, identity::identify(pid), View::of(pid).ok()),
        Err(e) => (0, Err(e), None),
    };
    let has_identity = who.is_ok();
    let (ours, theirs) = wire::pair(libc::SOCK_STREAM)?;
    let sent = wire::send(control.as_fd(), &Attested { pid, who }.encode(), Some(theirs.as_fd()));
    drop(theirs);
    sent?;
    let relay = UnixStream::from(ours);
    // The replies and the events of the host.
    let replies = {
        let mut from_host = relay.try_clone()?;
        let closer = closer.try_clone()?;
        let mut writer = writer;
        std::thread::Builder::new().name("sayso-bridge-write".into()).spawn(move || {
            let _ = io::copy(&mut from_host, &mut writer);
            let _ = closer.shutdown(Shutdown::Both);
        })?
    };
    let mut lines = Lines::new(if has_identity { view } else { None });
    let mut count = count;
    let result = loop {
        if let Err(e) = lines.relay(&part[..count], &relay) {
            break Err(e);
        }
        match reader.read(&mut part) {
            Ok(0) => break Ok(()),
            Ok(read) => count = read,
            Err(e) => break Err(e),
        }
        // After the read, before the bytes go on: the host never sees a line
        // of a second process, and a part without a pid here counts as a second process.
        if writer_pid() != Ok(pid) {
            eprintln!("sayso-bridge: another process wrote to a connection, so it closes");
            break Ok(());
        }
    };
    let _ = relay.shutdown(Shutdown::Both);
    let _ = closer.shutdown(Shutdown::Both);
    let _ = replies.join();
    result
}

/// Cuts the bytes of a client into lines, and opens the file of a `transcribe_file`.
struct Lines {
    view: Option<View>,
    pending: Vec<u8>,
    /// A line was too long: the rest goes on as it came.
    raw: bool,
    /// The number of the next file on this connection.
    next_file: u64,
}

impl Lines {
    fn new(view: Option<View>) -> Lines {
        Lines { view, pending: Vec::new(), raw: false, next_file: 0 }
    }

    fn relay(&mut self, bytes: &[u8], relay: &UnixStream) -> io::Result<()> {
        if self.raw {
            return send_all(relay, bytes, None);
        }
        self.pending.extend_from_slice(bytes);
        let mut start = 0;
        while let Some(end) = self.pending[start..].iter().position(|byte| *byte == b'\n').map(|at| start + at + 1) {
            let (line, file) = rewrite(&self.pending[start..end], self.view.as_ref(), &mut self.next_file);
            send_all(relay, &line, file.as_ref())?;
            start = end;
        }
        self.pending.drain(..start);
        if self.pending.len() > MAX_LINE {
            self.raw = true;
            send_all(relay, &std::mem::take(&mut self.pending), None)?;
        }
        Ok(())
    }
}

/// `file` goes with the first byte. Blocks until the host has read the rest.
fn send_all(relay: &UnixStream, bytes: &[u8], file: Option<&File>) -> io::Result<()> {
    let sent = wire::send(relay.as_fd(), bytes, file.map(AsFd::as_fd))?;
    (&*relay).write_all(&bytes[sent..])
}

/// The line for the host. The `path` of a `transcribe_file` means a file in the
/// view of the app, which the host in its sandbox cannot see. So the bridge
/// opens the file, and the path becomes the number of the file. Each other line
/// stays as it came.
fn rewrite(line: &[u8], view: Option<&View>, next_file: &mut u64) -> (Vec<u8>, Option<File>) {
    let unchanged = || (line.to_vec(), None);
    let Ok(mut message) = serde_json::from_slice::<serde_json::Value>(line) else { return unchanged() };
    if message.get("type").and_then(serde_json::Value::as_str) != Some("transcribe_file") {
        return unchanged();
    }
    let file = message.get("path").and_then(serde_json::Value::as_str).zip(view).and_then(|(path, view)| view.open(Path::new(path)).ok());
    let path = match &file {
        Some(_) => {
            let number = *next_file;
            *next_file += 1;
            format!("{FILE_PREFIX}{number}")
        }
        None => NO_FILE.to_string(),
    };
    message["path"] = path.into();
    let Ok(mut rewritten) = serde_json::to_vec(&message) else { return unchanged() };
    rewritten.push(b'\n');
    (rewritten, file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Who;
    use crate::wire::WaitingFiles;

    fn line_of(bytes: &[u8]) -> serde_json::Value {
        serde_json::from_slice(bytes).unwrap()
    }

    #[test]
    fn only_a_file_job_changes() {
        let view = View::of(std::process::id() as i32).unwrap();
        let mut next = 0;
        for line in [&br#"{"type":"hello","id":"1"}"#[..], b"not json", br#"{"type":"session_audio","session":1,"pcm":""}"#] {
            assert_eq!(rewrite(line, Some(&view), &mut next).0, line);
        }
        assert_eq!(next, 0);
    }

    #[test]
    fn a_file_job_gets_the_number_of_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        std::fs::write(&path, b"sound").unwrap();
        let view = View::of(std::process::id() as i32).unwrap();
        let mut next = 0;
        let line = serde_json::json!({"type": "transcribe_file", "id": "1", "path": path, "language": "en"}).to_string();
        let (rewritten, file) = rewrite(line.as_bytes(), Some(&view), &mut next);
        assert!(file.is_some());
        let message = line_of(&rewritten);
        assert_eq!(message["path"], format!("{FILE_PREFIX}0"));
        assert_eq!(message["language"], "en", "the rest stays");
        assert_eq!(*rewritten.last().unwrap(), b'\n');
        assert_eq!(next, 1);
    }

    #[test]
    fn a_file_that_does_not_open_gets_no_number() {
        let view = View::of(std::process::id() as i32).unwrap();
        let mut next = 0;
        let line = br#"{"type":"transcribe_file","path":"/no/such/file.wav"}"#;
        let (rewritten, file) = rewrite(line, Some(&view), &mut next);
        assert!(file.is_none());
        assert_eq!(line_of(&rewritten)["path"], NO_FILE);
        assert_eq!(next, 0);
        let (rewritten, file) = rewrite(br#"{"type":"transcribe_file","path":"/etc/hostname"}"#, None, &mut next);
        assert!(file.is_none(), "a process without an identity opens nothing");
        assert_eq!(line_of(&rewritten)["path"], NO_FILE);
    }

    #[test]
    fn a_client_cannot_name_a_path_for_the_host() {
        let view = View::of(std::process::id() as i32).unwrap();
        let mut next = 0;
        let line = format!(r#"{{"type":"transcribe_file","path":"{FILE_PREFIX}0"}}"#);
        let (rewritten, file) = rewrite(line.as_bytes(), Some(&view), &mut next);
        assert!(file.is_none());
        assert_eq!(line_of(&rewritten)["path"], NO_FILE, "the bridge reads each path as a path of the app");
        let mut waiting = WaitingFiles::default();
        assert!(waiting.take(line_of(&rewritten)["path"].as_str().unwrap()).is_none());
    }

    #[test]
    fn the_last_replies_go_out_and_the_socket_goes_when_sayso_closes() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("sayso/host.sock");
        let (ours, theirs) = wire::pair(libc::SOCK_SEQPACKET).unwrap();
        let bridge = {
            let socket = socket.clone();
            std::thread::spawn(move || run(theirs, &socket))
        };
        while !socket.exists() {
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut client = UnixStream::connect(&socket).unwrap();
        client.write_all(b"{\"type\":\"bye\"}\n").unwrap();
        let mut buffer = vec![0u8; wire::MAX_PACKET];
        let mut files = Vec::new();
        let count = wire::recv(ours.as_fd(), &mut buffer, &mut files).unwrap();
        let attested = Attested::decode(&buffer[..count]).unwrap();
        assert_eq!(attested.pid, std::process::id() as i32);
        assert!(matches!(attested.who, Ok(Who::Path(_))));
        assert_eq!(files.len(), 1);
        let mut relay = UnixStream::from(files.remove(0));
        let mut line = [0u8; 15];
        relay.read_exact(&mut line).unwrap();
        assert_eq!(&line, b"{\"type\":\"bye\"}\n");
        // A reply that is on its way when Sayso goes.
        relay.write_all(b"{\"type\":\"host_closing\"}\n").unwrap();
        drop(relay);
        drop(ours);
        bridge.join().unwrap().unwrap();
        assert!(!socket.exists());
        let mut reply = String::new();
        client.read_to_string(&mut reply).unwrap();
        assert_eq!(reply, "{\"type\":\"host_closing\"}\n");
    }

    #[test]
    fn a_live_socket_stops_a_second_bridge() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("sayso/host.sock");
        let _first = listen::bind(&socket).unwrap();
        let (_ours, theirs) = wire::pair(libc::SOCK_SEQPACKET).unwrap();
        assert_eq!(run(theirs, &socket).unwrap_err().kind(), io::ErrorKind::AddrInUse);
    }
}
