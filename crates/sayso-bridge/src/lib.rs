//! The checks of the host that also run outside the Flatpak sandbox
//! (`docs/sdk.md`, "Flatpak"). `sayso-host` uses this code when Sayso is
//! outside a sandbox. The program `sayso-bridge` uses the same code for a Sayso
//! in a Flatpak, because a host in a sandbox cannot see a process outside it.
//!
//! This crate has no `workspace-hack`, and it needs only libc and `serde_json`.
//! So `scripts/bundle-linux.sh` can link the program statically, and it runs on
//! any Linux, with no library of the system.

#[cfg(target_os = "linux")]
pub mod credentials;
#[cfg(target_os = "linux")]
pub mod files;
#[cfg(target_os = "linux")]
pub mod identity;
#[cfg(unix)]
pub mod listen;
#[cfg(target_os = "linux")]
pub mod serve;
#[cfg(target_os = "linux")]
pub mod wire;
