//! What an app learns when the engine program cannot start. The fake engine is a shell
//! script that writes what the dynamic loader of Linux writes for a missing library, and ends.

#![cfg(unix)]

use sayso_engine_sdk::{Engine, EngineOptions, Error, Stderr, StderrTail};
use std::sync::{Arc, Mutex};

const LOADER_LINE: &str = "error while loading shared libraries: libfake.so.1: cannot open shared object file: No such file or directory";

/// Start `/bin/sh` with a script, so the test never runs a file that it just wrote.
fn start_script(script: &str, stderr: Stderr) -> (Result<Engine, Error>, tempfile::TempDir) {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("engine.sh");
    std::fs::write(&path, script).unwrap();
    let options = EngineOptions::new("/bin/sh", folder.path().join("models")).arg(&path).stderr(stderr);
    (Engine::start(options), folder)
}

fn loader_failure() -> String {
    format!("echo '{LOADER_LINE}' >&2\nexit 127\n")
}

#[test]
fn the_start_error_of_an_engine_that_ends_at_once_has_what_it_wrote() {
    let tail = StderrTail::new();
    let forwarded = Arc::new(Mutex::new(Vec::new()));
    let forwarded_in = forwarded.clone();
    let (started, _folder) = start_script(&loader_failure(), tail.stderr(move |line| forwarded_in.lock().unwrap().push(line.to_owned())));
    let error = started.expect_err("the engine ended, so it did not start");
    assert!(matches!(error, Error::Exited), "got {error:?}");
    assert_eq!(tail.describe(&error), format!("the engine has ended. The engine wrote: {LOADER_LINE}"));
    // The lines still go to the function of the app.
    assert_eq!(*forwarded.lock().unwrap(), [LOADER_LINE]);
}

#[test]
fn without_a_tail_the_error_has_no_reason() {
    let (started, _folder) = start_script(&loader_failure(), Stderr::Null);
    assert_eq!(started.expect_err("the engine ended").to_string(), "the engine has ended");
}

#[test]
fn an_engine_that_writes_a_lot_gives_the_last_lines_only() {
    let tail = StderrTail::new();
    let script = "i=1\nwhile [ $i -le 20 ]; do echo \"line $i\" >&2; i=$((i + 1)); done\nexit 1\n";
    let (started, _folder) = start_script(script, tail.stderr(|_| {}));
    let message = tail.describe(&started.expect_err("the engine ended"));
    let expected: Vec<String> = (13..=20).map(|n| format!("line {n}")).collect();
    assert_eq!(message, format!("the engine has ended. The engine wrote: {}", expected.join(" | ")));
}
