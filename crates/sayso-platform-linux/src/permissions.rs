//! [`PermissionGuide`] on Linux.
//!
//! Linux has no per-app permission for these, so each one is a capability:
//! - Microphone: always granted (audio servers do not gate apps outside a sandbox).
//! - Accessibility: Sayso can send key presses to other apps (X11, `/dev/uinput`,
//!   or the remote desktop portal). See [`crate::inserter::injection_state`].
//! - Input Monitoring: Sayso can watch the keyboard without taking keys, for
//!   push-to-talk and double Esc. See [`crate::hotkeys::input_monitoring_state`].
//!
//! "Open settings" opens the Linux setup guide, which has the commands.

use crate::session::Session;
use sayso_platform::{Permission, PermissionGuide, PermissionState};

/// The setup guide for the permissions on Linux.
pub const GUIDE_URL: &str = "https://github.com/watzon/sayso/blob/main/docs/linux.md#permissions";

pub struct LinuxPermissions {
    session: &'static Session,
}

impl LinuxPermissions {
    pub fn new(session: &'static Session) -> Self {
        Self { session }
    }
}

impl PermissionGuide for LinuxPermissions {
    fn status(&self, permission: Permission) -> PermissionState {
        match permission {
            Permission::Microphone => PermissionState::Granted,
            Permission::Accessibility => crate::inserter::injection_state(self.session),
            Permission::InputMonitoring => crate::hotkeys::input_monitoring_state(self.session),
        }
    }

    fn request(&self, permission: Permission) {
        match permission {
            Permission::Microphone => {}
            Permission::Accessibility => crate::inserter::request_injection(self.session),
            Permission::InputMonitoring => self.open_settings(permission),
        }
    }

    fn open_settings(&self, _permission: Permission) {
        open_uri(self.session, GUIDE_URL);
    }
}

/// Open a file or a web page with the user's default app.
pub fn open_uri(session: &Session, target: impl AsRef<std::ffi::OsStr>) {
    let mut command = std::process::Command::new("xdg-open");
    command.arg(target);
    session.restore_env(&mut command);
    if let Err(e) = command.spawn() {
        log::warn!("cannot run xdg-open: {e}");
    }
}
