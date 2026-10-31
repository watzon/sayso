//! The line in the README makes the process tap functions of macOS 14.2 weak imports, so an
//! app starts on 14.0 and 14.1. `build.rs` of this crate sets it for this test program. This
//! reads the imports of the program, and it starts no capture.
#![cfg(target_os = "macos")]

use sayso_sdk::protocol::{SessionReply, SessionStart};
use sayso_sdk::{Error, Speech};
use std::process::Command;

#[test]
fn the_link_flag_makes_the_tap_functions_weak() {
    // The linker keeps the local mode and its capture in this program.
    std::hint::black_box(Speech::start_session as fn(&Speech, SessionStart) -> Result<SessionReply, Error>);

    let output = Command::new("nm").arg("-m").arg(std::env::current_exe().unwrap()).output().expect("nm of the Xcode tools runs");
    assert!(output.status.success());
    let table = String::from_utf8_lossy(&output.stdout);
    let taps: Vec<&str> = table.lines().filter(|line| line.contains("ProcessTap") && line.contains("undefined")).collect();
    assert!(!taps.is_empty(), "the capture is in this program");
    assert!(taps.iter().all(|line| line.contains("weak")), "{taps:?}");
}
