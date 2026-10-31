//! The SDK cannot use `sayso_core::paths`, so it has its own copy of the rules for the model
//! store. This test compares the two. It computes paths, and it touches no folder.

use sayso_core::paths::{Paths, SystemEnv};
use std::process::{Command, Stdio};

#[test]
fn the_store_of_the_sdk_is_the_store_of_sayso() {
    assert_eq!(sayso_sdk::model_store(), Paths::resolve(&SystemEnv).models_dir().join("v1"));
    if std::env::var_os("SDK_TEST_PATHS_CHILD").is_some() {
        return;
    }
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "the_store_of_the_sdk_is_the_store_of_sayso"])
        .env("SDK_TEST_PATHS_CHILD", "yes")
        .env("XDG_DATA_HOME", std::env::temp_dir().join("sayso-sdk-paths-test"))
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "the same with XDG_DATA_HOME");
}
