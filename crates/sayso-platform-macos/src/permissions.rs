//! [`PermissionGuide`]: microphone, Accessibility, and Input Monitoring.
//!
//! macOS cannot tell "denied" from "never asked" for Accessibility and Input
//! Monitoring through a preflight call, so a missing grant reports `Denied`.
//! Only the microphone reports `NotDetermined`.
//!
//! Requesting the microphone from a process that has no
//! `NSMicrophoneUsageDescription` in its Info.plist makes macOS kill the
//! process. Request only from the bundled app.

use crate::ffi::*;
use block2::RcBlock;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use objc2::runtime::Bool;
use objc2_app_kit::NSWorkspace;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_foundation::{NSString, NSURL};
use sayso_platform::{Permission, PermissionGuide, PermissionState};

pub struct MacPermissions;

/// System Settings page for a permission.
pub fn settings_url(permission: Permission) -> &'static str {
    match permission {
        Permission::Microphone => "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
        Permission::Accessibility => "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        Permission::InputMonitoring => "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
    }
}

pub(crate) fn microphone_status() -> PermissionState {
    // SAFETY: `AVMediaTypeAudio` is an AVFoundation constant.
    let status = unsafe {
        let Some(media) = AVMediaTypeAudio else { return PermissionState::NotDetermined };
        AVCaptureDevice::authorizationStatusForMediaType(media)
    };
    match status {
        AVAuthorizationStatus::Authorized => PermissionState::Granted,
        AVAuthorizationStatus::NotDetermined => PermissionState::NotDetermined,
        _ => PermissionState::Denied,
    }
}

fn request_microphone() {
    // SAFETY: the block is called once by AVFoundation on an arbitrary queue.
    unsafe {
        let Some(media) = AVMediaTypeAudio else { return };
        let handler = RcBlock::new(|granted: Bool| log::info!("microphone access granted: {}", granted.as_bool()));
        AVCaptureDevice::requestAccessForMediaType_completionHandler(media, &handler);
    }
}

/// Ask for Accessibility. Shows the system prompt when the grant is missing.
fn request_accessibility() {
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let options: CFDictionary<CFString, CFType> =
        CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value().as_CFType())]);
    // SAFETY: `options` lives for the call.
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef().cast()) };
}

impl PermissionGuide for MacPermissions {
    fn status(&self, permission: Permission) -> PermissionState {
        // SAFETY: plain queries, no arguments.
        let granted = |g: bool| if g { PermissionState::Granted } else { PermissionState::Denied };
        match permission {
            Permission::Microphone => microphone_status(),
            Permission::Accessibility => granted(unsafe { AXIsProcessTrusted() }),
            Permission::InputMonitoring => granted(unsafe { CGPreflightListenEventAccess() }),
        }
    }

    fn request(&self, permission: Permission) {
        match permission {
            Permission::Microphone => match microphone_status() {
                PermissionState::NotDetermined => request_microphone(),
                PermissionState::Denied => self.open_settings(permission),
                PermissionState::Granted => {}
            },
            Permission::Accessibility => request_accessibility(),
            Permission::InputMonitoring => {
                // SAFETY: shows the system prompt the first time, then does nothing.
                // After that the UI should call `open_settings`.
                unsafe { CGRequestListenEventAccess() };
            }
        }
    }

    fn open_settings(&self, permission: Permission) {
        let url = NSURL::URLWithString(&NSString::from_str(settings_url(permission)));
        match url {
            Some(url) => {
                if !NSWorkspace::sharedWorkspace().openURL(&url) {
                    log::warn!("could not open {url:?}");
                }
            }
            None => log::error!("bad settings URL for {permission:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_links_name_the_right_pane() {
        assert!(settings_url(Permission::Microphone).ends_with("Privacy_Microphone"));
        assert!(settings_url(Permission::Accessibility).ends_with("Privacy_Accessibility"));
        assert!(settings_url(Permission::InputMonitoring).ends_with("Privacy_ListenEvent"));
    }
}
