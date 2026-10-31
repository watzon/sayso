use super::Conn;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A client that does not read for this long loses its connection.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) struct Listener {
    listener: UnixListener,
    path: PathBuf,
    /// The device and inode of the socket file, so `remove` deletes only ours.
    file: (u64, u64),
}

impl Listener {
    pub fn bind(path: &Path) -> io::Result<Listener> {
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| io::Error::other(format!("{} has no folder", path.display())))?;
        private_dir(parent)?;
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AddrInUse, format!("another Sayso serves extensions at {}", path.display())));
        }
        // A socket file that nobody answers is left from a crash.
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        #[cfg(target_os = "linux")]
        linux::pass_credentials(&listener)?;
        let meta = std::fs::symlink_metadata(path)?;
        Ok(Listener { listener, path: path.to_path_buf(), file: (meta.dev(), meta.ino()) })
    }

    pub fn accept(&self) -> io::Result<Conn> {
        let (stream, _) = self.listener.accept()?;
        let writer = stream.try_clone()?;
        writer.set_write_timeout(Some(WRITE_TIMEOUT))?;
        let closer = stream.try_clone()?;
        #[cfg(target_os = "linux")]
        let (reader, peer) = linux::reader(stream);
        #[cfg(target_os = "macos")]
        let (reader, peer): (Box<dyn std::io::Read + Send>, Box<dyn Fn() -> Result<crate::Peer, String> + Send>) = {
            let writer_of = stream.try_clone()?;
            (Box::new(stream), Box::new(move || crate::identity::system::last_writer(&writer_of)))
        };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let (reader, peer): (Box<dyn std::io::Read + Send>, Box<dyn Fn() -> Result<crate::Peer, String> + Send>) = (Box::new(stream), Box::new(|| Ok(crate::Peer {})));
        Ok(Conn {
            reader,
            writer: Box::new(writer),
            closer: Arc::new(move || {
                let _ = closer.shutdown(std::net::Shutdown::Both);
            }),
            peer,
        })
    }

    /// Make a blocked `accept` return. False when that failed.
    pub fn wake(&self) -> bool {
        UnixStream::connect(&self.path).is_ok()
    }

    /// Delete the socket file, when it is still the file of this listener.
    pub fn remove(&self) {
        if let Ok(meta) = std::fs::symlink_metadata(&self.path)
            && (meta.dev(), meta.ino()) == self.file
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Make `dir` for this user only (mode 0700). A folder that is there must be
/// a real folder of this user that no other user can open.
fn private_dir(dir: &Path) -> io::Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(io::Error::new(e.kind(), format!("cannot make {}: {e}", dir.display()))),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: getuid has no failure mode.
    if meta.is_dir() && meta.uid() == unsafe { libc::getuid() } && meta.mode() & 0o077 == 0 {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{} is not a private folder of this user", dir.display())))
    }
}

/// The writer of each read on Linux. `SO_PEERCRED` says only who connected.
#[cfg(target_os = "linux")]
mod linux {
    use parking_lot::Mutex;
    use std::io::{self, Read};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::Arc;

    /// On the listener, so a line that a client writes before the accept
    /// also has its credentials. A connection gets the option with the accept.
    pub fn pass_credentials(listener: &UnixListener) -> io::Result<()> {
        let on: libc::c_int = 1;
        // SAFETY: the socket is open, and the size is the size of `on`.
        let result = unsafe { libc::setsockopt(listener.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PASSCRED, (&raw const on).cast(), std::mem::size_of::<libc::c_int>() as libc::socklen_t) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Who wrote what the reader has so far.
    #[derive(Default)]
    struct Writers {
        first: Option<libc::pid_t>,
        /// A second process, or a part without a pid in this PID namespace.
        other: bool,
    }

    impl Writers {
        fn add(&mut self, pid: Option<libc::pid_t>) {
            match (pid.filter(|pid| *pid > 0), self.first) {
                (Some(pid), None) => self.first = Some(pid),
                (Some(pid), Some(first)) if pid == first => {}
                _ => self.other = true,
            }
        }

        fn peer(&self) -> Result<crate::Peer, String> {
            match self.first {
                Some(pid) if !self.other => Ok(crate::Peer { pid }),
                _ => Err("more than one process wrote to this connection".into()),
            }
        }
    }

    /// The reader of a connection, and what gives the writer. The reader takes
    /// a part ahead of the line, so a second writer closes the connection at
    /// the line before its own at the latest.
    pub fn reader(stream: UnixStream) -> (Box<dyn Read + Send>, Box<dyn Fn() -> Result<crate::Peer, String> + Send>) {
        let writers = Arc::new(Mutex::new(Writers::default()));
        let peer = writers.clone();
        (Box::new(CredentialReader { stream, writers }), Box::new(move || peer.lock().peer()))
    }

    struct CredentialReader {
        stream: UnixStream,
        writers: Arc<Mutex<Writers>>,
    }

    impl Read for CredentialReader {
        /// One `recvmsg`. The kernel does not join the bytes of two writers.
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let mut part = libc::iovec { iov_base: buffer.as_mut_ptr().cast(), iov_len: buffer.len() };
            // Room for one `ucred`, in the alignment of `cmsghdr`. So file
            // descriptors that a client sends do not come into the host.
            let mut control = [0u64; 4];
            // SAFETY: all fields of `msghdr` can be zero.
            let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
            message.msg_iov = &raw mut part;
            message.msg_iovlen = 1;
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = std::mem::size_of_val(&control) as _;
            // SAFETY: the socket is open, and `message` points to live buffers of the sizes that it says.
            let count = unsafe { libc::recvmsg(self.stream.as_raw_fd(), &raw mut message, libc::MSG_CMSG_CLOEXEC) };
            if count < 0 {
                return Err(io::Error::last_os_error());
            }
            if count > 0 {
                self.writers.lock().add(sender(&message));
            }
            Ok(count as usize)
        }
    }

    fn sender(message: &libc::msghdr) -> Option<libc::pid_t> {
        // SAFETY: `recvmsg` filled the control data of `message`, and the
        // macros stay in its length. The data of `SCM_CREDENTIALS` is a `ucred`.
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(message);
            while !header.is_null() {
                if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_CREDENTIALS {
                    return Some(std::ptr::read_unaligned(libc::CMSG_DATA(header).cast::<libc::ucred>()).pid);
                }
                header = libc::CMSG_NXTHDR(message, header);
            }
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_second_writer_or_no_pid_gives_no_peer() {
            let mut writers = Writers::default();
            assert!(writers.peer().is_err(), "nothing was written");
            writers.add(Some(7));
            writers.add(Some(7));
            assert_eq!(writers.peer().unwrap().pid, 7);
            let mut second = Writers { first: Some(7), other: false };
            second.add(Some(8));
            assert!(second.peer().is_err());
            for no_pid in [None, Some(0)] {
                let mut writers = Writers::default();
                writers.add(no_pid);
                assert!(writers.peer().is_err());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_folder_of_the_socket_must_be_private() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("sayso");
        let listener = Listener::bind(&folder.join("host.sock")).unwrap();
        assert_eq!(std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777, 0o700);
        drop(listener);
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(Listener::bind(&folder.join("host.sock")).err().unwrap().kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn a_stale_socket_is_replaced_and_a_live_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sayso/host.sock");
        private_dir(path.parent().unwrap()).unwrap();
        drop(UnixListener::bind(&path).unwrap());
        let first = Listener::bind(&path).expect("a socket that nobody answers is stale");
        assert_eq!(Listener::bind(&path).err().unwrap().kind(), io::ErrorKind::AddrInUse);
        first.remove();
        assert!(!path.exists());
    }

    #[test]
    fn remove_leaves_the_socket_of_another_listener() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sayso/host.sock");
        let old = Listener::bind(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let _new = Listener::bind(&path).unwrap();
        old.remove();
        assert!(path.exists());
    }
}
