//! Link options of the engine.
//!
//! The ONNX Runtime build of `ort-sys` for Windows contains the DirectML
//! execution provider, so the exe imports `DirectML.dll`. The engine runs on
//! the CPU and never creates that provider, so the import is delay-loaded:
//! Windows loads the DLL only at the first call into it, and the exe starts
//! without it. See NOTES.md.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if windows && msvc {
        // The exe only: the test binaries that do not call ONNX Runtime have
        // no DirectML import, and the linker warns about the option there.
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:DirectML.dll");
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
}
