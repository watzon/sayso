//! [`SystemPrefs`]: dark mode and reduce motion.
//!
//! Dark mode reads the `AppleInterfaceStyle` user default. macOS keeps it in
//! sync with the Appearance setting, including automatic switching, and
//! reading it works from any thread. `NSApp.effectiveAppearance` would need
//! the main thread and would follow an override that the app sets itself.

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSUserDefaults};
use sayso_platform::SystemPrefs;

pub struct MacPrefs;

/// `AppleInterfaceStyle` is "Dark" in dark mode and absent in light mode.
pub fn style_is_dark(style: Option<&str>) -> bool {
    style.is_some_and(|s| s.eq_ignore_ascii_case("dark"))
}

impl SystemPrefs for MacPrefs {
    fn dark_mode(&self) -> bool {
        let style = NSUserDefaults::standardUserDefaults().stringForKey(&NSString::from_str("AppleInterfaceStyle"));
        style_is_dark(style.map(|s| s.to_string()).as_deref())
    }

    fn reduce_motion(&self) -> bool {
        NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_dark_means_dark() {
        assert!(style_is_dark(Some("Dark")));
        assert!(!style_is_dark(None));
        assert!(!style_is_dark(Some("Light")));
    }
}
