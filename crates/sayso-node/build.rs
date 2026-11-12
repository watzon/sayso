//! The addon is a library that Node loads, so its link flags are its own, and the apps
//! need no `build.rs` (unlike a Rust app of `sayso-sdk`).
//!
//! macOS: cpal imports the process tap functions of macOS 14.2 by name, and a strong import
//! stops the load of the addon on 14.0 and 14.1. So CoreAudio is a weak framework here.
//! `scripts/check-macho.py --weak-coreaudio` checks the result in CI.

fn main() {
    napi_build::setup();
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-weak_framework,CoreAudio");
    }
}
