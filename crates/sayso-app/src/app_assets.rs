//! The tray icon and the window icon. They belong to the app, not to `sayso-ui`:
//! the name Sayso and the app icon stay with Watzon Ventures LLC, and `sayso-ui`
//! is MIT-licensed and published.

use std::borrow::Cow;

#[derive(rust_embed::RustEmbed)]
#[folder = "../../assets/app"]
#[include = "tray.png"]
#[include = "icon-64.png"]
struct AppAssets;

/// Raw bytes of an app image: `tray.png` or `icon-64.png`.
pub fn bytes(name: &str) -> Option<Cow<'static, [u8]>> {
    AppAssets::get(name).map(|f| f.data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_the_tray_and_window_icons_only() {
        assert!(bytes("tray.png").is_some());
        assert!(bytes("icon-64.png").is_some());
        // The iconset is for the bundle scripts. Embedding it adds over 1 MB.
        assert!(AppAssets::iter().all(|name| name == "tray.png" || name == "icon-64.png"));
    }
}
