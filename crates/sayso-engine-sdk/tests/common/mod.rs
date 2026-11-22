//! Find a real engine for the tests. The tests start it as a process, as an app would.

#![allow(dead_code)]

use sayso_engine_sdk::{Engine, EngineOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The engine for this system:
///
/// 1. The program in `SAYSO_ENGINE`, for example the engine of a release file.
/// 2. macOS: the Swift engine, built with `swift build -c release` when it is not there.
/// 3. Linux and Windows: `sayso-engine` of this workspace, built with
///    `CGO_ENABLED=0 go build ./cmd/sayso-engine`. That build needs no GPU toolchain
///    and no parakeet.cpp. The engine is GPL, so the SDK (MIT) has no dependency on it:
///    the tests only run its program.
pub fn engine_program() -> &'static Path {
    static PROGRAM: OnceLock<PathBuf> = OnceLock::new();
    PROGRAM.get_or_init(|| {
        if let Some(program) = std::env::var_os("SAYSO_ENGINE") {
            return PathBuf::from(program);
        }
        if cfg!(target_os = "macos") { build_swift_engine() } else { build_go_engine() }
    })
}

fn build_swift_engine() -> PathBuf {
    let package = workspace_root().join("native/macos/SaysoEngine");
    let program = package.join(".build/release/SaysoEngine");
    if !program.is_file() {
        let status = Command::new("swift").args(["build", "-c", "release", "--package-path"]).arg(&package).status().expect("swift is not installed. Install Xcode, or set SAYSO_ENGINE");
        assert!(status.success(), "swift build failed");
    }
    program.canonicalize().expect("the Swift engine exists after the build")
}

fn build_go_engine() -> PathBuf {
    let go = std::env::var_os("GO").unwrap_or_else(|| "go".into());
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target"));
    std::fs::create_dir_all(&target_dir).expect("cannot create target dir for sayso-engine");
    let mut out = target_dir.join("sayso-engine");
    if cfg!(windows) {
        out.set_extension("exe");
    }
    let status = Command::new(go)
        .args(["build", "-o"])
        .arg(&out)
        .arg("./cmd/sayso-engine")
        .env("CGO_ENABLED", "0")
        .current_dir(workspace_root())
        .status()
        .expect("cannot run go; install Go or set SAYSO_ENGINE");
    assert!(status.success(), "go build ./cmd/sayso-engine failed");
    out.canonicalize().expect("the Go engine exists after the build")
}

/// Start the engine with an empty models folder. Keep the folder as long as the engine runs.
pub fn start_empty() -> (Engine, tempfile::TempDir) {
    let models = tempfile::tempdir().unwrap();
    let engine = Engine::start(EngineOptions::new(engine_program(), models.path())).unwrap_or_else(|e| panic!("cannot start {}: {e}", engine_program().display()));
    (engine, models)
}
