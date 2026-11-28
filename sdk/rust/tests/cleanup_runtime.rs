//! The runtime client against `sayso-fake-cleanup`, the MIT fake of the cleanup protocol v1
//! (scenario C03).
//!
//! The client has no way to give the runtime an environment, so a test that injects a fault sets
//! `SAYSO_FAKE_SCRIPT` in this process. The tests of this file run one at a time because of that.
#![cfg(feature = "cleanup")]

mod common;

use sayso_sdk::cleanup::client::runtime::{Runtime, RuntimeError};
use sayso_sdk::cleanup::models::cpu::Level;
use sayso_sdk::cleanup::models::protocol::{ErrorCode, Stop};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// A runtime on the fake, with `script` in `SAYSO_FAKE_SCRIPT` until the guard drops.
struct Fake {
    runtime: Arc<Runtime>,
    program: PathBuf,
    folder: tempfile::TempDir,
    _one: MutexGuard<'static, ()>,
}

impl Fake {
    fn start(script: Option<&str>) -> Option<Fake> {
        let one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let program = common::fake_cleanup()?;
        set_script(script);
        let folder = tempfile::tempdir().unwrap();
        Some(Fake {
            runtime: Runtime::new(program.clone()),
            program,
            folder,
            _one: one,
        })
    }

    /// A model file: the fake wants a regular file at the path of `load`.
    fn model(&self, name: &str) -> PathBuf {
        let path = self.folder.path().join(name);
        std::fs::write(&path, b"gguf").unwrap();
        path
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.runtime.stop();
        set_script(None);
    }
}

fn set_script(script: Option<&str>) {
    // Nothing else reads this variable in this process, and every test of this file holds ONE_AT_A_TIME.
    unsafe {
        match script {
            Some(script) => std::env::set_var("SAYSO_FAKE_SCRIPT", script),
            None => std::env::remove_var("SAYSO_FAKE_SCRIPT"),
        }
    }
}

#[test]
fn hello_says_what_the_runtime_is() {
    let Some(fake) = Fake::start(None) else {
        return;
    };
    let info = fake.runtime.hello().unwrap();
    assert_eq!(
        (
            info.version,
            info.backend.as_str(),
            info.libraries.as_deref()
        ),
        (1, "cpu", Some("found"))
    );
    assert!(!info.llama_cpp.is_empty());
    assert!(fake.runtime.is_running());
    common::passed("C03");
}

#[test]
fn the_libraries_of_a_runtime_can_be_missing() {
    let Some(fake) = Fake::start(Some(r#"{"libraries":"missing","backend":"metal"}"#)) else {
        return;
    };
    let info = fake.runtime.hello().unwrap();
    assert_eq!(
        (info.backend.as_str(), info.libraries.as_deref()),
        ("metal", Some("missing"))
    );
}

#[test]
fn a_cleanup_loads_once_counts_completes_and_unloads() {
    let Some(fake) = Fake::start(None) else {
        return;
    };
    let model = fake.model("a.gguf");
    let ((tokens, text), load_ms) = fake
        .runtime
        .with_model("a", &model, 2048, None, |s| {
            let tokens = s.count("one two three")?;
            let text = s.complete(
                "tone: formal\ntext: um hello there\nout:",
                &["\ntext:".to_string()],
                40,
                None,
            )?;
            Ok((tokens, text))
        })
        .unwrap();
    assert_eq!(tokens, 3);
    assert_eq!(text.text, " Hello there.");
    assert_eq!(text.stop, Stop::End);
    assert_eq!(load_ms, 3);
    let (_, load_ms) = fake
        .runtime
        .with_model("a", &model, 2048, None, |s| s.count("x"))
        .unwrap();
    assert_eq!(load_ms, 0, "the model stays loaded");

    fake.runtime.unload().unwrap();
    assert!(fake.runtime.is_running(), "an unload keeps the process");
    let (_, load_ms) = fake
        .runtime
        .with_model("a", &model, 2048, None, |s| s.count("x"))
        .unwrap();
    assert_eq!(load_ms, 3, "the model loads again after the unload");
    common::passed("C03");
}

#[test]
fn a_completion_that_runs_out_of_tokens_stops_with_the_limit() {
    let Some(fake) = Fake::start(None) else {
        return;
    };
    let model = fake.model("a.gguf");
    let (done, _) = fake
        .runtime
        .with_model("a", &model, 2048, None, |s| {
            s.complete("text: one two three four\nout:", &[], 2, None)
        })
        .unwrap();
    assert_eq!((done.stop, done.tokens_out), (Stop::Limit, 2));
}

#[test]
fn another_model_loads_again() {
    let Some(fake) = Fake::start(None) else {
        return;
    };
    let (a, b) = (fake.model("a.gguf"), fake.model("b.gguf"));
    fake.runtime
        .with_model("a", &a, 2048, None, |s| s.count("x"))
        .unwrap();
    let (_, load_ms) = fake
        .runtime
        .with_model("b", &b, 2048, None, |s| s.count("x"))
        .unwrap();
    assert_eq!(load_ms, 3);
}

#[test]
fn a_load_that_fails_is_an_error_and_the_runtime_keeps_running() {
    let Some(fake) = Fake::start(None) else {
        return;
    };
    let error = fake
        .runtime
        .ensure_loaded("a", &fake.folder.path().join("missing.gguf"), 2048, None)
        .unwrap_err();
    assert!(
        matches!(
            error,
            RuntimeError::Failed {
                code: ErrorCode::LoadFailed,
                ..
            }
        ),
        "{error:?}"
    );
    assert!(fake.runtime.is_running());
    fake.runtime
        .ensure_loaded("a", &fake.model("a.gguf"), 2048, None)
        .unwrap();
}

#[test]
fn a_request_past_its_deadline_kills_the_process_and_the_next_one_starts_it_again() {
    let Some(fake) = Fake::start(Some(r#"{"delay_ms":{"complete":3000}}"#)) else {
        return;
    };
    let model = fake.model("a.gguf");
    let started = Instant::now();
    let error = fake
        .runtime
        .with_model("a", &model, 2048, None, |s| {
            s.complete(
                "text: slow\nout:",
                &[],
                40,
                Some(Instant::now() + Duration::from_millis(300)),
            )
        })
        .unwrap_err();
    assert_eq!(error, RuntimeError::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the client did not wait for the late reply"
    );
    assert!(!fake.runtime.is_running());
    // The new process reads the environment again.
    set_script(None);
    let (answer, load_ms) = fake
        .runtime
        .with_model("a", &model, 2048, None, |s| {
            s.complete("text: again\nout:", &[], 40, None)
        })
        .unwrap();
    assert_eq!(answer.text, " Again.");
    assert_eq!(load_ms, 3, "the new process loads the model again");
}

#[test]
fn a_crash_is_an_error_and_the_next_request_works() {
    let Some(fake) = Fake::start(Some(r#"{"crash_at":2}"#)) else {
        return;
    };
    let model = fake.model("a.gguf");
    let error = fake
        .runtime
        .with_model("a", &model, 2048, None, |s| {
            s.complete("text: boom\nout:", &[], 40, None)
        })
        .unwrap_err();
    assert!(matches!(error, RuntimeError::Stopped(_)), "{error:?}");
    set_script(None);
    let (answer, _) = fake
        .runtime
        .with_model("a", &model, 2048, None, |s| {
            s.complete("text: fine\nout:", &[], 40, None)
        })
        .unwrap();
    assert_eq!(answer.text, " Fine.");
}

#[test]
fn a_build_that_this_processor_cannot_run_does_not_start() {
    // Only on x86_64 without AVX, or on another system, is there a build that this processor cannot run.
    let Some(build) = Level::ALL.into_iter().find(|b| !b.runs_here()) else {
        return;
    };
    let Some(fake) = Fake::start(None) else {
        return;
    };
    let path = fake.folder.path().join(build.file_name("sayso-cleanup"));
    std::fs::copy(&fake.program, &path).unwrap();
    let runtime = Runtime::new(path);
    let error = runtime
        .ensure_loaded("a", &fake.model("a.gguf"), 2048, None)
        .unwrap_err();
    assert!(
        matches!(error, RuntimeError::UnsupportedCpu(_)),
        "{error:?}"
    );
    assert!(
        sayso_sdk::cleanup::client::runtime::is_unsupported_cpu(&error.to_string()),
        "{error}"
    );
    assert!(!runtime.is_running());
}

#[test]
fn a_missing_binary_is_not_installed() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let runtime = Runtime::new(PathBuf::from("/no/such/sayso-cleanup"));
    assert!(matches!(
        runtime.ensure_loaded("a", Path::new("/models/a.gguf"), 2048, None),
        Err(RuntimeError::NotInstalled(_))
    ));
}

#[test]
fn a_busy_runtime_makes_a_short_wait_time_out() {
    let Some(fake) = Fake::start(Some(r#"{"delay_ms":{"complete":1500}}"#)) else {
        return;
    };
    let model = fake.model("a.gguf");
    let busy = fake.runtime.clone();
    let busy_model = model.clone();
    let worker = std::thread::spawn(move || {
        let _ = busy.with_model("a", &busy_model, 2048, None, |s| {
            s.complete(
                "text: slow\nout:",
                &[],
                40,
                Some(Instant::now() + Duration::from_secs(5)),
            )
        });
    });
    std::thread::sleep(Duration::from_millis(300));
    let error = fake
        .runtime
        .with_model(
            "a",
            &model,
            2048,
            Some(Instant::now() + Duration::from_millis(100)),
            |s| s.count("x"),
        )
        .unwrap_err();
    assert_eq!(error, RuntimeError::Timeout);
    worker.join().unwrap();
}

#[test]
fn stop_ends_the_process() {
    let Some(fake) = Fake::start(None) else {
        return;
    };
    fake.runtime
        .ensure_loaded("a", &fake.model("a.gguf"), 2048, None)
        .unwrap();
    assert!(fake.runtime.is_running());
    fake.runtime.stop();
    assert!(!fake.runtime.is_running());
}

#[test]
fn after_a_failed_switch_the_old_model_loads_again() {
    let Some(fake) = Fake::start(None) else {
        return;
    };
    let model = fake.model("a.gguf");
    fake.runtime.ensure_loaded("a", &model, 2048, None).unwrap();
    assert!(
        fake.runtime
            .ensure_loaded("b", &fake.folder.path().join("missing.gguf"), 2048, None)
            .is_err()
    );
    // The runtime has no model now. The client must not think that "a" is loaded.
    let (tokens, load_ms) = fake
        .runtime
        .with_model("a", &model, 2048, None, |s| s.count("one two"))
        .unwrap();
    assert_eq!((tokens, load_ms), (2, 3));
}

#[test]
fn nothing_loads_while_cleanup_is_off_or_the_model_is_being_deleted() {
    let Some(fake) = Fake::start(None) else {
        return;
    };
    let (a, b) = (fake.model("a.gguf"), fake.model("b.gguf"));
    fake.runtime.ensure_loaded("a", &a, 2048, None).unwrap();
    fake.runtime.set_enabled(false);
    fake.runtime.stop();
    assert_eq!(
        fake.runtime.ensure_loaded("a", &a, 2048, None),
        Err(RuntimeError::Closed)
    );
    assert!(
        !fake.runtime.is_running(),
        "a refused load does not start the process"
    );
    fake.runtime.set_enabled(true);
    fake.runtime.ensure_loaded("a", &a, 2048, None).unwrap();

    fake.runtime.block("a");
    assert!(!fake.runtime.is_running(), "block stops the process");
    assert_eq!(
        fake.runtime.ensure_loaded("a", &a, 2048, None),
        Err(RuntimeError::Closed)
    );
    fake.runtime.ensure_loaded("b", &b, 2048, None).unwrap();
    fake.runtime.unblock("a");
    fake.runtime.ensure_loaded("a", &a, 2048, None).unwrap();
}
