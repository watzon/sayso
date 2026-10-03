//! [`SystemPrefs`]: dark mode and reduce motion.
//!
//! Dark mode reads `AppsUseLightTheme` in the Personalize key of the current
//! user: 0 is dark. It is the "Choose your app mode" setting, which Windows
//! keeps apart from the mode of the taskbar and Start. Reading it works from
//! any thread.
//!
//! Reduce motion is the "Animation effects" switch in Settings >
//! Accessibility > Visual effects (`SPI_GETCLIENTAREAANIMATION`).

use crate::win32;
use sayso_platform::SystemPrefs;
use windows::Win32::System::Registry::HKEY_CURRENT_USER;
use windows::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW};
use windows::core::BOOL;

pub struct WinPrefs;

const PERSONALIZE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

/// `AppsUseLightTheme` is 0 in dark mode. A missing value means light mode.
pub fn apps_are_dark(apps_use_light_theme: Option<u32>) -> bool {
    apps_use_light_theme == Some(0)
}

impl SystemPrefs for WinPrefs {
    fn dark_mode(&self) -> bool {
        apps_are_dark(win32::read_dword(HKEY_CURRENT_USER, PERSONALIZE_KEY, "AppsUseLightTheme"))
    }

    fn reduce_motion(&self) -> bool {
        let mut animations = BOOL(1);
        // SAFETY: the action writes one BOOL to the pointer.
        let read = unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                Some((&mut animations as *mut BOOL).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        read.is_ok() && !animations.as_bool()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_zero_means_dark() {
        assert!(apps_are_dark(Some(0)));
        assert!(!apps_are_dark(Some(1)));
        assert!(!apps_are_dark(None));
    }
}
