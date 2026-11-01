//! `sayso-bridge`: relays connected apps to a Sayso in a Flatpak (`docs/sdk.md`,
//! "Flatpak"). Sayso starts it with `flatpak-spawn --host`.
//!
//! `sayso-bridge --relay-fd=<fd> --socket=<path>`

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    use std::os::fd::{FromRawFd, OwnedFd};
    let mut relay = None;
    let mut socket = None;
    for arg in std::env::args_os().skip(1) {
        let arg = arg.to_string_lossy().into_owned();
        if let Some(fd) = arg.strip_prefix("--relay-fd=") {
            relay = fd.parse::<i32>().ok();
        } else if let Some(path) = arg.strip_prefix("--socket=") {
            socket = Some(std::path::PathBuf::from(path));
        }
    }
    let (Some(relay), Some(socket)) = (relay, socket) else {
        eprintln!("usage: sayso-bridge --relay-fd=<fd> --socket=<path>. Only Sayso starts this program.");
        return std::process::ExitCode::from(2);
    };
    // SAFETY: Sayso gives this number, and nothing else in this process owns it.
    let relay = unsafe { OwnedFd::from_raw_fd(relay) };
    match sayso_bridge::serve::run(relay, &socket) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => std::process::ExitCode::from(sayso_bridge::serve::EXIT_IN_USE),
        Err(e) => {
            eprintln!("sayso-bridge: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("sayso-bridge runs only on Linux");
    std::process::exit(2);
}
