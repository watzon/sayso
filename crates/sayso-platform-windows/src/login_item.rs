//! [`LoginItem`] with the `Run` key of the current user.
//!
//! Sayso starts at login when `HKCU\...\CurrentVersion\Run` has a "Sayso"
//! value with the quoted path of the exe. The user can turn the entry off in
//! Task Manager (Startup apps) or in Settings. Windows keeps that switch in
//! `Explorer\StartupApproved\Run`: a binary value whose first byte is even
//! (`0x02`) when the entry is on and odd (`0x03`) when it is off.
//!
//! Turning the item on in Sayso clears that switch, so the user's choice in
//! Sayso wins.

use crate::win32;
use sayso_platform::{LoginItem, LoginItemState, PlatformError, Result};
use std::path::Path;
use windows::Win32::System::Registry::HKEY_CURRENT_USER;

pub struct WinLoginItem;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const VALUE: &str = "Sayso";

/// The command line for the `Run` value: the quoted exe path.
pub fn run_command(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// The state from the `Run` value and the `StartupApproved` switch.
pub fn state_from(run: Option<&str>, approved: Option<&[u8]>) -> LoginItemState {
    let registered = run.is_some_and(|cmd| !cmd.trim().is_empty());
    let switched_off = approved.and_then(|bytes| bytes.first()).is_some_and(|b| b & 1 == 1);
    if registered && !switched_off { LoginItemState::Enabled } else { LoginItemState::Disabled }
}

impl LoginItem for WinLoginItem {
    fn state(&self) -> LoginItemState {
        let run = win32::read_string(HKEY_CURRENT_USER, RUN_KEY, VALUE);
        let approved = win32::read_binary(HKEY_CURRENT_USER, APPROVED_KEY, VALUE);
        state_from(run.as_deref(), approved.as_deref())
    }

    fn set_enabled(&self, enabled: bool) -> Result<LoginItemState> {
        let outcome = if enabled {
            std::env::current_exe()
                .map_err(|e| format!("cannot find the Sayso exe: {e}"))
                .and_then(|exe| win32::write_string(HKEY_CURRENT_USER, RUN_KEY, VALUE, &run_command(&exe)))
                .and_then(|()| win32::delete_value(HKEY_CURRENT_USER, APPROVED_KEY, VALUE))
        } else {
            win32::delete_value(HKEY_CURRENT_USER, RUN_KEY, VALUE)
                .and_then(|()| win32::delete_value(HKEY_CURRENT_USER, APPROVED_KEY, VALUE))
        };
        outcome.map_err(PlatformError::Failed)?;
        Ok(self.state())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_is_the_quoted_path() {
        assert_eq!(run_command(Path::new(r"C:\Program Files\Sayso\sayso.exe")), r#""C:\Program Files\Sayso\sayso.exe""#);
    }

    #[test]
    fn enabled_needs_the_run_value() {
        assert_eq!(state_from(None, None), LoginItemState::Disabled);
        assert_eq!(state_from(Some(""), None), LoginItemState::Disabled);
        assert_eq!(state_from(Some(r#""C:\sayso.exe""#), None), LoginItemState::Enabled);
    }

    #[test]
    fn task_manager_can_switch_it_off() {
        let run = Some(r#""C:\sayso.exe""#);
        let on = [0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let off = [0x03, 0, 0, 0, 0x8c, 0x3f, 0x1a, 0x2b, 0x55, 0x10, 0xd9, 0x01];
        assert_eq!(state_from(run, Some(&on)), LoginItemState::Enabled);
        assert_eq!(state_from(run, Some(&off)), LoginItemState::Disabled);
        assert_eq!(state_from(run, Some(&[])), LoginItemState::Enabled);
        // A switch without the Run value is still off.
        assert_eq!(state_from(None, Some(&on)), LoginItemState::Disabled);
    }

    /// Turns the login item on and off in the real registry, then puts back
    /// what was there before.
    #[test]
    #[ignore = "writes the Run key of the current user (restored after)"]
    fn round_trip_in_the_registry() {
        let run_before = win32::read_string(HKEY_CURRENT_USER, RUN_KEY, VALUE);
        let approved_before = win32::read_binary(HKEY_CURRENT_USER, APPROVED_KEY, VALUE);

        let on = WinLoginItem.set_enabled(true);
        let command = win32::read_string(HKEY_CURRENT_USER, RUN_KEY, VALUE);
        let off = WinLoginItem.set_enabled(false);
        let after_off = win32::read_string(HKEY_CURRENT_USER, RUN_KEY, VALUE);

        // Restore before asserting, so a failure leaves nothing behind.
        match &run_before {
            Some(cmd) => win32::write_string(HKEY_CURRENT_USER, RUN_KEY, VALUE, cmd).unwrap(),
            None => win32::delete_value(HKEY_CURRENT_USER, RUN_KEY, VALUE).unwrap(),
        }
        match &approved_before {
            Some(bytes) => win32::write_binary(HKEY_CURRENT_USER, APPROVED_KEY, VALUE, bytes).unwrap(),
            None => win32::delete_value(HKEY_CURRENT_USER, APPROVED_KEY, VALUE).unwrap(),
        }
        assert_eq!(win32::read_string(HKEY_CURRENT_USER, RUN_KEY, VALUE), run_before);

        assert_eq!(on, Ok(LoginItemState::Enabled));
        let exe = std::env::current_exe().unwrap();
        assert_eq!(command, Some(run_command(&exe)));
        assert_eq!(off, Ok(LoginItemState::Disabled));
        assert_eq!(after_off, None);
    }
}
