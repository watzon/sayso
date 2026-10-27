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
        let meta = std::fs::symlink_metadata(path)?;
        Ok(Listener { listener, path: path.to_path_buf(), file: (meta.dev(), meta.ino()) })
    }

    pub fn accept(&self) -> io::Result<Conn> {
        let (stream, _) = self.listener.accept()?;
        let writer = stream.try_clone()?;
        writer.set_write_timeout(Some(WRITE_TIMEOUT))?;
        let closer = stream.try_clone()?;
        let peer = crate::Peer { stream: stream.try_clone()? };
        Ok(Conn {
            reader: Box::new(stream),
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
