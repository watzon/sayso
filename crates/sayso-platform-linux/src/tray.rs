//! The tray icon, through the StatusNotifierItem D-Bus protocol (KDE, most
//! panels, and GNOME with the AppIndicator extension).
//!
//! A primary click opens Sayso's own popover. The icon also has a small
//! native menu, because some panels (GNOME's AppIndicator extension) show
//! only the menu.

use ksni::blocking::TrayMethods as _;

/// What happened at the tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// A primary click at (x, y) X11 pixels, top-left origin. (0, 0) when the
    /// panel does not say where.
    Click { x: i32, y: i32 },
    /// "Start or stop dictation" from the menu.
    ToggleDictation,
    OpenHub,
    Quit,
}

type Handler = Box<dyn Fn(TrayEvent) + Send + Sync>;

struct SaysoTray {
    /// ARGB32 in network byte order, as the protocol wants.
    icon: Option<ksni::Icon>,
    on_event: Handler,
}

impl ksni::Tray for SaysoTray {
    fn id(&self) -> String {
        "sayso".into()
    }

    fn title(&self) -> String {
        "Sayso".into()
    }

    fn icon_name(&self) -> String {
        // A panel prefers a named icon over the pixmap, so name one only
        // when there is no pixmap.
        if self.icon.is_some() { String::new() } else { "audio-input-microphone-symbolic".into() }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone().into_iter().collect()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: "Sayso".into(), ..Default::default() }
    }

    fn activate(&mut self, x: i32, y: i32) {
        (self.on_event)(TrayEvent::Click { x, y });
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Start or Stop Dictation".into(),
                activate: Box::new(|t: &mut Self| (t.on_event)(TrayEvent::ToggleDictation)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Open Sayso".into(),
                activate: Box::new(|t: &mut Self| (t.on_event)(TrayEvent::OpenHub)),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit Sayso".into(),
                activate: Box::new(|t: &mut Self| (t.on_event)(TrayEvent::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Keeps the tray icon. Dropping it removes the icon.
pub struct Tray {
    _handle: ksni::blocking::Handle<SaysoTray>,
}

impl Tray {
    /// Show the icon. `rgba` is the image, row by row, `width` by `height`.
    /// `on_event` runs on the tray's own thread. None when no tray host runs
    /// (for example GNOME without the AppIndicator extension).
    pub fn install(rgba: Option<(Vec<u8>, u32, u32)>, on_event: impl Fn(TrayEvent) + Send + Sync + 'static) -> Option<Tray> {
        let icon = rgba.map(|(data, width, height)| ksni::Icon {
            width: width as i32,
            height: height as i32,
            data: rgba_to_argb(&data),
        });
        let tray = SaysoTray { icon, on_event: Box::new(on_event) };
        // A sandbox cannot own the bus name of a tray item, and the item works without it.
        match tray.disable_dbus_name(sayso_core::flatpak::app_id().is_some()).spawn() {
            Ok(handle) => Some(Tray { _handle: handle }),
            Err(e) => {
                log::warn!("no tray icon: {e}");
                None
            }
        }
    }
}

/// RGBA bytes to ARGB bytes (the order the protocol uses).
pub fn rgba_to_argb(rgba: &[u8]) -> Vec<u8> {
    let (pixels, _) = rgba.as_chunks::<4>();
    pixels.iter().flat_map(|&[r, g, b, a]| [a, r, g, b]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_move_alpha_to_the_front() {
        assert_eq!(rgba_to_argb(&[1, 2, 3, 4, 5, 6, 7, 8]), vec![4, 1, 2, 3, 8, 5, 6, 7]);
    }
}
