//! The list of input devices, kept in the model.
//!
//! Asking the audio system for the devices is a full scan (CoreAudio on
//! macOS), so no view does it while it draws. Views read the cache. A scan
//! runs off the UI thread when a view that shows devices opens, and every
//! few seconds while at least one such view is visible, so a plugged-in
//! microphone shows up.

use crate::model::AppModel;
use gpui_kit::*;
use sayso_platform::AudioDevice;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// How often the list refreshes while a view shows it.
const POLL: Duration = Duration::from_millis(2500);

/// The cached list and the state of its refresh.
#[derive(Default)]
pub struct InputDevices {
    list: Vec<AudioDevice>,
    /// A scan runs now.
    scanning: bool,
    /// The poll loop runs.
    polling: bool,
    /// The number of live [`DeviceWatch`] values.
    watchers: Rc<Cell<usize>>,
}

impl InputDevices {
    /// Keep the result of a scan. True when the list changed.
    fn store(&mut self, list: Vec<AudioDevice>) -> bool {
        let changed = self.list != list;
        if changed {
            self.list = list;
        }
        changed
    }
}

/// A view that shows the device list holds one of these while it is visible.
/// The list refreshes at a slow rate until the last one drops.
pub struct DeviceWatch(Rc<Cell<usize>>);

impl Drop for DeviceWatch {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

/// A watch that a view takes and gives back as it appears and goes away.
#[derive(Default)]
pub struct ViewWatch(Option<DeviceWatch>);

impl ViewWatch {
    /// Hold a watch while `visible`, and none otherwise.
    pub fn set(&mut self, visible: bool, model: &Entity<AppModel>, cx: &mut App) {
        match (visible, self.0.is_some()) {
            (true, false) => self.0 = Some(model.update(cx, |m, cx| m.watch_input_devices(cx))),
            (false, true) => self.0 = None,
            _ => {}
        }
    }
}

impl AppModel {
    /// The input devices at the last scan. It reads the cache and never scans.
    pub fn input_devices(&self) -> Vec<AudioDevice> {
        self.devices.list.clone()
    }

    /// Scan for devices on a background thread. The views draw again when the list changed.
    pub fn refresh_input_devices(&mut self, cx: &mut Context<Self>) {
        if self.devices.scanning {
            return;
        }
        self.devices.scanning = true;
        let audio = self.services.platform.audio.clone();
        cx.spawn(async move |this, cx| {
            let list = cx.background_spawn(async move { audio.devices() }).await;
            let _ = this.update(cx, |m, cx| {
                m.devices.scanning = false;
                if m.devices.store(list) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Refresh the list now, and every few seconds until the returned watch
    /// and all others are dropped.
    pub fn watch_input_devices(&mut self, cx: &mut Context<Self>) -> DeviceWatch {
        let watchers = self.devices.watchers.clone();
        watchers.set(watchers.get() + 1);
        self.refresh_input_devices(cx);
        if !self.devices.polling {
            self.devices.polling = true;
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(POLL).await;
                    let again = this.update(cx, |m, cx| {
                        if m.devices.watchers.get() == 0 {
                            m.devices.polling = false;
                            return false;
                        }
                        m.refresh_input_devices(cx);
                        true
                    });
                    if !matches!(again, Ok(true)) {
                        break;
                    }
                }
            })
            .detach();
        }
        DeviceWatch(watchers)
    }
}

#[cfg(test)]
mod tests {
    use super::{DeviceWatch, InputDevices};
    use sayso_platform::AudioDevice;
    use std::cell::Cell;
    use std::rc::Rc;

    fn device(id: &str) -> AudioDevice {
        AudioDevice { id: id.into(), name: id.into(), is_default: false }
    }

    #[test]
    fn a_scan_that_finds_the_same_devices_changes_nothing() {
        let mut devices = InputDevices::default();
        assert!(devices.store(vec![device("a")]));
        assert!(!devices.store(vec![device("a")]));
        assert!(devices.store(vec![device("a"), device("b")]));
        assert!(devices.store(Vec::new()));
    }

    #[test]
    fn the_last_watch_to_drop_stops_the_polling() {
        let count = Rc::new(Cell::new(2));
        let (a, b) = (DeviceWatch(count.clone()), DeviceWatch(count.clone()));
        drop(a);
        assert_eq!(count.get(), 1);
        drop(b);
        assert_eq!(count.get(), 0);
    }
}
