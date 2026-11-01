//! The host's end of `sayso-bridge` (`docs/sdk.md`, "Flatpak"). A host in a
//! Flatpak sandbox cannot see a process outside it. So Sayso starts the bridge
//! outside the sandbox, and gives it one end of a channel with no name. The
//! bridge listens where apps look for Sayso, runs the checks of the host on each
//! app, and sends each connection here, with the identity.

use crate::Shared;
use crate::identity::system::from_who;
use crate::sandbox::{Attested, FileAccess};
use crate::transport::Conn;
use sayso_bridge::wire;
use std::io;
use std::net::Shutdown;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// A client that does not read for this long loses its connection. The bridge reads at once.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
/// The first wait before a new start of a bridge that ended.
const FIRST_WAIT: Duration = Duration::from_secs(5);
const LONGEST_WAIT: Duration = Duration::from_secs(60);
/// How long the bridge has to end by itself when Sayso closes.
const END_WAIT: Duration = Duration::from_secs(4);

/// The bridge of this host.
pub(crate) struct Bridge {
    stop: Arc<AtomicBool>,
    control: Arc<Mutex<Option<Arc<UnixStream>>>>,
    thread: Option<JoinHandle<()>>,
}

impl Bridge {
    /// Start the bridge from the files of this Flatpak, and start it again when
    /// it ends. None: apps outside the sandbox use local mode.
    pub fn start(shared: Arc<Shared>, socket: PathBuf) -> Option<Bridge> {
        let program = match program() {
            Ok(program) => program,
            Err(e) => {
                log::warn!("host: apps outside the sandbox cannot connect: {e}");
                return None;
            }
        };
        let (stop, control) = (Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(None)));
        let thread = {
            let (stop, control) = (stop.clone(), control.clone());
            std::thread::Builder::new().name("sayso-host-bridge".into()).spawn(move || keep_running(&shared, &program, &socket, &stop, &control))
        };
        match thread {
            Ok(thread) => Some(Bridge { stop, control, thread: Some(thread) }),
            Err(e) => {
                log::warn!("host: cannot start the thread of the bridge: {e}");
                None
            }
        }
    }

    /// Serve the connections on `control`.
    pub fn attach(shared: Arc<Shared>, control: OwnedFd) -> Option<Bridge> {
        let control = Arc::new(UnixStream::from(control));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let control = control.clone();
            std::thread::Builder::new().name("sayso-host-bridge".into()).spawn(move || serve(&shared, &control)).ok()?
        };
        Some(Bridge { stop, control: Arc::new(Mutex::new(Some(control))), thread: Some(thread) })
    }

    /// Close the channel, so the bridge ends and removes its socket. A
    /// connection stays until its last reply is out.
    pub fn close(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(control) = self.control.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = control.shutdown(Shutdown::Both);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// `sayso-bridge` in the folder of this app, as the host outside the sandbox sees it.
fn program() -> io::Result<PathBuf> {
    let info = std::fs::read_to_string("/.flatpak-info")?;
    let info = sayso_bridge::identity::FlatpakInfo::parse(&info).map_err(io::Error::other)?;
    let program = info.app_path.join("bin/sayso-bridge");
    // The sandbox has the same file at this other path.
    if !Path::new("/app/bin/sayso-bridge").exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "this Flatpak has no sayso-bridge"));
    }
    Ok(program)
}

fn keep_running(shared: &Arc<Shared>, program: &Path, socket: &Path, stop: &AtomicBool, slot: &Mutex<Option<Arc<UnixStream>>>) {
    let mut wait = FIRST_WAIT;
    while !stop.load(Ordering::SeqCst) {
        let started = Instant::now();
        match run_once(shared, program, socket, stop, slot) {
            Ok(status) if status.code() == Some(sayso_bridge::serve::EXIT_IN_USE as i32) => log::info!("host: another Sayso serves apps at {}, so the bridge waits", socket.display()),
            Ok(status) => log::info!("host: the bridge ended: {status}"),
            Err(e) => log::warn!("host: cannot run the bridge: {e}"),
        }
        if started.elapsed() > LONGEST_WAIT {
            wait = FIRST_WAIT;
        }
        let until = Instant::now() + wait;
        while Instant::now() < until && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
        wait = (wait * 2).min(LONGEST_WAIT);
    }
}

fn run_once(shared: &Arc<Shared>, program: &Path, socket: &Path, stop: &AtomicBool, slot: &Mutex<Option<Arc<UnixStream>>>) -> io::Result<std::process::ExitStatus> {
    // The other end of the channel closes at an exec. Only this child keeps it.
    let (ours, theirs) = wire::pair(libc::SOCK_SEQPACKET)?;
    let fd = theirs.as_raw_fd();
    let mut command = sayso_core::flatpak::host_command_with_fd(program, fd);
    command.arg(format!("--relay-fd={fd}")).arg({
        let mut arg = std::ffi::OsString::from("--socket=");
        arg.push(socket);
        arg
    });
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
    // SAFETY: `fcntl` is safe between fork and exec, and `theirs` is open until the spawn ends.
    unsafe {
        command.pre_exec(move || if libc::fcntl(fd, libc::F_SETFD, 0) == 0 { Ok(()) } else { Err(io::Error::last_os_error()) });
    }
    let mut child = command.spawn()?;
    drop(theirs);
    let ours = Arc::new(UnixStream::from(ours));
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(ours.clone());
    // `Bridge::close` can come just before the channel is in the slot.
    if stop.load(Ordering::SeqCst) {
        let _ = ours.shutdown(Shutdown::Both);
    }
    log::info!("host: the bridge starts at {}", socket.display());
    serve(shared, &ours);
    let deadline = Instant::now() + END_WAIT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            return child.wait();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Take each new connection, until the channel ends.
fn serve(shared: &Arc<Shared>, control: &UnixStream) {
    let mut packet = vec![0u8; wire::MAX_PACKET];
    loop {
        let mut files = Vec::new();
        let count = match wire::recv(control.as_fd(), &mut packet, &mut files) {
            Ok(0) => return,
            Ok(count) => count,
            Err(e) => {
                log::warn!("host: the channel of the bridge failed: {e}");
                return;
            }
        };
        let (Some(attested), Some(relay)) = (wire::Attested::decode(&packet[..count]), files.pop()) else {
            log::warn!("host: the bridge sent a broken message");
            continue;
        };
        match conn(attested, relay) {
            Ok(conn) => crate::serve_connection(shared, conn),
            Err(e) => log::warn!("host: cannot serve a connection of the bridge: {e}"),
        }
    }
}

/// The reader gives the file of each `transcribe_file`, and the bridge checked
/// the writer of each read before it sent the bytes on. So the host asks the
/// system nothing here, and the writer does not change.
fn conn(attested: wire::Attested, relay: OwnedFd) -> io::Result<Conn> {
    let stream = UnixStream::from(relay);
    let writer = stream.try_clone()?;
    writer.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let closer = stream.try_clone()?;
    let (reader, files) = wire::RelayReader::new(stream);
    let pid = attested.pid;
    Ok(Conn {
        reader: Box::new(reader),
        writer: Box::new(writer),
        closer: Arc::new(move || {
            let _ = closer.shutdown(Shutdown::Both);
        }),
        peer: Box::new(move || Ok(crate::Peer { pid })),
        attested: Some(Attested { caller: attested.who.map(from_who), files: FileAccess::FromBridge(files) }),
    })
}
