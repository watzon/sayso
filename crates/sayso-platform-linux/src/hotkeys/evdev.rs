//! The listen-only key stream from `/dev/input`.
//!
//! One thread reads every keyboard with `poll(2)`. It never grabs a device,
//! so all keys also reach the focused app. An inotify watch on `/dev/input`
//! picks up keyboards that are plugged in later. The user needs read access
//! to the event devices, which normally means membership in the `input`
//! group. This works in X11 and Wayland sessions.

use super::listen::Listen;
use super::logic::KeyAction;
use evdev::raw_stream::RawDevice;
use evdev::{EventType, KeyCode};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

const INPUT_DIR: &str = "/dev/input";

/// Whether the user can read the input event devices. Cheap: it does not
/// open any device.
pub fn devices_readable() -> bool {
    event_paths().iter().any(|p| {
        let Ok(c) = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()) else { return false };
        // SAFETY: plain permission check on a valid C string.
        unsafe { libc::access(c.as_ptr(), libc::R_OK) == 0 }
    })
}

/// The `/dev/input/event*` files.
fn event_paths() -> Vec<PathBuf> {
    let Ok(dir) = std::fs::read_dir(INPUT_DIR) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = dir
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("event")))
        .collect();
    paths.sort();
    paths
}

/// Open a device read-only and non-blocking. `None` when it is not a keyboard.
fn open_keyboard(path: &Path) -> std::io::Result<Option<RawDevice>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let device = RawDevice::from_fd(file.into())?;
    let is_keyboard = device.supported_events().contains(EventType::KEY)
        && device.supported_keys().is_some_and(|keys| {
            keys.contains(KeyCode::KEY_A) && keys.contains(KeyCode::KEY_Z) && keys.contains(KeyCode::KEY_SPACE)
        });
    Ok(is_keyboard.then_some(device))
}

/// The running reader thread. Dropping it stops the thread.
pub struct EvdevReader {
    wake: OwnedFd,
    join: Option<JoinHandle<()>>,
}

impl EvdevReader {
    /// Open the keyboards and start the reader thread. Fails when no event
    /// device can be read.
    pub fn start(listen: Arc<Listen>) -> Result<Self, String> {
        let mut devices = BTreeMap::new();
        let mut denied = 0;
        for path in event_paths() {
            match open_keyboard(&path) {
                Ok(Some(device)) => {
                    log::info!("listening to keyboard {} ({})", device.name().unwrap_or("?"), path.display());
                    devices.insert(path, device);
                }
                Ok(None) => {}
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => denied += 1,
                Err(e) => log::debug!("cannot open {}: {e}", path.display()),
            }
        }
        if devices.is_empty() && denied > 0 {
            return Err(format!(
                "no access to {INPUT_DIR}. Add the user to the input group and log in again"
            ));
        }
        if devices.is_empty() && event_paths().is_empty() {
            return Err(format!("{INPUT_DIR} has no event devices"));
        }
        // SAFETY: plain syscalls; the results are checked before use.
        let wake = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if wake < 0 {
            return Err(format!("eventfd failed: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: `wake` is a new descriptor that nothing else owns.
        let wake = unsafe { OwnedFd::from_raw_fd(wake) };
        let notify = watch_input_dir();
        let wake_fd = wake.as_raw_fd();
        let join = std::thread::Builder::new()
            .name("sayso-evdev".into())
            .spawn(move || Reader { listen, devices, notify, wake: wake_fd }.run())
            .map_err(|e| format!("cannot start the evdev thread: {e}"))?;
        Ok(Self { wake, join: Some(join) })
    }
}

impl Drop for EvdevReader {
    fn drop(&mut self) {
        let one: u64 = 1;
        // SAFETY: writes 8 bytes from a valid u64 to our own eventfd.
        unsafe { libc::write(self.wake.as_raw_fd(), (&one as *const u64).cast(), 8) };
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// An inotify descriptor that watches `/dev/input` for new and changed
/// files. udev sets the permissions after it creates a node, so attribute
/// changes count too. `None` when inotify is not available.
fn watch_input_dir() -> Option<OwnedFd> {
    // SAFETY: plain syscalls; the results are checked before use.
    unsafe {
        let fd = libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC);
        if fd < 0 {
            log::warn!("inotify is not available; new keyboards need a restart");
            return None;
        }
        let fd = OwnedFd::from_raw_fd(fd);
        let dir = c"/dev/input";
        if libc::inotify_add_watch(fd.as_raw_fd(), dir.as_ptr(), libc::IN_CREATE | libc::IN_ATTRIB) < 0 {
            log::warn!("cannot watch {INPUT_DIR}: {}", std::io::Error::last_os_error());
            return None;
        }
        Some(fd)
    }
}

struct Reader {
    listen: Arc<Listen>,
    devices: BTreeMap<PathBuf, RawDevice>,
    notify: Option<OwnedFd>,
    wake: RawFd,
}

impl Reader {
    fn run(mut self) {
        loop {
            let mut fds: Vec<libc::pollfd> = vec![pollfd(self.wake)];
            if let Some(n) = &self.notify {
                fds.push(pollfd(n.as_raw_fd()));
            }
            let first_device = fds.len();
            let paths: Vec<PathBuf> = self.devices.keys().cloned().collect();
            fds.extend(self.devices.values().map(|d| pollfd(d.as_raw_fd())));
            // SAFETY: `fds` is a valid array of pollfd for the whole call.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                log::error!("poll failed, the evdev reader stops: {err}");
                break;
            }
            if fds[0].revents != 0 {
                break;
            }
            if self.notify.is_some() && fds[1].revents != 0 {
                self.drain_notify();
                self.rescan();
            }
            for (fd, path) in fds[first_device..].iter().zip(paths) {
                if fd.revents != 0 {
                    self.read_device(&path);
                }
            }
        }
        self.listen.reset_keys();
    }

    fn read_device(&mut self, path: &Path) {
        let Some(device) = self.devices.get_mut(path) else { return };
        let mut keys = Vec::new();
        let result = device.fetch_events().map(|events| {
            for event in events {
                if event.event_type() == EventType::KEY {
                    let action = match event.value() {
                        0 => KeyAction::Up,
                        1 => KeyAction::Down,
                        _ => KeyAction::Repeat,
                    };
                    keys.push((event.code(), action));
                }
            }
        });
        for (code, action) in keys {
            self.listen.key(code, action);
        }
        match result {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => {
                // ENODEV: the keyboard was unplugged.
                log::info!("keyboard {} is gone: {e}", path.display());
                self.devices.remove(path);
                // Its key-up events will not arrive.
                self.listen.reset_keys();
            }
        }
    }

    fn drain_notify(&self) {
        let Some(n) = &self.notify else { return };
        let mut buf = [0u8; 4096];
        // SAFETY: reads into a valid buffer of the given size.
        while unsafe { libc::read(n.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
    }

    /// Open keyboards that appeared since the last scan.
    fn rescan(&mut self) {
        for path in event_paths() {
            if self.devices.contains_key(&path) {
                continue;
            }
            match open_keyboard(&path) {
                Ok(Some(device)) => {
                    log::info!("listening to new keyboard {} ({})", device.name().unwrap_or("?"), path.display());
                    self.devices.insert(path, device);
                }
                Ok(None) => {}
                // udev may not have set the permissions yet; IN_ATTRIB brings us back.
                Err(e) => log::debug!("cannot open {} yet: {e}", path.display()),
            }
        }
    }
}

fn pollfd(fd: RawFd) -> libc::pollfd {
    libc::pollfd { fd, events: libc::POLLIN, revents: 0 }
}
