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
use evdev::{AttributeSetRef, EventType, InputEvent, KeyCode, RelativeAxisCode};
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

fn is_pointer_button(code: u16) -> bool {
    (KeyCode::BTN_LEFT.0..=KeyCode::BTN_TASK.0).contains(&code)
        || (KeyCode::BTN_TOOL_PEN.0..=KeyCode::BTN_TOOL_QUADTAP.0).contains(&code)
}

fn is_input_device(keys: Option<&AttributeSetRef<KeyCode>>, scroll: bool) -> bool {
    scroll || keys.is_some_and(|keys| {
        (keys.contains(KeyCode::KEY_A) && keys.contains(KeyCode::KEY_Z) && keys.contains(KeyCode::KEY_SPACE))
            || keys.iter().any(|key| is_pointer_button(key.0))
    })
}

fn is_pointer_activity(event: &InputEvent) -> bool {
    (event.event_type() == EventType::KEY && is_pointer_button(event.code()) && event.value() != 0)
        || (event.event_type() == EventType::RELATIVE && event.value() != 0
            && matches!(RelativeAxisCode(event.code()), RelativeAxisCode::REL_WHEEL | RelativeAxisCode::REL_HWHEEL
                | RelativeAxisCode::REL_WHEEL_HI_RES | RelativeAxisCode::REL_HWHEEL_HI_RES))
}

/// Open keyboards and pointers read-only: button/scroll gestures cancel taps.
fn open_input(path: &Path) -> std::io::Result<Option<RawDevice>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let device = RawDevice::from_fd(file.into())?;
    let scroll = device.supported_relative_axes().is_some_and(|axes| {
        axes.contains(RelativeAxisCode::REL_WHEEL) || axes.contains(RelativeAxisCode::REL_HWHEEL)
            || axes.contains(RelativeAxisCode::REL_WHEEL_HI_RES) || axes.contains(RelativeAxisCode::REL_HWHEEL_HI_RES)
    });
    Ok(is_input_device(device.supported_keys(), scroll).then_some(device))
}

/// Shared device-opening seam for startup and hotplug. An open failure leaves
/// the device's capabilities unknown, including whether it is a pointer.
fn rescan_devices<T>(
    listen: &Listen,
    devices: &mut BTreeMap<PathBuf, T>,
    paths: Vec<PathBuf>,
    mut open: impl FnMut(&Path) -> std::io::Result<Option<T>>,
) -> bool {
    let mut available = true;
    for path in paths {
        if devices.contains_key(&path) {
            continue;
        }
        match open(&path) {
            Ok(Some(device)) => {
                log::info!("listening to input device {}", path.display());
                devices.insert(path, device);
            }
            Ok(None) => {}
            Err(e) => {
                available = false;
                log::warn!("modifier toggles paused: cannot open {} yet: {e}", path.display());
            }
        }
    }
    // udev may not have set permissions yet; IN_ATTRIB retries the scan.
    // Keep keyboard features running, but no modifier toggle is safe while
    // an unopened device could be an unseen mouse.
    listen.logic.lock().set_pointer_available(available);
    available
}

fn initial_devices<T>(
    listen: &Listen,
    paths: Vec<PathBuf>,
    open: impl FnMut(&Path) -> std::io::Result<Option<T>>,
) -> Result<BTreeMap<PathBuf, T>, String> {
    if paths.is_empty() {
        return Err(format!("{INPUT_DIR} has no event devices"));
    }
    let mut devices = BTreeMap::new();
    let complete = rescan_devices(listen, &mut devices, paths, open);
    if devices.is_empty() && !complete {
        return Err(format!("cannot read input devices in {INPUT_DIR}; input monitoring access is required"));
    }
    Ok(devices)
}

/// The running reader thread. Dropping it stops the thread.
pub struct EvdevReader {
    wake: OwnedFd,
    join: Option<JoinHandle<()>>,
}

impl EvdevReader {
    /// Open accessible input devices and start the reader thread. Unreadable
    /// devices pause only modifier-only toggles until a rescan succeeds.
    /// Fails when the directory is empty, or no device opens and any open failed.
    pub fn start(listen: Arc<Listen>) -> Result<Self, String> {
        let devices = initial_devices(&listen, event_paths(), open_input)?;
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
        if libc::inotify_add_watch(fd.as_raw_fd(), dir.as_ptr(), libc::IN_CREATE | libc::IN_ATTRIB | libc::IN_DELETE) < 0 {
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
            let mut events = Vec::new();
            for (fd, path) in fds[first_device..].iter().zip(paths) {
                if fd.revents != 0 {
                    self.read_device(&path, &mut events);
                }
            }
            dispatch_batch(&self.listen, events);
        }
        self.listen.reset_keys();
    }

    fn read_device(&mut self, path: &Path, batch: &mut Vec<InputEvent>) {
        let Some(device) = self.devices.get_mut(path) else { return };
        let result = device.fetch_events().map(|events| {
            batch.extend(events);
        });
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

    /// Open keyboards and pointers that appeared since the last scan.
    fn rescan(&mut self) {
        rescan_devices(&self.listen, &mut self.devices, event_paths(), open_input);
    }
}

/// Merge device queues before dispatch: a keyboard batch must not deliver the
/// release before a click already queued on a separate pointer descriptor.
fn dispatch_batch(listen: &Listen, mut events: Vec<InputEvent>) {
    events.sort_by_key(InputEvent::timestamp);
    for event in events {
        if is_pointer_activity(&event) {
            listen.pointer_activity();
        } else if event.event_type() == EventType::KEY && !is_pointer_button(event.code()) {
            let action = match event.value() {
                0 => KeyAction::Up,
                1 => KeyAction::Down,
                _ => KeyAction::Repeat,
            };
            listen.key(event.code(), action);
        }
    }
}

fn pollfd(fd: RawFd) -> libc::pollfd {
    libc::pollfd { fd, events: libc::POLLIN, revents: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evdev::{InputEvent, RelativeAxisCode};

    fn paths() -> Vec<PathBuf> {
        vec![PathBuf::from("event-keyboard"), PathBuf::from("event-unknown")]
    }

    fn tap(listen: &Listen, code: KeyCode) {
        dispatch_batch(listen, vec![
            InputEvent::new(EventType::KEY.0, code.0, 1),
            InputEvent::new(EventType::KEY.0, code.0, 0),
        ]);
    }

    fn configure_toggle(listen: &Listen) {
        listen.logic.lock().set_toggle_modifiers("left_control".parse::<sayso_core::hotkey::Hotkey>().unwrap().modifier_keys());
    }

    #[test]
    fn startup_open_failures_pause_only_modifier_toggles() {
        use sayso_platform::HotkeyEvent;
        for errno in [libc::EACCES, libc::EPERM, libc::ENOENT, libc::ENODEV, libc::EIO] {
            let (tx, rx) = crossbeam_channel::unbounded();
            let listen = Arc::new(Listen::new(tx));
            configure_toggle(&listen);
            let devices = initial_devices(&listen, paths(), |path| {
                if path == Path::new("event-keyboard") { Ok(Some(())) }
                else { Err(std::io::Error::from_raw_os_error(errno)) }
            }).expect("an unreadable possible pointer must not stop the keyboard stream");
            assert_eq!(devices.len(), 1);
            tap(&listen, KeyCode::KEY_LEFTCTRL);
            assert!(rx.try_recv().is_err(), "unsafe toggle after open error {errno}");
            configure_toggle(&listen);
            tap(&listen, KeyCode::KEY_LEFTCTRL);
            assert!(rx.try_recv().is_err(), "rebinding must not bypass degradation");

            listen.logic.lock().set_ptt(Some("right_control".parse().unwrap()));
            tap(&listen, KeyCode::KEY_RIGHTCTRL);
            assert_eq!(rx.try_recv(), Ok(HotkeyEvent::PushToTalkDown));
            assert_eq!(rx.try_recv(), Ok(HotkeyEvent::PushToTalkUp));
            listen.logic.lock().set_chords(vec![(
                super::super::keymap::chord(&"f5".parse().unwrap()).unwrap(), HotkeyEvent::Toggle,
            )]);
            tap(&listen, KeyCode::KEY_F5);
            assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Toggle));
            listen.logic.lock().configure_esc(true, std::time::Duration::from_millis(400));
            listen.logic.lock().set_recording(true);
            tap(&listen, KeyCode::KEY_ESC);
            assert_eq!(rx.try_recv(), Ok(HotkeyEvent::Cancel));
            let captured = listen.open_capture();
            tap(&listen, KeyCode::KEY_F6);
            assert_eq!(captured.try_recv(), Ok("f6".parse().unwrap()));
            assert!(rx.try_recv().is_err());
        }
    }

    #[test]
    fn rescan_open_failures_pause_and_recovery_restores_modifier_toggles() {
        for errno in [libc::EACCES, libc::EPERM, libc::ENOENT, libc::ENODEV, libc::EIO] {
            for recovered_device in [Some(()), None] {
                let (tx, rx) = crossbeam_channel::unbounded();
                let listen = Listen::new(tx);
                configure_toggle(&listen);
                let mut devices = initial_devices(&listen, vec![paths()[0].clone()], |_| Ok(Some(()))).unwrap();
                tap(&listen, KeyCode::KEY_LEFTCTRL);
                assert_eq!(rx.try_recv(), Ok(sayso_platform::HotkeyEvent::Toggle));
                listen.key(KeyCode::KEY_LEFTCTRL.0, KeyAction::Down);
                rescan_devices(&listen, &mut devices, paths(), |path| {
                    assert_eq!(path, Path::new("event-unknown"), "existing keyboard must remain open");
                    Err(std::io::Error::from_raw_os_error(errno))
                });
                listen.key(KeyCode::KEY_LEFTCTRL.0, KeyAction::Up);
                assert!(rx.try_recv().is_err(), "rescan must clear an in-flight tap");
                configure_toggle(&listen);
                tap(&listen, KeyCode::KEY_LEFTCTRL);
                assert!(rx.try_recv().is_err());
                // Recovery cannot complete a tap begun while degraded.
                listen.key(KeyCode::KEY_LEFTCTRL.0, KeyAction::Down);
                rescan_devices(&listen, &mut devices, paths(), |_| Ok(recovered_device));
                listen.key(KeyCode::KEY_LEFTCTRL.0, KeyAction::Up);
                assert!(rx.try_recv().is_err());
                tap(&listen, KeyCode::KEY_LEFTCTRL);
                assert_eq!(rx.try_recv(), Ok(sayso_platform::HotkeyEvent::Toggle));
                assert_eq!(devices.len(), 1 + usize::from(recovered_device.is_some()));
            }
        }
    }

    #[test]
    fn startup_without_access_reports_failure_instead_of_an_empty_listener() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let listen = Listen::new(tx);
        for errno in [libc::EACCES, libc::EPERM, libc::ENOENT, libc::ENODEV, libc::EIO] {
            let result = initial_devices::<()>(&listen, paths(), |_| {
                Err(std::io::Error::from_raw_os_error(errno))
            });
            assert!(result.is_err(), "no input stream after open error {errno}");
        }
        assert!(initial_devices::<()>(&listen, vec![], |_| panic!("no paths to open")).is_err());
    }

    #[test]
    fn batches_merge_pointer_and_keyboard_events_by_kernel_time() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let listen = Listen::new(tx);
        listen.logic.lock().set_toggle_modifiers("left_control".parse::<sayso_core::hotkey::Hotkey>().unwrap().modifier_keys());
        let down = InputEvent::new_now(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.0, 1);
        std::thread::sleep(std::time::Duration::from_millis(2));
        let click = InputEvent::new_now(EventType::KEY.0, KeyCode::BTN_LEFT.0, 1);
        std::thread::sleep(std::time::Duration::from_millis(2));
        let up = InputEvent::new_now(EventType::KEY.0, KeyCode::KEY_LEFTCTRL.0, 0);
        // Poll happens to read the keyboard before the mouse.
        dispatch_batch(&listen, vec![down, up, click]);
        assert!(rx.try_recv().is_err());
        dispatch_batch(&listen, vec![down, up]);
        assert_eq!(rx.try_recv(), Ok(sayso_platform::HotkeyEvent::Toggle));
    }

    #[test]
    fn mouse_buttons_touch_and_scroll_cancel_but_motion_does_not() {
        for (kind, code, value, expected) in [
            (EventType::KEY, KeyCode::BTN_LEFT.0, 1, true),
            (EventType::KEY, KeyCode::BTN_TOUCH.0, 1, true),
            (EventType::KEY, KeyCode::BTN_LEFT.0, 0, false),
            (EventType::KEY, KeyCode::KEY_A.0, 1, false),
            (EventType::RELATIVE, RelativeAxisCode::REL_WHEEL.0, -1, true),
            (EventType::RELATIVE, RelativeAxisCode::REL_HWHEEL_HI_RES.0, 120, true),
            (EventType::RELATIVE, RelativeAxisCode::REL_X.0, 10, false),
        ] {
            assert_eq!(is_pointer_activity(&InputEvent::new(kind.0, code, value)), expected);
        }
    }

    #[test]
    fn mice_and_touchpads_are_input_devices_too() {
        let keyboard: evdev::AttributeSet<KeyCode> = [KeyCode::KEY_A, KeyCode::KEY_Z, KeyCode::KEY_SPACE].into_iter().collect();
        let mouse: evdev::AttributeSet<KeyCode> = [KeyCode::BTN_LEFT].into_iter().collect();
        let touch: evdev::AttributeSet<KeyCode> = [KeyCode::BTN_TOUCH].into_iter().collect();
        let other: evdev::AttributeSet<KeyCode> = [KeyCode::KEY_POWER].into_iter().collect();
        assert!(is_input_device(Some(&keyboard), false));
        assert!(is_input_device(Some(&mouse), false));
        assert!(is_input_device(Some(&touch), false));
        assert!(is_input_device(None, true));
        assert!(!is_input_device(Some(&other), false));
    }
}
