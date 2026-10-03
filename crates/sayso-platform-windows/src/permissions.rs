//! [`PermissionGuide`]: the microphone privacy switches.
//!
//! Windows asks no permission for Accessibility or Input Monitoring: any
//! desktop program may send keys and install a keyboard hook. So both
//! report `Granted`.
//!
//! The microphone has switches in Settings > Privacy & security > Microphone.
//! Windows keeps them in the consent store of the registry:
//! - the machine switch ("Microphone access", set by an administrator) in HKLM,
//! - the user switch ("Microphone access") in HKCU,
//! - "Let desktop apps access your microphone" in HKCU, under `NonPackaged`.
//!
//! Each holds "Allow" or "Deny". One "Deny" is enough to silence Sayso.
//! Desktop apps get no prompt, so there is no `NotDetermined`.

use crate::win32;
use sayso_platform::{Permission, PermissionGuide, PermissionState};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::PCWSTR;

pub struct WinPermissions;

const CONSENT_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

/// Settings page for a permission. Only the microphone has one.
pub fn settings_uri(permission: Permission) -> Option<&'static str> {
    match permission {
        Permission::Microphone => Some("ms-settings:privacy-microphone"),
        Permission::Accessibility | Permission::InputMonitoring => None,
    }
}

/// The microphone state from the three switches. A missing value means the
/// switch was never touched, and its default is "Allow".
pub fn combine(switches: &[Option<String>]) -> PermissionState {
    let denied = switches.iter().flatten().any(|v| v.trim().eq_ignore_ascii_case("deny"));
    if denied { PermissionState::Denied } else { PermissionState::Granted }
}

pub(crate) fn microphone_status() -> PermissionState {
    let nonpackaged = format!(r"{CONSENT_KEY}\NonPackaged");
    combine(&[
        win32::read_string(HKEY_LOCAL_MACHINE, CONSENT_KEY, "Value"),
        win32::read_string(HKEY_CURRENT_USER, CONSENT_KEY, "Value"),
        win32::read_string(HKEY_CURRENT_USER, &nonpackaged, "Value"),
    ])
}

/// Open a URI with the shell, for example a Settings page.
pub(crate) fn open_uri(uri: &str) {
    let (verb, file) = (win32::wide("open"), win32::wide(uri));
    // SAFETY: the strings are NUL-terminated and live for the call.
    let result = unsafe {
        ShellExecuteW(None, PCWSTR(verb.as_ptr()), PCWSTR(file.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL)
    };
    // ShellExecuteW returns a value above 32 on success.
    if result.0 as usize <= 32 {
        log::warn!("could not open {uri} (code {})", result.0 as usize);
    }
}

impl PermissionGuide for WinPermissions {
    fn status(&self, permission: Permission) -> PermissionState {
        match permission {
            Permission::Microphone => microphone_status(),
            Permission::Accessibility | Permission::InputMonitoring => PermissionState::Granted,
        }
    }

    /// Windows has no microphone prompt for desktop apps, so a missing
    /// microphone opens Settings. The other two need nothing.
    fn request(&self, permission: Permission) {
        if self.status(permission) != PermissionState::Granted {
            self.open_settings(permission);
        }
    }

    fn open_settings(&self, permission: Permission) {
        if let Some(uri) = settings_uri(permission) {
            open_uri(uri);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn one_deny_is_enough() {
        assert_eq!(combine(&[s("Allow"), s("Allow"), s("Allow")]), PermissionState::Granted);
        assert_eq!(combine(&[s("Allow"), s("Deny"), s("Allow")]), PermissionState::Denied);
        assert_eq!(combine(&[None, None, s("Deny")]), PermissionState::Denied);
        assert_eq!(combine(&[s("Deny"), None, None]), PermissionState::Denied);
    }

    #[test]
    fn missing_switches_mean_allowed() {
        assert_eq!(combine(&[None, None, None]), PermissionState::Granted);
        assert_eq!(combine(&[]), PermissionState::Granted);
        assert_eq!(combine(&[s("deny ")]), PermissionState::Denied, "case and spaces do not matter");
        assert_eq!(combine(&[s("Prompt")]), PermissionState::Granted);
    }

    #[test]
    fn only_the_microphone_has_a_settings_page() {
        assert_eq!(settings_uri(Permission::Microphone), Some("ms-settings:privacy-microphone"));
        assert_eq!(settings_uri(Permission::Accessibility), None);
        assert_eq!(settings_uri(Permission::InputMonitoring), None);
    }

    #[test]
    fn accessibility_and_input_monitoring_are_always_granted() {
        assert_eq!(WinPermissions.status(Permission::Accessibility), PermissionState::Granted);
        assert_eq!(WinPermissions.status(Permission::InputMonitoring), PermissionState::Granted);
    }
}
