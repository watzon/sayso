//! Files that an app names (`docs/sdk.md`, "Flatpak"). A path from a sandbox
//! means a file in the sandbox. If the host opened it with its own view, a
//! sandboxed app could read each file of the user through Sayso.

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::Path;

/// The files that one process sees. It stays with this one process.
pub struct View {
    process: File,
}

impl View {
    pub fn of(pid: libc::pid_t) -> io::Result<View> {
        Ok(View { process: crate::identity::open_process(pid)? })
    }

    /// Open a regular file at `path` for reading, as this process sees it.
    /// `RESOLVE_IN_ROOT` also keeps each link and `..` in the root of the
    /// process: a link in the sandbox to `/home/me/secret` cannot leave it.
    /// A pipe or a device gives an error, and does not block the host.
    pub fn open(&self, path: &Path) -> io::Result<File> {
        if !path.is_absolute() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("path must be absolute: {}", path.display())));
        }
        let root = File::open(format!("/proc/self/fd/{}/root", self.process.as_raw_fd()))?;
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the path has a NUL"))?;
        // SAFETY: all fields of `open_how` can be zero.
        let mut how: libc::open_how = unsafe { std::mem::zeroed() };
        how.flags = (libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOCTTY) as u64;
        how.resolve = libc::RESOLVE_IN_ROOT | libc::RESOLVE_NO_MAGICLINKS;
        // SAFETY: the name and `how` live for the call, and the size is the size of `how`.
        let fd = unsafe { libc::syscall(libc::SYS_openat2, root.as_raw_fd(), name.as_ptr(), &raw const how, std::mem::size_of::<libc::open_how>()) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call gave this new descriptor.
        let file = unsafe { File::from_raw_fd(fd as i32) };
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} is not a regular file", path.display())));
        }
        Ok(file)
    }
}

/// A path that opens `file` again in this process, for as long as it is open.
pub fn path_of(file: &File) -> std::path::PathBuf {
    format!("/proc/self/fd/{}", file.as_raw_fd()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn this_process_opens_a_regular_file_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        std::fs::write(&path, b"sound").unwrap();
        let view = View::of(std::process::id() as libc::pid_t).unwrap();
        let mut text = String::new();
        view.open(&path).unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(text, "sound");
        let mut again = String::new();
        File::open(path_of(&view.open(&path).unwrap())).unwrap().read_to_string(&mut again).unwrap();
        assert_eq!(again, "sound", "the path opens the file again");
        assert!(view.open(Path::new("a.wav")).is_err(), "not absolute");
        assert!(view.open(dir.path()).is_err(), "a folder");
        assert!(view.open(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn a_magic_link_does_not_open() {
        let view = View::of(std::process::id() as libc::pid_t).unwrap();
        assert!(view.open(Path::new("/proc/self/exe")).is_err());
    }

    #[test]
    fn a_process_that_ended_shows_nothing() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let view = View::of(child.id() as libc::pid_t).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(view.open(Path::new("/etc/hostname")).is_err());
    }
}
