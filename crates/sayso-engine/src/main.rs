//! `sayso-engine`: the speech engine sidecar for Linux and Windows.
//!
//! It reads NDJSON requests on stdin and writes NDJSON replies and events on
//! stdout (engine protocol v1). See the `sayso_engine` library.

use std::process::ExitCode;

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    sayso_engine::run()
}

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    eprintln!(
        "sayso-engine runs on Linux and Windows. On macOS, Sayso uses SaysoEngine \
         (native/macos/SaysoEngine)."
    );
    ExitCode::from(2)
}
