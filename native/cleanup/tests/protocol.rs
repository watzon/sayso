//! Protocol tests of the built binary that need no model file.

mod common;

use common::Session;
use sayso_core::cleanup::protocol::{ErrorCode, PROTOCOL_VERSION, Reply, Request};

fn assert_error(reply: Reply, expected: ErrorCode) {
    match reply {
        Reply::Error { code, message } => {
            assert_eq!(code, expected, "{message}");
            assert!(!message.is_empty());
        }
        other => panic!("expected {expected:?}, got {other:?}"),
    }
}

#[test]
fn hello_names_version_and_backend() {
    let mut s = Session::start();
    match s.call(Request::Hello) {
        Reply::Hello {
            version,
            llama_cpp,
            backend,
        } => {
            assert_eq!(version, PROTOCOL_VERSION);
            assert!(!llama_cpp.is_empty());
            assert!(backend == "metal" || backend == "cpu", "{backend}");
        }
        other => panic!("{other:?}"),
    }
    assert!(s.finish().success());
}

#[test]
fn a_line_that_does_not_parse_is_a_bad_request() {
    let mut s = Session::start();
    let reply = s.raw("this is not json");
    assert_eq!(reply.id, "");
    assert_error(reply.reply, ErrorCode::BadRequest);
    // The id is kept when it can be read.
    let reply = s.raw(r#"{"v":1,"id":"42","type":"no_such_request"}"#);
    assert_eq!(reply.id, "42");
    assert_error(reply.reply, ErrorCode::BadRequest);
    // The process still works.
    assert!(matches!(s.call(Request::Hello), Reply::Hello { .. }));
    assert!(s.finish().success());
}

#[test]
fn requests_that_need_a_model_say_not_loaded() {
    let mut s = Session::start();
    assert_error(
        s.call(Request::Count {
            model_id: "m".into(),
            text: "hi".into(),
        }),
        ErrorCode::NotLoaded,
    );
    let complete = Request::Complete {
        model_id: "m".into(),
        prompt: "hi".into(),
        end: vec![],
        max_tokens: 4,
    };
    assert_error(s.call(complete), ErrorCode::NotLoaded);
    assert_eq!(s.call(Request::Unload), Reply::Unloaded);
    assert!(s.finish().success());
}

#[test]
fn a_missing_model_file_is_a_load_failure() {
    let mut s = Session::start();
    let missing = std::env::temp_dir().join("sayso-cleanup-test-no-such-file.gguf");
    let load = Request::Load {
        model_id: "m".into(),
        path: missing.display().to_string(),
        n_ctx: 512,
    };
    assert_error(s.call(load), ErrorCode::LoadFailed);
    // A file that is not a GGUF file fails too, and the runtime keeps running.
    let junk = std::env::temp_dir().join(format!(
        "sayso-cleanup-test-junk-{}.gguf",
        std::process::id()
    ));
    std::fs::write(&junk, b"not a model").unwrap();
    let load = Request::Load {
        model_id: "m".into(),
        path: junk.display().to_string(),
        n_ctx: 512,
    };
    assert_error(s.call(load), ErrorCode::LoadFailed);
    std::fs::remove_file(&junk).ok();
    assert_error(
        s.call(Request::Count {
            model_id: "m".into(),
            text: "x".into(),
        }),
        ErrorCode::NotLoaded,
    );
    assert!(s.finish().success());
}

#[test]
fn an_empty_context_is_a_bad_request() {
    let mut s = Session::start();
    let load = Request::Load {
        model_id: "m".into(),
        path: "x".into(),
        n_ctx: 0,
    };
    assert_error(s.call(load), ErrorCode::BadRequest);
    assert!(s.finish().success());
}

#[test]
fn end_of_input_exits_with_zero() {
    assert!(Session::start().finish().success());
}
