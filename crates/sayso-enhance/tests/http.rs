//! The OpenAI-compatible provider against a local mock server.
//! Each test uses its own model name, because the mode cache is per process.

mod common;

use common::{MockServer, Reply};
use sayso_core::config::{Ai, Provider, ProviderKind};
use sayso_core::enhance::{EnhanceError, EnhanceRequest, Enhancer, output_schema};
use sayso_enhance::{MemoryStore, OpenAiCompatible, SecretStore, build_enhancer, list_models, test_provider};
use serde_json::json;
use std::time::{Duration, Instant};

fn request(model: &str) -> EnhanceRequest {
    EnhanceRequest {
        system_prompt: "Fix the grammar.".into(),
        transcript: "me and him goes home".into(),
        model: Some(model.into()),
        temperature: None,
        timeout: Duration::from_secs(5),
    }
}

fn provider(server: &MockServer) -> OpenAiCompatible {
    OpenAiCompatible::new("local", &server.base_url, "unused")
}

#[test]
fn strict_json_schema_success() {
    let server = MockServer::start(|_, _| Reply::text("He and I go home."));
    let response = provider(&server).enhance(&request("strict-ok")).unwrap();
    assert_eq!(response.text, "He and I go home.");
    assert_eq!(response.mode, "json_schema");
    assert_eq!(response.model, "strict-ok");
    assert_eq!(response.provider_id, "local");

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let sent = &requests[0];
    assert_eq!((sent.method.as_str(), sent.path.as_str()), ("POST", "/v1/chat/completions"));
    assert_eq!(sent.body["model"], "strict-ok");
    assert_eq!(sent.body["response_format"]["json_schema"]["strict"], true);
    assert_eq!(sent.body["response_format"]["json_schema"]["schema"], output_schema());
    assert_eq!(sent.body["messages"][0]["content"], "Fix the grammar.");
    assert_eq!(sent.body["messages"][1]["content"], "<transcript>\nme and him goes home\n</transcript>");
    assert!((sent.body["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);
    assert!(sent.body["max_tokens"].as_u64().unwrap() > 0);
    assert!(!sent.headers.contains_key("authorization"), "no key, no header");
}

#[test]
fn falls_back_to_json_object_after_http_400() {
    let server = MockServer::start(|_, req| match req.format_type() {
        "json_schema" => Reply::status(400, r#"{"error":"response_format json_schema is not supported"}"#),
        _ => Reply::text("Done."),
    });
    let response = provider(&server).enhance(&request("falls-to-object")).unwrap();
    assert_eq!((response.text.as_str(), response.mode.as_str()), ("Done.", "json_object"));
    let requests = server.requests();
    assert_eq!(requests.iter().map(|r| r.format_type()).collect::<Vec<_>>(), ["json_schema", "json_object"]);
    let system = requests[1].body["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("JSON schema") && system.contains("\"text\""), "the schema is in the prompt");
}

#[test]
fn falls_back_when_the_reply_ignores_the_format() {
    let server = MockServer::start(|_, req| match req.format_type() {
        "json_schema" => Reply::content("Sure! He and I go home."),
        _ => Reply::text("He and I go home."),
    });
    let response = provider(&server).enhance(&request("ignores-format")).unwrap();
    assert_eq!(response.mode, "json_object");
}

#[test]
fn falls_back_to_a_forced_tool_call() {
    let server = MockServer::start(|_, req| {
        if req.has_tools() {
            Reply::tool("Tool text.")
        } else if req.format_type() == "json_schema" {
            Reply::status(400, "no")
        } else {
            Reply::status(422, "no")
        }
    });
    let response = provider(&server).enhance(&request("falls-to-tool")).unwrap();
    assert_eq!((response.text.as_str(), response.mode.as_str()), ("Tool text.", "tool_call"));
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    let tool_request = &requests[2].body;
    assert_eq!(tool_request["tools"][0]["function"]["name"], "return_text");
    assert_eq!(tool_request["tools"][0]["function"]["parameters"], output_schema());
    assert_eq!(tool_request["tool_choice"]["function"]["name"], "return_text");
}

#[test]
fn remembers_the_working_mode_per_base_url_and_model() {
    let server = MockServer::start(|_, req| {
        if req.has_tools() { Reply::tool("ok") } else { Reply::status(400, "no") }
    });
    let p = provider(&server);
    assert_eq!(p.enhance(&request("remembered")).unwrap().mode, "tool_call");
    assert_eq!(server.requests().len(), 3);
    assert_eq!(p.enhance(&request("remembered")).unwrap().mode, "tool_call");
    assert_eq!(server.requests().len(), 4, "the second call goes straight to the tool mode");
    // A new provider object shares the memory. Another model starts from the top.
    let again = provider(&server);
    assert_eq!(again.enhance(&request("remembered")).unwrap().mode, "tool_call");
    assert_eq!(server.requests().len(), 5);
    again.enhance(&request("another-model")).unwrap();
    assert_eq!(server.requests().len(), 8);
}

#[test]
fn invalid_output_in_every_mode_is_invalid_output() {
    let server = MockServer::start(|_, _| Reply::content("this is {not json"));
    let error = provider(&server).enhance(&request("always-invalid")).unwrap_err();
    assert!(matches!(error, EnhanceError::InvalidOutput(_)), "{error:?}");
    assert_eq!(server.requests().len(), 3, "all three modes were tried");
}

#[test]
fn wrong_schema_is_invalid_output() {
    let server = MockServer::start(|_, _| Reply::content(r#"{"result":"wrong field"}"#));
    let error = provider(&server).enhance(&request("wrong-field")).unwrap_err();
    assert!(matches!(error, EnhanceError::InvalidOutput(_)));
}

#[test]
fn slow_server_times_out_within_the_deadline() {
    let server = MockServer::start(|_, _| Reply::text("late").after(Duration::from_millis(2_000)));
    let mut req = request("slow");
    req.timeout = Duration::from_millis(300);
    let started = Instant::now();
    let error = provider(&server).enhance(&req).unwrap_err();
    assert_eq!(error, EnhanceError::Timeout(300));
    assert!(started.elapsed() < Duration::from_millis(1_500), "took {:?}", started.elapsed());
}

#[test]
fn the_deadline_covers_all_mode_tries() {
    // Each try takes 200 ms and fails with 400. The 500 ms deadline must stop the ladder.
    let server = MockServer::start(|_, _| Reply::status(400, "no").after(Duration::from_millis(200)));
    let mut req = request("deadline-ladder");
    req.timeout = Duration::from_millis(500);
    let started = Instant::now();
    let error = provider(&server).enhance(&req).unwrap_err();
    assert!(started.elapsed() < Duration::from_millis(1_000), "took {:?}", started.elapsed());
    assert!(matches!(error, EnhanceError::Http { status: 400, .. } | EnhanceError::Timeout(_)), "{error:?}");
}

#[test]
fn openrouter_extras_are_in_the_body() {
    let server = MockServer::start(|_, _| Reply::text("ok"));
    let p = provider(&server).with_openrouter(true).with_zero_data_retention(true);
    p.enhance(&request("or-zdr")).unwrap();
    let body = &server.requests()[0].body;
    assert_eq!(body["provider"]["require_parameters"], true);
    assert_eq!(body["provider"]["zdr"], true);
    assert_eq!(body["provider"]["data_collection"], "deny");
}

#[test]
fn openrouter_without_zdr_only_requires_parameters() {
    let server = MockServer::start(|_, _| Reply::text("ok"));
    provider(&server).with_openrouter(true).enhance(&request("or-plain")).unwrap();
    assert_eq!(server.requests()[0].body["provider"], json!({"require_parameters": true}));
}

#[test]
fn other_servers_get_no_routing_fields() {
    let server = MockServer::start(|_, _| Reply::text("ok"));
    provider(&server).with_zero_data_retention(true).enhance(&request("not-or")).unwrap();
    assert!(server.requests()[0].body.get("provider").is_none());
}

#[test]
fn authorization_header_comes_from_the_secret_store() {
    let server = MockServer::start(|_, _| Reply::text("ok"));
    let secrets = MemoryStore::new();
    secrets.set("my-account", "sk-test-123").unwrap();
    let config = Provider {
        id: "p".into(),
        name: "P".into(),
        kind: ProviderKind::OpenAiCompatible {
            base_url: server.base_url.clone(),
            model: "from-config".into(),
            api_key_account: Some("my-account".into()),
            zero_data_retention: false,
        },
    };
    let enhancer = build_enhancer(&config, &secrets, &Ai::default(), None);
    let mut req = request("with-key");
    req.model = None;
    let response = enhancer.enhance(&req).unwrap();
    assert_eq!(response.model, "from-config", "the provider model is used without an override");
    let sent = &server.requests()[0];
    assert_eq!(sent.headers.get("authorization").map(String::as_str), Some("Bearer sk-test-123"));
    assert_eq!(sent.body["model"], "from-config");
}

#[test]
fn http_errors_other_than_400_and_422_stop_the_ladder() {
    let server = MockServer::start(|_, _| Reply::status(401, r#"{"error":"bad key"}"#));
    let error = provider(&server).enhance(&request("unauthorized")).unwrap_err();
    assert!(matches!(&error, EnhanceError::Http { status: 401, body } if body.contains("bad key")));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_refusal_is_not_retried() {
    let server = MockServer::start(|_, _| {
        Reply::ok(json!({"choices":[{"message":{"content":null,"refusal":"I cannot do that."}}]}))
    });
    let error = provider(&server).enhance(&request("refuses")).unwrap_err();
    assert_eq!(error, EnhanceError::Refused("I cannot do that.".into()));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn an_unreachable_server_is_a_network_error() {
    // Port 9 (discard) on localhost has no listener in the test environment.
    let p = OpenAiCompatible::new("p", "http://127.0.0.1:9/v1", "m");
    let error = p.enhance(&request("unreachable")).unwrap_err();
    assert!(matches!(error, EnhanceError::Network(_)), "{error:?}");
}

#[test]
fn zero_request_timeout_uses_the_provider_default() {
    let server = MockServer::start(|_, _| Reply::text("late").after(Duration::from_millis(2_000)));
    let p = provider(&server).with_default_timeout(Duration::from_millis(250));
    let mut req = request("default-timeout");
    req.timeout = Duration::ZERO;
    assert_eq!(p.enhance(&req).unwrap_err(), EnhanceError::Timeout(250));
}

#[test]
fn list_models_reads_the_data_array_and_sends_the_key() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"data":[{"id":"b-model"},{"id":"a-model"}]})));
    let models = list_models(&server.base_url, Some("sk-list")).unwrap();
    assert_eq!(models, ["a-model", "b-model"]);
    let sent = &server.requests()[0];
    assert_eq!((sent.method.as_str(), sent.path.as_str()), ("GET", "/v1/models"));
    assert_eq!(sent.headers.get("authorization").map(String::as_str), Some("Bearer sk-list"));
}

#[test]
fn list_models_reports_http_errors() {
    let server = MockServer::start(|_, _| Reply::status(401, "nope"));
    assert!(matches!(list_models(&server.base_url, None), Err(EnhanceError::Http { status: 401, .. })));
}

#[test]
fn detect_ollama_lists_model_names() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"models":[{"name":"qwen3:4b"},{"name":"llama3.2:3b"}]})));
    let names = sayso_enhance::detect_ollama_at(&server.root_url, Duration::from_secs(2)).unwrap();
    assert_eq!(names, ["llama3.2:3b", "qwen3:4b"]);
    assert_eq!(server.requests()[0].path, "/api/tags");
    assert_eq!(sayso_enhance::detect_ollama_at("http://127.0.0.1:9", Duration::from_millis(300)), None);
}

#[test]
fn test_provider_returns_the_elapsed_time() {
    let server = MockServer::start(|_, _| Reply::text("Testing one two three."));
    let ms = test_provider(&provider(&server)).unwrap();
    assert!(ms < 5_000);
}
