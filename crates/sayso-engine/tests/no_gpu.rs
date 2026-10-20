//! Without a discrete Vulkan GPU, a `parakeet_cpp` model fails to load with the reason `no_gpu`, and the
//! engine keeps running. `VK_DRIVER_FILES` names no driver, so this holds on a computer with a GPU too.

#![cfg(all(target_os = "linux", feature = "parakeet"))]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn a_gpu_model_without_a_gpu_fails_with_no_gpu() {
    let dir = tempfile::tempdir().unwrap();
    // A downloaded model as the engine leaves it: the GGUF in a folder named by its commit, and the marker.
    let folder = dir.path().join("m");
    std::fs::create_dir_all(folder.join("c0ffee")).unwrap();
    std::fs::write(folder.join("c0ffee").join("m.gguf"), b"not read: the GPU check comes first").unwrap();
    std::fs::write(folder.join(".complete"), "c0ffee\n").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_sayso-engine"))
        .env("SAYSO_MODELS_DIR", dir.path())
        .env("VK_DRIVER_FILES", dir.path().join("no-driver.json"))
        .env_remove("GGML_VK_VISIBLE_DEVICES")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let engine = r#"{"kind":"parakeet_cpp","repo":"o/r","revision":"c0ffee","file":"m.gguf","preview_archive":null}"#;
    writeln!(stdin, r#"{{"v":1,"type":"load","id":"l","model":"m","engine":{engine}}}"#).unwrap();
    // A load runs on its own thread: wait for its reply, since `shutdown` ends the engine at once.
    let mut replies = Vec::new();
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let reply: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
        let done = reply["id"] == "l" && matches!(reply["type"].as_str(), Some("ok" | "error"));
        replies.push(reply);
        if done {
            break;
        }
    }
    writeln!(stdin, r#"{{"v":1,"type":"shutdown","id":"x"}}"#).unwrap();
    assert!(child.wait().unwrap().success());
    let failed = replies
        .iter()
        .find(|r| r["type"] == "model_state" && r["state"] == "failed")
        .unwrap_or_else(|| panic!("no failed state in {replies:?}"));
    assert_eq!(failed["reason"], "no_gpu", "{failed}");
    assert_eq!(replies.last().unwrap()["type"], "error", "{replies:?}");
}
