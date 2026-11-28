//! The wire corpus of `sdk/protocol/testdata` (scenarios P01 and P02): every message that the Go
//! SDK and the Go host read must read the same here, and write the same again. `names.json` lists
//! the names of the protocols, and `paths.json` the path rules.

mod common;

use sayso_sdk::protocol::{
    self, Capability, Decoded, ErrorCode, Kind, ModelState, Role, Source, SourceEndReason,
    TimestampLevel,
};
use serde_json::Value;
use std::path::PathBuf;

fn corpus(name: &str) -> Option<PathBuf> {
    let path = common::corpus_root().join(name);
    if path.exists() {
        return Some(path);
    }
    assert!(
        !common::require(),
        "SAYSO_TEST_REQUIRE_FAKES=1 and {} is missing",
        path.display()
    );
    eprintln!(
        "skipped: {} is not here (the crate was unpacked without the repository)",
        path.display()
    );
    None
}

/// The cases of an ndjson file: the comment above each message line, and the line.
fn cases(name: &str) -> Vec<(String, String)> {
    let Some(path) = corpus(name) else {
        return Vec::new();
    };
    let text = std::fs::read_to_string(path).unwrap();
    let mut case = String::new();
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Some(name) = line.strip_prefix("# ") {
            case = name.to_string();
        } else {
            assert!(!case.is_empty(), "a message without a case name: {line}");
            out.push((std::mem::take(&mut case), line.to_string()));
        }
    }
    out
}

/// A field with the value `null` counts as missing.
fn normalized(value: Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, normalized(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(normalized).collect()),
        other => other,
    }
}

/// Decode `line` as a `T`, write it again, and compare the two as JSON values.
fn round_trip<T: Kind + std::fmt::Debug>(case: &str, line: &str) {
    match protocol::decode::<T>(line) {
        Ok(Decoded::Known(message)) => {
            if case.starts_with("decode:") {
                return;
            }
            let written = protocol::encode(message.id.as_deref(), &message.body);
            let (written, read): (Value, Value) = (
                serde_json::from_str(&written).unwrap(),
                serde_json::from_str(line).unwrap(),
            );
            assert_eq!(normalized(written), normalized(read), "{case}: {line}");
        }
        Ok(Decoded::Unknown { .. }) if case.starts_with("decode:") => {}
        Ok(Decoded::Unknown { kind, .. }) => panic!("{case}: the type {kind} is unknown: {line}"),
        Err(e) => panic!("{case}: {e}: {line}"),
    }
}

fn unknown<T: Kind + std::fmt::Debug>(case: &str, line: &str) {
    match protocol::decode::<T>(line) {
        Ok(Decoded::Unknown { .. }) => {}
        other => panic!("{case}: expected an unknown type, got {other:?}"),
    }
}

#[test]
fn the_host_requests_read_and_write_again() {
    let cases = cases("host/requests.ndjson");
    for (case, line) in &cases {
        round_trip::<protocol::Request>(case, line);
    }
    common::passed("P01");
}

#[test]
fn the_host_replies_and_events_read_and_write_again() {
    for file in ["host/replies.ndjson", "host/events.ndjson"] {
        for (case, line) in cases(file) {
            round_trip::<protocol::HostMessage>(&case, &line);
        }
    }
    common::passed("P01");
}

#[test]
fn names_and_fields_that_this_version_does_not_know_are_kept_or_ignored() {
    for (case, line) in cases("host/edge.ndjson") {
        let kind = serde_json::from_str::<Value>(&line).unwrap()["type"]
            .as_str()
            .unwrap()
            .to_string();
        if protocol::Request::knows(&kind) {
            round_trip::<protocol::Request>(&case, &line);
        } else if protocol::HostMessage::knows(&kind) {
            round_trip::<protocol::HostMessage>(&case, &line);
        } else {
            unknown::<protocol::HostMessage>(&case, &line);
            unknown::<protocol::Request>(&case, &line);
        }
    }
    common::passed("P01");
}

#[test]
fn an_unknown_name_comes_back_as_the_name() {
    let hello: Value = serde_json::from_str(r#"{"type":"hello","protocol":1,"features":["future"],"capabilities":["calendar.read"],"v":1,"id":"x2"}"#).unwrap();
    let Ok(Decoded::Known(message)) = protocol::decode::<protocol::Request>(&hello.to_string())
    else {
        panic!()
    };
    let protocol::Request::Hello(hello) = message.body else {
        panic!()
    };
    assert_eq!(
        hello.capabilities,
        vec![Capability::Other("calendar.read".into())]
    );
    assert_eq!(
        ErrorCode::parse("quota_exceeded").as_str(),
        "quota_exceeded"
    );
}

#[test]
fn the_ids_up_to_two_to_the_53_stay_exact() {
    let line = r#"{"type":"job_done","job":9007199254740991,"text":"","v":1}"#;
    let Ok(Decoded::Known(message)) = protocol::decode::<protocol::HostMessage>(line) else {
        panic!()
    };
    assert!(matches!(
        message.body,
        protocol::HostMessage::Event(protocol::Event::JobDone {
            job: 9007199254740991,
            ..
        })
    ));
}

#[test]
fn the_engine_messages_read_and_write_again() {
    for (case, line) in cases("engine/requests.ndjson") {
        round_trip::<protocol::engine::Request>(&case, &line);
    }
    for file in ["engine/replies.ndjson", "engine/events.ndjson"] {
        for (case, line) in cases(file) {
            round_trip::<protocol::engine::FromEngine>(&case, &line);
        }
    }
    common::passed("P01");
}

#[cfg(feature = "cleanup")]
#[test]
fn the_cleanup_messages_read_and_write_again() {
    use sayso_sdk::cleanup::models::protocol::{ReplyLine, RequestLine};
    for (case, line) in cases("cleanup/requests.ndjson") {
        let request: RequestLine =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("{case}: {e}: {line}"));
        let written = serde_json::to_value(&request).unwrap();
        assert_eq!(
            normalized(written),
            normalized(serde_json::from_str(&line).unwrap()),
            "{case}"
        );
    }
    for (case, line) in cases("cleanup/replies.ndjson") {
        let reply: ReplyLine =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("{case}: {e}: {line}"));
        let written = serde_json::to_value(&reply).unwrap();
        assert_eq!(
            normalized(written),
            normalized(serde_json::from_str(&line).unwrap()),
            "{case}"
        );
    }
    common::passed("P01");
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {value}"))
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

fn names<T: std::fmt::Display>(all: &[T]) -> Vec<String> {
    all.iter().map(|n| n.to_string()).collect()
}

fn types(all: &[&str]) -> Vec<String> {
    all.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_names_of_the_protocols_are_the_names_of_names_json() {
    let Some(path) = corpus("names.json") else {
        return;
    };
    let all: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let host = &all["host"];
    assert_eq!(host["protocol"], protocol::PROTOCOL);
    assert_eq!(strings(&host["capabilities"]), names(Capability::ALL));
    assert_eq!(strings(&host["error_codes"]), names(ErrorCode::ALL));
    assert_eq!(strings(&host["model_states"]), names(ModelState::ALL));
    assert_eq!(strings(&host["roles"]), names(Role::ALL));
    assert_eq!(strings(&host["sources"]), names(Source::ALL));
    assert_eq!(strings(&host["source_ends"]), names(SourceEndReason::ALL));
    assert_eq!(
        strings(&host["timestamp_levels"]),
        names(TimestampLevel::ALL)
    );
    assert_eq!(strings(&host["features"]), types(protocol::feature::ALL));
    assert_eq!(
        strings(&host["engine_features"]),
        types(protocol::feature::ENGINE)
    );
    assert_eq!(
        strings(&host["request_types"]),
        types(protocol::Request::TYPES)
    );
    // The Rust events of the host are the names of `event_types`: the Event type lists them in its own order.
    let mut rust: Vec<_> = protocol::Event::TYPES
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut go = strings(&host["event_types"]);
    rust.sort();
    go.sort();
    assert_eq!(rust, go);

    let engine = &all["engine"];
    assert_eq!(engine["protocol"], protocol::PROTOCOL);
    assert_eq!(
        strings(&engine["features"]),
        types(protocol::engine::FEATURES)
    );
    assert_eq!(
        strings(&engine["reasons"]),
        types(protocol::engine::REASONS)
    );
    assert_eq!(
        strings(&engine["request_types"]),
        types(protocol::engine::Request::TYPES)
    );
    assert_eq!(
        strings(&engine["reply_types"]),
        types(protocol::engine::Reply::TYPES)
    );
    let mut rust: Vec<_> = protocol::engine::Event::TYPES
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut go = strings(&engine["event_types"]);
    rust.sort();
    go.sort();
    assert_eq!(rust, go);
    for code in strings(&engine["error_codes"]) {
        assert!(
            ErrorCode::ALL.iter().any(|known| known.as_str() == code),
            "the engine code {code} is a host code too"
        );
    }
    common::passed("P01");
}

#[cfg(feature = "cleanup")]
#[test]
fn the_cleanup_names_are_the_names_of_names_json() {
    use sayso_sdk::cleanup::models::protocol::{ErrorCode, PROTOCOL_VERSION, Stop};
    let Some(path) = corpus("names.json") else {
        return;
    };
    let all: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let cleanup = &all["cleanup"];
    assert_eq!(cleanup["protocol"], PROTOCOL_VERSION);
    fn name<T: serde::Serialize>(value: &T) -> String {
        serde_json::to_value(value)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }
    assert_eq!(
        strings(&cleanup["stops"]),
        [Stop::End, Stop::Limit, Stop::Context]
            .iter()
            .map(name)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        strings(&cleanup["error_codes"]),
        [
            ErrorCode::BadRequest,
            ErrorCode::NotLoaded,
            ErrorCode::WrongModel,
            ErrorCode::LoadFailed,
            ErrorCode::DecodeFailed
        ]
        .iter()
        .map(name)
        .collect::<Vec<_>>()
    );
}

struct CaseEnv {
    vars: std::collections::HashMap<String, String>,
    temp: Option<String>,
    uid: Option<u32>,
    sid: Option<String>,
}

impl protocol::socket::SocketEnv for CaseEnv {
    fn var(&self, key: &str) -> Option<String> {
        self.vars.get(key).cloned().filter(|v| !v.is_empty())
    }
    fn darwin_user_temp_dir(&self) -> Option<PathBuf> {
        self.temp.clone().map(PathBuf::from)
    }
    fn uid(&self) -> Option<u32> {
        self.uid
    }
    fn user_sid(&self) -> Option<String> {
        self.sid.clone()
    }
}

fn vars_of(case: &Value) -> std::collections::HashMap<String, String> {
    case["env"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
        .collect()
}

/// A path as text with `/`, so the expected text of the table also matches a path made on Windows.
fn slashes(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[test]
fn the_socket_rules_give_the_paths_of_paths_json() {
    use protocol::socket::{System, client_socket_path_for, host_socket_path_for};
    let Some(path) = corpus("paths.json") else {
        return;
    };
    let table: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for case in table["socket"].as_array().unwrap() {
        let name = case["case"].as_str().unwrap();
        let system = match case["system"].as_str().unwrap() {
            "linux" => System::Linux,
            "macos" => System::MacOs,
            "windows" => System::Windows,
            _ => System::OtherUnix,
        };
        let env = CaseEnv {
            vars: vars_of(case),
            temp: case["darwin_temp"].as_str().map(String::from),
            uid: case["uid"].as_u64().map(|u| u as u32),
            sid: case["sid"].as_str().map(String::from),
        };
        let (host, client) = (
            host_socket_path_for(system, &env),
            client_socket_path_for(system, &env),
        );
        if case["error"].as_bool() == Some(true) {
            assert!(host.is_err() && client.is_err(), "{name}");
            continue;
        }
        assert_eq!(
            slashes(&host.unwrap()),
            case["host"].as_str().unwrap().replace('\\', "/"),
            "{name}: host"
        );
        assert_eq!(
            slashes(&client.unwrap()),
            case["client"].as_str().unwrap().replace('\\', "/"),
            "{name}: client"
        );
    }
    common::passed("P02");
}

#[test]
fn the_data_rules_give_the_folders_of_paths_json() {
    use sayso_sdk::paths;
    let Some(path) = corpus("paths.json") else {
        return;
    };
    let table: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for case in table["data"].as_array().unwrap() {
        let name = case["case"].as_str().unwrap();
        let mut vars = vars_of(case);
        let home = case["home"].as_str().unwrap().to_string();
        vars.insert("HOME".into(), home.clone());
        vars.insert("USERPROFILE".into(), home);
        let env = move |key: &str| {
            vars.get(key)
                .cloned()
                .filter(|v| !v.is_empty())
                .map(std::ffi::OsString::from)
        };
        let os = match case["goos"].as_str().unwrap() {
            "darwin" => "macos",
            other => other,
        };
        for (field, got) in [
            ("data", paths::data_for(os, &env)),
            ("models", paths::models_for(os, &env)),
            ("store", paths::store_for(os, &env)),
            ("cleanup", paths::cleanup_for(os, &env)),
        ] {
            assert_eq!(
                slashes(&got),
                case[field].as_str().unwrap(),
                "{name}: {field}"
            );
        }
    }
    common::passed("P02");
}
