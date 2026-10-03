//! The install lock: one file for one install.
//!
//! Each Sayso holds a shared lock while it runs. Staging and the swap need
//! the exclusive lock, so they run only when no other Sayso uses the install.
//! Inside one process, a mutex gives the exclusive lock to one thread at a
//! time: the file lock alone belongs to the whole process.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

pub struct InstallLock {
    #[cfg(unix)]
    file: std::fs::File,
    /// One update step at a time in this process.
    busy: Mutex<()>,
}

/// The exclusive lock. It goes back to the shared lock when it drops.
pub struct Exclusive<'a> {
    lock: &'a InstallLock,
    _busy: MutexGuard<'a, ()>,
}

impl Drop for Exclusive<'_> {
    fn drop(&mut self) {
        self.lock.flock_shared();
    }
}

impl InstallLock {
    /// Take the shared lock. Returns the lock and true when it had to wait:
    /// another Sayso had the exclusive lock, so a swap can have run in the
    /// meantime, and the caller can be the old version.
    #[cfg(unix)]
    pub fn shared(path: &Path) -> std::io::Result<(Self, bool)> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::fs::File::options().create(true).truncate(false).write(true).open(path)?;
        let lock = Self { file, busy: Mutex::new(()) };
        if lock.flock(libc::LOCK_SH | libc::LOCK_NB) {
            return Ok((lock, false));
        }
        if lock.flock(libc::LOCK_SH) { Ok((lock, true)) } else { Err(std::io::Error::last_os_error()) }
    }

    /// Off Unix no install updates itself yet, so the lock there is always free.
    #[cfg(not(unix))]
    pub fn shared(_path: &Path) -> std::io::Result<(Self, bool)> {
        Ok((Self { busy: Mutex::new(()) }, false))
    }

    /// Change the shared lock to the exclusive lock, without a wait. None
    /// when another Sayso runs from this install, or when another thread of
    /// this process has the exclusive lock. The shared lock stays.
    pub fn try_exclusive(&self) -> Option<Exclusive<'_>> {
        let busy = self.busy.try_lock().ok()?;
        #[cfg(unix)]
        if !self.flock(libc::LOCK_EX | libc::LOCK_NB) {
            // A failed change can drop the lock that was there.
            self.flock_shared();
            return None;
        }
        Some(Exclusive { lock: self, _busy: busy })
    }

    fn flock_shared(&self) {
        #[cfg(unix)]
        self.flock(libc::LOCK_SH);
    }

    #[cfg(unix)]
    fn flock(&self, operation: libc::c_int) -> bool {
        use std::os::fd::AsRawFd;
        // SAFETY: the descriptor is open for the life of `self`.
        unsafe { libc::flock(self.file.as_raw_fd(), operation) == 0 }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn the_exclusive_lock_needs_all_other_holders_gone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("install.lock");
        let (a, waited) = InstallLock::shared(&path).unwrap();
        assert!(!waited);
        drop(a.try_exclusive().unwrap());
        let (b, waited) = InstallLock::shared(&path).unwrap();
        assert!(!waited);
        assert!(a.try_exclusive().is_none());
        // The failed try did not drop the shared lock of `a`.
        assert!(b.try_exclusive().is_none());
        drop(b);
        assert!(a.try_exclusive().is_some());
    }

    #[test]
    fn one_thread_of_a_process_has_the_exclusive_lock_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let (lock, _) = InstallLock::shared(&dir.path().join("install.lock")).unwrap();
        let held = lock.try_exclusive().unwrap();
        std::thread::scope(|s| {
            assert!(s.spawn(|| lock.try_exclusive().is_none()).join().unwrap());
        });
        drop(held);
        assert!(lock.try_exclusive().is_some());
    }

    #[test]
    fn a_start_during_a_swap_waits_and_knows_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("install.lock");
        let (a, _) = InstallLock::shared(&path).unwrap();
        let held = a.try_exclusive().unwrap();
        let path2 = path.clone();
        let starter = std::thread::spawn(move || InstallLock::shared(&path2).unwrap().1);
        std::thread::sleep(std::time::Duration::from_millis(100));
        drop(held);
        assert!(starter.join().unwrap());
    }
}
