//! The engine ends with `_exit`, without the C++ global destructors. Its reply to `shutdown`
//! must still arrive, and the exit status stays success.

#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn shutdown_is_answered_and_the_engine_exits_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_sayso-engine"))
        .env("SAYSO_MODELS_DIR", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"v":1,"type":"hello","id":"h"}}"#).unwrap();
    writeln!(stdin, r#"{{"v":1,"type":"shutdown","id":"x"}}"#).unwrap();
    let replies: Vec<serde_json::Value> = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
        .collect();
    assert!(child.wait().unwrap().success());
    assert!(
        replies.iter().any(|r| r["id"] == "x" && r["type"] == "ok"),
        "no reply to shutdown in {replies:?}"
    );
}
