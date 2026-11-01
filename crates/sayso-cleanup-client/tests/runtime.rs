//! The runtime client against the fake runtime (`src/bin/fake-cleanup-runtime.rs`).

use sayso_cleanup_client::runtime::{Runtime, RuntimeError};
use sayso_cleanup_models::protocol::{ErrorCode, Stop};
use sayso_cleanup_models::cpu::Level;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn runtime() -> Arc<Runtime> {
    Runtime::new(PathBuf::from(env!("CARGO_BIN_EXE_fake-cleanup-runtime")))
}

fn model() -> &'static Path {
    Path::new("/models/a.gguf")
}

#[test]
fn a_cleanup_loads_once_and_completes() {
    let rt = runtime();
    let (text, load_ms) = rt
        .with_model("a", model(), 2048, None, |s| {
            assert_eq!(s.count("12345678")?, 2);
            s.complete("tone: formal\ntext: hello there\nout:", &[], 40, None)
        })
        .unwrap();
    assert_eq!(text.text, " HELLO THERE");
    assert_eq!(text.stop, Stop::End);
    assert_eq!(load_ms, 1);
    // The model stays loaded: no second load.
    let (_, load_ms) = rt
        .with_model("a", model(), 2048, None, |s| s.count("x"))
        .unwrap();
    assert_eq!(load_ms, 0);
}

#[test]
fn another_model_loads_again() {
    let rt = runtime();
    rt.with_model("a", model(), 2048, None, |s| s.count("x"))
        .unwrap();
    let (_, load_ms) = rt
        .with_model("b", Path::new("/models/b.gguf"), 2048, None, |s| {
            s.count("x")
        })
        .unwrap();
    assert_eq!(load_ms, 1);
}

#[test]
fn a_load_that_fails_is_an_error_and_the_runtime_keeps_running() {
    let rt = runtime();
    let error = rt
        .ensure_loaded("a", Path::new("/models/missing.gguf"), 2048, None)
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
    assert!(rt.is_running());
    rt.ensure_loaded("a", model(), 2048, None).unwrap();
}

#[test]
fn a_request_past_its_deadline_kills_the_process_and_the_next_one_starts_it_again() {
    let rt = runtime();
    let started = Instant::now();
    let error = rt
        .with_model("a", model(), 2048, None, |s| {
            s.complete(
                "text: SLEEP\nout:",
                &[],
                40,
                Some(Instant::now() + Duration::from_millis(300)),
            )
        })
        .unwrap_err();
    assert_eq!(error, RuntimeError::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the client did not wait for the late reply"
    );
    assert!(!rt.is_running());
    let (answer, load_ms) = rt
        .with_model("a", model(), 2048, None, |s| {
            s.complete("text: again\nout:", &[], 40, None)
        })
        .unwrap();
    assert_eq!(answer.text, " AGAIN");
    assert_eq!(load_ms, 1, "the new process loads the model again");
}

#[test]
fn a_crash_is_an_error_and_the_next_request_works() {
    let rt = runtime();
    let error = rt
        .with_model("a", model(), 2048, None, |s| {
            s.complete("text: CRASH\nout:", &[], 40, None)
        })
        .unwrap_err();
    assert!(matches!(error, RuntimeError::Stopped(_)), "{error:?}");
    let (answer, _) = rt
        .with_model("a", model(), 2048, None, |s| {
            s.complete("text: fine\nout:", &[], 40, None)
        })
        .unwrap();
    assert_eq!(answer.text, " FINE");
}

#[test]
fn an_illegal_instruction_is_a_processor_that_cannot_run_the_build() {
    let rt = runtime();
    let error = rt
        .with_model("a", model(), 2048, None, |s| {
            s.complete("text: ILLEGAL\nout:", &[], 40, None)
        })
        .unwrap_err();
    assert!(
        matches!(error, RuntimeError::UnsupportedCpu(_)),
        "{error:?}"
    );
    assert!(
        sayso_cleanup_client::runtime::is_unsupported_cpu(&error.to_string()),
        "{error}"
    );
    // Another request starts the process again, as after any crash.
    let (answer, _) = rt
        .with_model("a", model(), 2048, None, |s| {
            s.complete("text: fine\nout:", &[], 40, None)
        })
        .unwrap();
    assert_eq!(answer.text, " FINE");
}

#[test]
fn a_build_that_this_processor_cannot_run_does_not_start() {
    // The build of the packages that this processor lacks the instructions
    // for. Only on x86_64 without AVX, or on another system, is there one.
    let Some(build) = Level::ALL.into_iter().find(|b| !b.runs_here()) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(build.file_name("sayso-cleanup"));
    std::fs::copy(env!("CARGO_BIN_EXE_fake-cleanup-runtime"), &path).unwrap();
    let rt = Runtime::new(path);
    let error = rt.ensure_loaded("a", model(), 2048, None).unwrap_err();
    assert!(
        matches!(error, RuntimeError::UnsupportedCpu(_)),
        "{error:?}"
    );
    assert!(!rt.is_running());
}

#[test]
fn a_missing_binary_is_not_installed() {
    let rt = Runtime::new(PathBuf::from("/no/such/sayso-cleanup"));
    assert!(matches!(
        rt.ensure_loaded("a", model(), 2048, None),
        Err(RuntimeError::NotInstalled(_))
    ));
}

#[test]
fn a_busy_runtime_makes_a_short_wait_time_out() {
    let rt = runtime();
    let busy = rt.clone();
    let worker = std::thread::spawn(move || {
        let _ = busy.with_model("a", model(), 2048, None, |s| {
            s.complete(
                "text: SLEEP\nout:",
                &[],
                40,
                Some(Instant::now() + Duration::from_secs(2)),
            )
        });
    });
    std::thread::sleep(Duration::from_millis(200));
    let error = rt
        .with_model(
            "a",
            model(),
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
    let rt = runtime();
    rt.ensure_loaded("a", model(), 2048, None).unwrap();
    assert!(rt.is_running());
    rt.stop();
    assert!(!rt.is_running());
}

#[test]
fn after_a_failed_switch_the_old_model_loads_again() {
    let rt = runtime();
    rt.ensure_loaded("a", model(), 2048, None).unwrap();
    assert!(
        rt.ensure_loaded("b", Path::new("/models/missing.gguf"), 2048, None)
            .is_err()
    );
    // The runtime has no model now. The client must not think that "a" is loaded.
    let (tokens, load_ms) = rt
        .with_model("a", model(), 2048, None, |s| s.count("12345678"))
        .unwrap();
    assert_eq!((tokens, load_ms), (2, 1));
}

#[test]
fn nothing_loads_while_cleanup_is_off_or_the_model_is_being_deleted() {
    let rt = runtime();
    rt.ensure_loaded("a", model(), 2048, None).unwrap();
    rt.set_enabled(false);
    rt.stop();
    assert_eq!(
        rt.ensure_loaded("a", model(), 2048, None),
        Err(RuntimeError::Closed)
    );
    assert!(
        !rt.is_running(),
        "a refused load does not start the process"
    );
    rt.set_enabled(true);
    rt.ensure_loaded("a", model(), 2048, None).unwrap();

    rt.block("a");
    assert!(!rt.is_running(), "block stops the process");
    assert_eq!(
        rt.ensure_loaded("a", model(), 2048, None),
        Err(RuntimeError::Closed)
    );
    // Another model still loads.
    rt.ensure_loaded("b", Path::new("/models/b.gguf"), 2048, None)
        .unwrap();
    rt.unblock("a");
    rt.ensure_loaded("a", model(), 2048, None).unwrap();
}
