//! macOS: cpal imports the process tap functions of macOS 14.2 by name, and a strong import
//! stops a program at launch on 14.0 and 14.1. A library cannot set a link flag for the
//! program of another package, so each app needs this line in its own `build.rs` (see the
//! README). This sets it for the examples and the tests of this crate, and the test
//! `macos_launch` proves that it makes the imports weak.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-examples=-Wl,-weak_framework,CoreAudio");
        println!("cargo:rustc-link-arg-tests=-Wl,-weak_framework,CoreAudio");
    }
}
