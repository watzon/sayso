//! The socket file and its folder, for the host and for the bridge.

use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

/// Listen at `path`. A live socket there gives `AddrInUse`. A socket file that
/// nobody answers is left from a crash, and it goes away.
pub fn bind(path: &Path) -> io::Result<UnixListener> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| io::Error::other(format!("{} has no folder", path.display())))?;
    private_dir(parent)?;
    if UnixStream::connect(path).is_ok() {
        return Err(io::Error::new(io::ErrorKind::AddrInUse, format!("another Sayso serves extensions at {}", path.display())));
    }
    let _ = std::fs::remove_file(path);
    UnixListener::bind(path)
}

/// Make `dir` for this user only (mode 0700). A folder that is there must be
/// a real folder of this user that no other user can open.
pub fn private_dir(dir: &Path) -> io::Result<()> {
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
