//! The app learns why its engine did not start. The fake engine is a program that writes what
//! the dynamic loader of Linux writes for a missing library, and ends with status 127.

#![cfg(unix)]

use sayso_sdk::embedded::{EngineModels, ProgramEngine};
use std::os::unix::fs::PermissionsExt;

const LOADER_LINE: &str = "error while loading shared libraries: libfake.so.1: cannot open shared object file: No such file or directory";

#[test]
fn the_error_of_an_engine_that_ends_at_once_has_the_last_lines_of_its_stderr() {
    let folder = tempfile::tempdir().unwrap();
    let program = folder.path().join("fake-engine");
    std::fs::write(&program, format!("#!/bin/sh\necho '{LOADER_LINE}' >&2\nexit 127\n")).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();

    let engine = ProgramEngine::new(program, folder.path().join("models"), None);
    let error = engine.catalog().expect_err("the engine ended, so it did not start");
    assert_eq!(error.message, format!("the engine has ended. The engine wrote: {LOADER_LINE}"));
}
