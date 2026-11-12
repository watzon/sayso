//! The values of the addon. A test of a napi addon cannot link without Node, so this
//! test builds `src/values.rs` by itself.

#[path = "../src/values.rs"]
#[allow(dead_code)]
mod values;

use sayso_protocol::{ErrorCode, Event as Message};
use sayso_sdk::{ActiveMode, Event, Features, Mode};
use serde_json::{Value, json};
use values::*;

#[test]
fn numbers_up_to_2_53_pass() {
    assert!(check_safe(&json!({ "job": MAX_SAFE_INTEGER, "list": [1, -2, 0.5] })).is_ok());
}

#[test]
fn a_number_above_2_53_fails() {
    let e = check_safe(&json!({ "inner": [{ "size": MAX_SAFE_INTEGER + 1 }] })).unwrap_err();
    assert_eq!(e.code, Some(ErrorCode::BadRequest));
    assert!(e.message.contains("2^53"), "{}", e.message);
    assert!(check_safe(&json!(-(MAX_SAFE_INTEGER as i64) - 1)).is_err());
}

#[test]
fn an_outcome_is_ok_or_err() {
    assert_eq!(outcome_text(Ok(json!(5))), r#"{"ok":5}"#);
    let back: Value = serde_json::from_str(&outcome_text(Err(error(Some(ErrorCode::Busy), "busy")))).unwrap();
    assert_eq!(back["err"]["code"], "busy");
    assert_eq!(back["err"]["message"], "busy");
    let big = outcome_text(Ok(json!(u64::MAX)));
    assert!(big.starts_with(r#"{"err":"#), "{big}");
}

#[test]
fn integers_from_javascript() {
    assert_eq!(integer(7.0, "the job").unwrap(), 7);
    assert_eq!(integer(MAX_SAFE_INTEGER as f64, "the job").unwrap(), MAX_SAFE_INTEGER);
    assert!(integer(7.5, "the job").is_err());
    assert!(integer(-1.0, "the job").is_err());
    assert!(integer(f64::NAN, "the job").is_err());
    assert!(integer(9007199254740992.0, "the job").unwrap_err().contains("the job"));
}

#[test]
fn events_have_the_names_of_the_wire() {
    let done = event_value(Event::Message(Message::JobDone { job: 3, text: "hi".into() })).unwrap();
    assert_eq!(done, json!({ "kind": "message", "message": { "type": "job_done", "job": 3, "text": "hi" } }));
    let features = Features { enhance: false, subscribe: false, download_question: false, system_audio: true, capabilities: vec![], denied: vec![], host: vec![], engine: None };
    let changed = event_value(Event::ModeChanged { mode: ActiveMode::Local, features }).unwrap();
    assert_eq!(changed["mode"], "local");
    assert_eq!(changed["features"]["systemAudio"], true);
    assert_eq!(changed["features"]["engine"], Value::Null);
    let gone = event_value(Event::Disconnected { code: ErrorCode::HostClosing }).unwrap();
    assert_eq!(gone, json!({ "kind": "disconnected", "code": "host_closing" }));
}

#[test]
fn options_from_javascript() {
    let options = options(r#"{"mode":"auto","app":{"name":"Notes"},"capabilities":["transcribe.file"],"engine":{"program":"/x/engine","modelsDir":"/x/models"},"models":{"finalModel":"m","language":"en"},"timing":{"retryMs":5}}"#).unwrap();
    assert_eq!(options.mode, Mode::Auto);
    assert_eq!(options.app.name, "Notes");
    assert_eq!(options.capabilities.len(), 1);
    assert!(options.engine.is_some());
    assert_eq!(options.models.final_model.as_deref(), Some("m"));
    assert_eq!(options.timing.retry.as_millis(), 5);
    let sayso = values::options(r#"{"mode":{"sayso":{"start":true}},"app":{"name":"N"}}"#).unwrap();
    assert_eq!(sayso.mode, Mode::Sayso { start: true });
}

#[test]
fn bad_options_say_what_is_wrong() {
    let fails = |text: &str| match values::options(text) {
        Ok(_) => panic!("{text} passed"),
        Err(e) => e,
    };
    assert!(fails(r#"{"mode":"remote","app":{"name":"N"}}"#).contains("remote"));
    assert!(fails(r#"{"mode":"local","app":{"name":"N"},"engine":{}}"#).contains("release or program"));
    assert!(fails(r#"{"mode":"local","app":{"name":"N"},"colour":1}"#).contains("colour"));
}
