//! [`LoginItem`] with `SMAppService.mainApp` (macOS 13 and later).
//!
//! Only a bundled app can register. For an unbundled binary the status is
//! `NotFound`, which maps to `Disabled`, and `register` returns an error.

use objc2_service_management::{SMAppService, SMAppServiceStatus};
use sayso_platform::{LoginItem, LoginItemState, PlatformError, Result};

pub struct MacLoginItem;

pub fn map_status(status: SMAppServiceStatus) -> LoginItemState {
    match status {
        SMAppServiceStatus::Enabled => LoginItemState::Enabled,
        SMAppServiceStatus::RequiresApproval => LoginItemState::RequiresApproval,
        // NotRegistered, NotFound, and any future value.
        _ => LoginItemState::Disabled,
    }
}

impl LoginItem for MacLoginItem {
    fn state(&self) -> LoginItemState {
        // SAFETY: plain AppKit queries with no preconditions.
        map_status(unsafe { SMAppService::mainAppService().status() })
    }

    fn set_enabled(&self, enabled: bool) -> Result<LoginItemState> {
        // SAFETY: as above.
        let service = unsafe { SMAppService::mainAppService() };
        let outcome = unsafe {
            if enabled { service.registerAndReturnError() } else { service.unregisterAndReturnError() }
        };
        match outcome {
            Ok(()) => Ok(self.state()),
            // Registering twice, or unregistering what is not registered, is not a failure for us.
            Err(err) if self.state() == if enabled { LoginItemState::Enabled } else { LoginItemState::Disabled } => {
                log::debug!("login item call returned {err:?} but the state is already right");
                Ok(self.state())
            }
            Err(err) => Err(PlatformError::Failed(err.localizedDescription().to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_ui_states() {
        assert_eq!(map_status(SMAppServiceStatus::Enabled), LoginItemState::Enabled);
        assert_eq!(map_status(SMAppServiceStatus::RequiresApproval), LoginItemState::RequiresApproval);
        assert_eq!(map_status(SMAppServiceStatus::NotRegistered), LoginItemState::Disabled);
        assert_eq!(map_status(SMAppServiceStatus::NotFound), LoginItemState::Disabled);
    }
}
