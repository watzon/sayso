//! Every adapter against a local mock server. No network, no keys.

mod common;

use common::{MockServer, Reply};
use sayso_core::speech::{SpeechError, SpeechProvider, SpeechProviderKind, SpeechRequest};
use sayso_transcribe::{check, list_models, transcribe};
use serde_json::json;
use std::time::{Duration, Instant};

/// Bytes that are not valid UTF-8, so a test proves the body is binary-safe.
const WAV: &[u8] = b"RIFF\x00\x9f\x92\x96WAVEfmt SAYSO-WAV-MARKER\xff\xfe";

fn provider(kind: SpeechProviderKind, server: &MockServer) -> SpeechProvider {
    SpeechProvider {
        id: "p".into(),
        name: "P".into(),
        kind,
        base_url: Some(server.base_url.clone()),
        api_key_account: None,
        models: vec![],
    }
}

fn request<'a>(model: &'a str, language: Option<&'a str>, vocabulary: &'a [String]) -> SpeechRequest<'a> {
    SpeechRequest { model, wav: WAV, language, vocabulary, timeout: Duration::from_secs(5) }
}

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|w| w.to_string()).collect()
}

const OPENAI_KINDS: [SpeechProviderKind; 4] = [
    SpeechProviderKind::OpenAi,
    SpeechProviderKind::Groq,
    SpeechProviderKind::Mistral,
    SpeechProviderKind::OpenAiCompatible,
];

// OpenAI-compatible

#[test]
fn openai_kinds_send_the_file_the_model_and_the_hints() {
    for kind in [SpeechProviderKind::OpenAi, SpeechProviderKind::Groq, SpeechProviderKind::OpenAiCompatible] {
        let server = MockServer::start(|_, _| Reply::ok(json!({"text": "  Hello world.\n"})));
        let vocabulary = words(&["Sayso", " ", "Kubernetes"]);
        let text = transcribe(&provider(kind, &server), Some("sk-test"), &request("whisper-1", Some("en"), &vocabulary)).unwrap();
        assert_eq!(text, "Hello world.", "{kind:?}: the transcript is trimmed");

        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        let sent = &requests[0];
        assert_eq!((sent.method.as_str(), sent.path.as_str()), ("POST", "/v1/audio/transcriptions"));
        assert_eq!(sent.header("authorization"), Some("Bearer sk-test"));
        assert!(sent.header("content-type").unwrap().starts_with("multipart/form-data; boundary="));
        let file = sent.file("file");
        assert_eq!(file.filename.as_deref(), Some("audio.wav"));
        assert_eq!(file.content_type.as_deref(), Some("audio/wav"));
        assert_eq!(file.data, WAV, "the WAV bytes arrive unchanged");
        assert_eq!(sent.field("model").as_deref(), Some("whisper-1"));
        assert_eq!(sent.field("response_format").as_deref(), Some("json"));
        assert_eq!(sent.field("language").as_deref(), Some("en"));
        assert_eq!(sent.field("prompt").as_deref(), Some("Sayso, Kubernetes"), "{kind:?}");
        assert!(sent.field("context_bias").is_none() && sent.field("keywords").is_none());
    }
}

#[test]
fn openai_without_hints_sends_only_the_basic_fields() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"text": "ok"})));
    transcribe(&provider(SpeechProviderKind::Groq, &server), Some("k"), &request("whisper-large-v3", None, &[])).unwrap();
    assert_eq!(server.requests()[0].names(), ["file", "model", "response_format"]);
}

#[test]
fn gpt_transcribe_uses_its_own_field_names() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"text": "ok"})));
    let vocabulary = words(&["Sayso", "<b>Ku\r\nbe</b>", "<>"]);
    transcribe(&provider(SpeechProviderKind::OpenAi, &server), Some("k"), &request("gpt-transcribe", Some("de"), &vocabulary)).unwrap();
    let sent = &server.requests()[0];
    assert_eq!(sent.fields("languages[]"), ["de"]);
    assert!(sent.field("language").is_none());
    assert_eq!(sent.field("keywords").as_deref(), Some("Sayso\nbKube/b"), "one word per line, markup characters removed");
    assert_eq!(sent.fields("keywords").len(), 1);
    assert!(sent.field("prompt").is_none());
}

#[test]
fn only_the_openai_kind_gets_the_gpt_transcribe_fields() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"text": "ok"})));
    let vocabulary = words(&["Sayso"]);
    transcribe(&provider(SpeechProviderKind::OpenAiCompatible, &server), None, &request("gpt-transcribe", Some("de"), &vocabulary))
        .unwrap();
    let sent = &server.requests()[0];
    assert_eq!(sent.field("language").as_deref(), Some("de"));
    assert_eq!(sent.field("prompt").as_deref(), Some("Sayso"));
    assert!(sent.field("languages[]").is_none() && sent.field("keywords").is_none());
}

#[test]
fn other_openai_models_keep_the_prompt() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"text": "ok"})));
    let vocabulary = words(&["Sayso"]);
    transcribe(&provider(SpeechProviderKind::OpenAi, &server), Some("k"), &request("gpt-4o-transcribe", None, &vocabulary)).unwrap();
    let sent = &server.requests()[0];
    assert_eq!(sent.field("prompt").as_deref(), Some("Sayso"));
    assert!(sent.field("language").is_none(), "no language, no field");
}

#[test]
fn mistral_sends_one_context_bias_field_per_word_up_to_100() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"text": "ok"})));
    let vocabulary: Vec<String> = (0..120).map(|i| format!("word{i}")).collect();
    transcribe(&provider(SpeechProviderKind::Mistral, &server), Some("k"), &request("voxtral-mini-latest", Some("fr"), &vocabulary))
        .unwrap();
    let sent = &server.requests()[0];
    let bias = sent.fields("context_bias");
    assert_eq!(bias.len(), 100);
    assert_eq!((bias[0].as_str(), bias[99].as_str()), ("word0", "word99"));
    assert_eq!(sent.field("language").as_deref(), Some("fr"));
    assert!(sent.field("prompt").is_none());
}

#[test]
fn a_long_dictionary_is_cut_to_800_characters_in_the_prompt() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"text": "ok"})));
    let vocabulary: Vec<String> = (0..500).map(|i| format!("word{i}")).collect();
    transcribe(&provider(SpeechProviderKind::OpenAiCompatible, &server), None, &request("m", None, &vocabulary)).unwrap();
    let prompt = server.requests()[0].field("prompt").unwrap();
    assert!(prompt.chars().count() <= 800 && prompt.chars().count() > 700, "{}", prompt.len());
    assert!(prompt.starts_with("word0, word1, ") && prompt.ends_with(|c: char| c.is_ascii_digit()));
}

#[test]
fn a_hinted_request_that_gets_400_or_422_is_sent_again_without_hints() {
    for status in [400, 422] {
        let server = MockServer::start(move |n, _| {
            if n == 0 { Reply::raw(status, r#"{"error":{"message":"unknown field"}}"#) } else { Reply::ok(json!({"text": "second"})) }
        });
        let vocabulary = words(&["Sayso"]);
        let text = transcribe(&provider(SpeechProviderKind::OpenAiCompatible, &server), None, &request("m", Some("en"), &vocabulary))
            .unwrap();
        assert_eq!(text, "second");
        let requests = server.requests();
        assert_eq!(requests.len(), 2, "HTTP {status}");
        assert_eq!(requests[0].names(), ["file", "model", "response_format", "language", "prompt"]);
        assert_eq!(requests[1].names(), ["file", "model", "response_format"]);
        assert_eq!(requests[1].file("file").data, WAV);
    }
}

#[test]
fn the_retry_returns_the_second_error() {
    let server = MockServer::start(|n, _| Reply::raw(if n == 0 { 400 } else { 500 }, &format!(r#"{{"error":{{"message":"try {n}"}}}}"#)));
    let error = transcribe(&provider(SpeechProviderKind::Groq, &server), Some("k"), &request("m", Some("en"), &[])).unwrap_err();
    assert_eq!(error, SpeechError::Http { status: 500, message: "try 1".into() });
    assert_eq!(server.requests().len(), 2, "one retry, not more");
}

#[test]
fn a_request_without_hints_is_not_retried() {
    let server = MockServer::start(|_, _| Reply::raw(400, r#"{"error":{"message":"bad file"}}"#));
    let blank = words(&["  "]);
    let error = transcribe(&provider(SpeechProviderKind::OpenAiCompatible, &server), None, &request("m", None, &blank)).unwrap_err();
    assert_eq!(error, SpeechError::Http { status: 400, message: "bad file".into() });
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn other_statuses_are_not_retried() {
    let server = MockServer::start(|_, _| Reply::raw(500, "oops"));
    let vocabulary = words(&["Sayso"]);
    transcribe(&provider(SpeechProviderKind::OpenAi, &server), Some("k"), &request("m", Some("en"), &vocabulary)).unwrap_err();
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn the_retry_shares_the_deadline() {
    // Each try takes 300 ms and fails with 400. The 500 ms deadline must stop the second try.
    let server = MockServer::start(|_, _| Reply::raw(400, "no").after(Duration::from_millis(300)));
    let mut req = request("m", Some("en"), &[]);
    req.timeout = Duration::from_millis(500);
    let started = Instant::now();
    let error = transcribe(&provider(SpeechProviderKind::OpenAiCompatible, &server), None, &req).unwrap_err();
    assert_eq!(error, SpeechError::Timeout(500));
    assert!(started.elapsed() < Duration::from_millis(1_200), "took {:?}", started.elapsed());
}

#[test]
fn openai_error_texts() {
    let cases = [
        (r#"{"error":{"message":"Incorrect API key","type":"x"}}"#, "Incorrect API key"),
        (r#"{"message":"top level"}"#, "top level"),
        (r#"{"detail":"a detail"}"#, "a detail"),
        (r#"{"error":"plain error"}"#, "plain error"),
        ("<html>Unauthorized</html>", "<html>Unauthorized</html>"),
    ];
    for (body, message) in cases {
        let server = MockServer::start(move |_, _| Reply::raw(401, body));
        let error = transcribe(&provider(SpeechProviderKind::Groq, &server), Some("k"), &request("m", None, &[])).unwrap_err();
        assert_eq!(error, SpeechError::Http { status: 401, message: message.into() });
    }
}

#[test]
fn an_unreadable_error_body_is_cut_to_200_characters() {
    let server = MockServer::start(|_, _| Reply::raw(502, &"x".repeat(1_000)));
    let error = transcribe(&provider(SpeechProviderKind::OpenAi, &server), Some("k"), &request("m", None, &[])).unwrap_err();
    assert_eq!(error, SpeechError::Http { status: 502, message: "x".repeat(200) });
}

#[test]
fn a_200_without_a_transcript_is_an_invalid_response() {
    for body in [r#"{"result":"no text field"}"#, r#"{"text":null}"#, "not json"] {
        let server = MockServer::start(move |_, _| Reply::raw(200, body));
        let error = transcribe(&provider(SpeechProviderKind::OpenAi, &server), Some("k"), &request("m", None, &[])).unwrap_err();
        assert!(matches!(error, SpeechError::InvalidResponse(_)), "{body}: {error:?}");
    }
}

#[test]
fn silence_is_an_empty_transcript_not_an_error() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"text": "  "})));
    assert_eq!(transcribe(&provider(SpeechProviderKind::OpenAi, &server), Some("k"), &request("m", None, &[])), Ok(String::new()));
}

#[test]
fn an_unreachable_server_is_a_network_error() {
    // Port 9 (discard) on localhost has no listener in the test environment.
    let mut p = provider(SpeechProviderKind::OpenAiCompatible, &MockServer::start(|_, _| Reply::raw(200, "")));
    p.base_url = Some("http://127.0.0.1:9/v1".into());
    let error = transcribe(&p, None, &request("m", None, &[])).unwrap_err();
    assert!(matches!(error, SpeechError::Network(_)), "{error:?}");
}

#[test]
fn a_slow_server_times_out_within_the_deadline() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"text": "late"})).after(Duration::from_millis(2_000)));
    let mut req = request("m", None, &[]);
    req.timeout = Duration::from_millis(300);
    let started = Instant::now();
    let error = transcribe(&provider(SpeechProviderKind::OpenAiCompatible, &server), None, &req).unwrap_err();
    assert_eq!(error, SpeechError::Timeout(300));
    assert!(started.elapsed() < Duration::from_millis(1_500), "took {:?}", started.elapsed());
}

#[test]
fn a_cloud_provider_without_a_key_fails_before_any_request() {
    // No base URL: the real address would be used, so the check must come first.
    for kind in SpeechProviderKind::ALL {
        let p = SpeechProvider {
            id: "p".into(),
            name: "P".into(),
            kind,
            base_url: Some("https://stt.invalid".into()),
            api_key_account: Some("speech.p".into()),
            models: vec![],
        };
        for key in [None, Some(""), Some("  \n")] {
            let error = transcribe(&p, key, &request("m", None, &[])).unwrap_err();
            assert_eq!(error, SpeechError::NotConfigured("no API key is saved".into()), "{kind:?} {key:?}");
            assert_eq!(check(&p, key, Duration::from_secs(1)), Err(SpeechError::NotConfigured("no API key is saved".into())));
        }
    }
    // The error comes back at once: no DNS lookup, no connect.
    let known = SpeechProvider {
        id: "p".into(),
        name: "P".into(),
        kind: SpeechProviderKind::OpenAi,
        base_url: None,
        api_key_account: None,
        models: vec![],
    };
    let started = Instant::now();
    assert!(matches!(transcribe(&known, None, &request("m", None, &[])), Err(SpeechError::NotConfigured(_))));
    assert!(started.elapsed() < Duration::from_millis(200));
}

#[test]
fn a_custom_endpoint_without_an_address_is_not_configured() {
    let p = SpeechProvider {
        id: "p".into(),
        name: "P".into(),
        kind: SpeechProviderKind::OpenAiCompatible,
        base_url: Some("  ".into()),
        api_key_account: None,
        models: vec![],
    };
    let expected = SpeechError::NotConfigured("no address is set".into());
    assert_eq!(transcribe(&p, Some("k"), &request("m", None, &[])), Err(expected.clone()));
    assert_eq!(check(&p, Some("k"), Duration::from_secs(1)), Err(expected.clone()));
    assert_eq!(list_models(&p, Some("k"), Duration::from_secs(1)), Err(expected));
}

#[test]
fn a_local_server_works_without_a_key_and_gets_no_authorization_header() {
    for key in [None, Some("")] {
        let server = MockServer::start(|_, _| Reply::ok(json!({"text": "local"})));
        let text = transcribe(&provider(SpeechProviderKind::OpenAiCompatible, &server), key, &request("m", None, &[])).unwrap();
        assert_eq!(text, "local");
        assert!(!server.requests()[0].headers.contains_key("authorization"));
    }
}

#[test]
fn the_user_agent_is_sayso() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"text": "ok"})));
    transcribe(&provider(SpeechProviderKind::OpenAi, &server), Some("k"), &request("m", None, &[])).unwrap();
    assert_eq!(server.requests()[0].header("user-agent"), Some("Sayso"));
}

#[test]
fn list_models_reads_the_data_array_and_sends_the_key() {
    for kind in OPENAI_KINDS {
        let server = MockServer::start(|_, _| Reply::ok(json!({"data": [{"id": "b-model"}, {"id": "a-model"}, {"id": "b-model"}, {"object": "x"}]})));
        let models = list_models(&provider(kind, &server), Some("sk-list"), Duration::from_secs(2)).unwrap();
        assert_eq!(models, ["a-model", "b-model"], "{kind:?}");
        let sent = &server.requests()[0];
        assert_eq!((sent.method.as_str(), sent.path.as_str()), ("GET", "/v1/models"));
        assert_eq!(sent.header("authorization"), Some("Bearer sk-list"));
    }
}

#[test]
fn list_models_reports_errors() {
    let server = MockServer::start(|_, _| Reply::raw(401, r#"{"error":{"message":"bad key"}}"#));
    let p = provider(SpeechProviderKind::Groq, &server);
    assert_eq!(list_models(&p, Some("k"), Duration::from_secs(2)), Err(SpeechError::Http { status: 401, message: "bad key".into() }));
    let server = MockServer::start(|_, _| Reply::ok(json!({"models": []})));
    let p = provider(SpeechProviderKind::Groq, &server);
    assert!(matches!(list_models(&p, Some("k"), Duration::from_secs(2)), Err(SpeechError::InvalidResponse(_))));
}

#[test]
fn list_models_for_other_kinds_returns_the_known_ids_without_a_request() {
    let server = MockServer::start(|_, _| Reply::raw(500, "must not be called"));
    for kind in [SpeechProviderKind::ElevenLabs, SpeechProviderKind::Deepgram, SpeechProviderKind::AssemblyAi] {
        let ids = list_models(&provider(kind, &server), None, Duration::from_secs(1)).unwrap();
        let known: Vec<String> = kind.known_models().iter().map(|m| m.id.to_string()).collect();
        assert_eq!(ids, known);
        assert!(!ids.is_empty());
    }
    assert!(server.requests().is_empty());
}

#[test]
fn check_for_the_openai_kinds_gets_models_and_sends_no_audio() {
    for kind in OPENAI_KINDS {
        let server = MockServer::start(|_, _| Reply::ok(json!({"data": []})));
        check(&provider(kind, &server), Some("sk-check"), Duration::from_secs(2)).unwrap();
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!((requests[0].method.as_str(), requests[0].path.as_str()), ("GET", "/v1/models"));
        assert_eq!(requests[0].header("authorization"), Some("Bearer sk-check"));
        assert!(requests[0].body.is_empty());
    }
}

#[test]
fn check_fails_with_the_provider_text() {
    let server = MockServer::start(|_, _| Reply::raw(401, r#"{"error":{"message":"bad key"}}"#));
    let error = check(&provider(SpeechProviderKind::OpenAi, &server), Some("k"), Duration::from_secs(2)).unwrap_err();
    assert_eq!(error, SpeechError::Http { status: 401, message: "bad key".into() });
}

#[test]
fn check_times_out() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"data": []})).after(Duration::from_millis(2_000)));
    let error = check(&provider(SpeechProviderKind::OpenAi, &server), Some("k"), Duration::from_millis(250)).unwrap_err();
    assert_eq!(error, SpeechError::Timeout(250));
}

// ElevenLabs

#[test]
fn elevenlabs_request() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"text": " Hallo Welt. ", "words": []})));
    let long = "x".repeat(50);
    let vocabulary = words(&["Sayso", &long, "Zürich"]);
    let text = transcribe(&provider(SpeechProviderKind::ElevenLabs, &server), Some("xi-key"), &request("scribe_v2", Some("de"), &vocabulary))
        .unwrap();
    assert_eq!(text, "Hallo Welt.");
    let sent = &server.requests()[0];
    assert_eq!((sent.method.as_str(), sent.path.as_str()), ("POST", "/v1/speech-to-text"));
    assert_eq!(sent.header("xi-api-key"), Some("xi-key"));
    assert!(!sent.headers.contains_key("authorization"));
    assert_eq!(sent.field("model_id").as_deref(), Some("scribe_v2"));
    assert_eq!(sent.field("tag_audio_events").as_deref(), Some("false"));
    assert_eq!(sent.field("diarize").as_deref(), Some("false"));
    assert_eq!(sent.field("language_code").as_deref(), Some("de"));
    assert_eq!(sent.fields("keyterms"), ["Sayso", "Zürich"], "a word of 50 characters is skipped");
    let file = sent.file("file");
    assert_eq!((file.filename.as_deref(), file.content_type.as_deref()), (Some("audio.wav"), Some("audio/wav")));
    assert_eq!(file.data, WAV);
}

#[test]
fn every_adapter_drops_its_hints_after_a_400() {
    // The array encoding of key terms is not confirmed for every provider. A
    // rejected hint must not cost the user the transcript.
    let cases = [
        (SpeechProviderKind::ElevenLabs, "scribe_v2", json!({"text": "second"})),
        (SpeechProviderKind::Deepgram, "nova-3", json!({"results": {"channels": [{"alternatives": [{"transcript": "second"}]}]}})),
        (SpeechProviderKind::AssemblyAi, "universal-3-5-pro", json!({"text": "second"})),
    ];
    for (kind, model, answer) in cases {
        let server = MockServer::start(move |n, _| if n == 0 { Reply::raw(422, r#"{"detail":"bad field"}"#) } else { Reply::ok(answer.clone()) });
        let vocabulary = words(&["Kubernetes"]);
        let text = transcribe(&provider(kind, &server), Some("k"), &request(model, Some("de"), &vocabulary)).unwrap();
        assert_eq!(text, "second", "{kind:?}");
        let requests = server.requests();
        assert_eq!(requests.len(), 2, "{kind:?}");
        let first = String::from_utf8_lossy(&requests[0].body).to_string() + &requests[0].path;
        assert!(first.contains("Kubernetes"), "{kind:?}: the first request has the dictionary word");
        let second = String::from_utf8_lossy(&requests[1].body).to_string() + &requests[1].path;
        assert!(!second.contains("Kubernetes"), "{kind:?}: the second request has no dictionary word");
    }
}

#[test]
fn elevenlabs_without_hints_and_with_many_words() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"text": "ok"})));
    transcribe(&provider(SpeechProviderKind::ElevenLabs, &server), Some("k"), &request("scribe_v2", None, &[])).unwrap();
    assert_eq!(server.requests()[0].names(), ["file", "model_id", "tag_audio_events", "diarize"]);

    let vocabulary: Vec<String> = (0..150).map(|i| format!("w{i}")).collect();
    transcribe(&provider(SpeechProviderKind::ElevenLabs, &server), Some("k"), &request("scribe_v2", None, &vocabulary)).unwrap();
    assert_eq!(server.requests()[1].fields("keyterms").len(), 100);
}

#[test]
fn elevenlabs_error_texts() {
    let cases = [
        (r#"{"detail":{"status":"invalid_api_key","message":"Invalid API key"}}"#, "Invalid API key"),
        (r#"{"detail":"Not allowed"}"#, "Not allowed"),
        (r#"{"detail":[{"loc":["body","model_id"],"msg":"Field required","type":"missing"}]}"#, "Field required"),
    ];
    for (body, message) in cases {
        let server = MockServer::start(move |_, _| Reply::raw(401, body));
        let error = transcribe(&provider(SpeechProviderKind::ElevenLabs, &server), Some("k"), &request("scribe_v2", None, &[])).unwrap_err();
        assert_eq!(error, SpeechError::Http { status: 401, message: message.into() });
    }
}

#[test]
fn elevenlabs_response_without_text_and_empty_text() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"words": []})));
    let p = provider(SpeechProviderKind::ElevenLabs, &server);
    assert!(matches!(transcribe(&p, Some("k"), &request("scribe_v2", None, &[])), Err(SpeechError::InvalidResponse(_))));
    let server = MockServer::start(|_, _| Reply::ok(json!({"text": ""})));
    let p = provider(SpeechProviderKind::ElevenLabs, &server);
    assert_eq!(transcribe(&p, Some("k"), &request("scribe_v2", None, &[])), Ok(String::new()));
}

#[test]
fn elevenlabs_check_gets_models_with_the_key_header() {
    let server = MockServer::start(|_, _| Reply::ok(json!([])));
    check(&provider(SpeechProviderKind::ElevenLabs, &server), Some("xi-key"), Duration::from_secs(2)).unwrap();
    let sent = &server.requests()[0];
    assert_eq!((sent.method.as_str(), sent.path.as_str()), ("GET", "/v1/models"));
    assert_eq!(sent.header("xi-api-key"), Some("xi-key"));

    let server = MockServer::start(|_, _| Reply::raw(401, r#"{"detail":{"message":"Invalid API key"}}"#));
    let error = check(&provider(SpeechProviderKind::ElevenLabs, &server), Some("k"), Duration::from_secs(2)).unwrap_err();
    assert_eq!(error, SpeechError::Http { status: 401, message: "Invalid API key".into() });
}

// Deepgram

fn deepgram_reply(text: &str) -> Reply {
    Reply::ok(json!({"results": {"channels": [{"alternatives": [{"transcript": text, "confidence": 0.99}]}]}}))
}

#[test]
fn deepgram_request_with_a_language_and_nova_3_keyterms() {
    let server = MockServer::start(|_, _| deepgram_reply(" Hello there. "));
    let vocabulary = words(&["Sayso", "New York", "a&b"]);
    let text =
        transcribe(&provider(SpeechProviderKind::Deepgram, &server), Some("dg-key"), &request("nova-3", Some("en"), &vocabulary)).unwrap();
    assert_eq!(text, "Hello there.");
    let sent = &server.requests()[0];
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.route(), "/v1/listen");
    assert_eq!(
        sent.query(),
        ["model=nova-3", "smart_format=true", "punctuate=true", "language=en", "keyterm=Sayso", "keyterm=New%20York", "keyterm=a%26b"]
    );
    assert_eq!(sent.header("authorization"), Some("Token dg-key"));
    assert_eq!(sent.header("content-type"), Some("audio/wav"));
    assert_eq!(sent.body, WAV, "the body is the raw WAV");
}

#[test]
fn deepgram_detects_the_language_when_none_is_set() {
    let server = MockServer::start(|_, _| deepgram_reply("ok"));
    transcribe(&provider(SpeechProviderKind::Deepgram, &server), Some("k"), &request("nova-3", None, &[])).unwrap();
    let query = server.requests()[0].path.clone();
    assert!(query.contains("&detect_language=true") && !query.contains("language=en") && !query.contains("&language="), "{query}");
}

#[test]
fn deepgram_sends_keyterms_only_for_nova_3() {
    let server = MockServer::start(|_, _| deepgram_reply("ok"));
    let vocabulary = words(&["Sayso"]);
    transcribe(&provider(SpeechProviderKind::Deepgram, &server), Some("k"), &request("nova-2", Some("en"), &vocabulary)).unwrap();
    transcribe(&provider(SpeechProviderKind::Deepgram, &server), Some("k"), &request("nova-3-medical", Some("en"), &vocabulary)).unwrap();
    let requests = server.requests();
    assert!(!requests[0].path.contains("keyterm"), "{}", requests[0].path);
    assert!(requests[1].path.contains("&keyterm=Sayso"), "{}", requests[1].path);
}

#[test]
fn deepgram_keyterms_stay_in_a_budget() {
    let server = MockServer::start(|_, _| deepgram_reply("ok"));
    let vocabulary: Vec<String> = (0..300).map(|i| format!("term{i}")).collect();
    transcribe(&provider(SpeechProviderKind::Deepgram, &server), Some("k"), &request("nova-3", None, &vocabulary)).unwrap();
    let terms: Vec<String> = server.requests()[0].query().iter().filter_map(|q| q.strip_prefix("keyterm=")).map(str::to_string).collect();
    assert!(terms.len() > 40 && terms.len() < 100, "{}", terms.len());
    assert!(terms.iter().map(String::len).sum::<usize>() <= 400);
}

#[test]
fn deepgram_error_texts_and_bad_responses() {
    let server = MockServer::start(|_, _| Reply::raw(401, r#"{"err_code":"INVALID_AUTH","err_msg":"Invalid credentials.","request_id":"x"}"#));
    let p = provider(SpeechProviderKind::Deepgram, &server);
    assert_eq!(
        transcribe(&p, Some("k"), &request("nova-3", None, &[])),
        Err(SpeechError::Http { status: 401, message: "Invalid credentials.".into() })
    );
    let server = MockServer::start(|_, _| Reply::raw(400, r#"{"message":"Bad request"}"#));
    let p = provider(SpeechProviderKind::Deepgram, &server);
    assert_eq!(
        transcribe(&p, Some("k"), &request("nova-3", None, &[])),
        Err(SpeechError::Http { status: 400, message: "Bad request".into() })
    );
    let server = MockServer::start(|_, _| Reply::ok(json!({"results": {"channels": []}})));
    let p = provider(SpeechProviderKind::Deepgram, &server);
    assert!(matches!(transcribe(&p, Some("k"), &request("nova-3", None, &[])), Err(SpeechError::InvalidResponse(_))));
}

#[test]
fn deepgram_empty_transcript_is_ok() {
    let server = MockServer::start(|_, _| deepgram_reply(""));
    assert_eq!(transcribe(&provider(SpeechProviderKind::Deepgram, &server), Some("k"), &request("nova-3", None, &[])), Ok(String::new()));
}

#[test]
fn deepgram_check_gets_projects() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"projects": []})));
    check(&provider(SpeechProviderKind::Deepgram, &server), Some("dg-key"), Duration::from_secs(2)).unwrap();
    let sent = &server.requests()[0];
    assert_eq!((sent.method.as_str(), sent.path.as_str()), ("GET", "/v1/projects"));
    assert_eq!(sent.header("authorization"), Some("Token dg-key"));

    let server = MockServer::start(|_, _| Reply::raw(401, r#"{"err_msg":"Invalid credentials."}"#));
    let error = check(&provider(SpeechProviderKind::Deepgram, &server), Some("k"), Duration::from_secs(2)).unwrap_err();
    assert_eq!(error, SpeechError::Http { status: 401, message: "Invalid credentials.".into() });
}

// AssemblyAI Sync

#[test]
fn assemblyai_request() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"text": " Good morning. ", "words": []})));
    let vocabulary = words(&["Sayso", "Kubernetes"]);
    let text = transcribe(
        &provider(SpeechProviderKind::AssemblyAi, &server),
        Some("aai-key"),
        &request("universal-3-5-pro", Some("en"), &vocabulary),
    )
    .unwrap();
    assert_eq!(text, "Good morning.");
    let sent = &server.requests()[0];
    assert_eq!((sent.method.as_str(), sent.path.as_str()), ("POST", "/v1/transcribe"));
    assert_eq!(sent.header("authorization"), Some("aai-key"), "no Bearer prefix");
    assert_eq!(sent.header("x-aai-model"), Some("universal-3-5-pro"));
    assert_eq!(sent.names(), ["config", "audio"], "the config part comes first");
    let parts = sent.parts();
    assert_eq!(parts[0].content_type.as_deref(), Some("application/json"));
    assert_eq!(parts[0].filename, None);
    let config: serde_json::Value = serde_json::from_slice(&parts[0].data).unwrap();
    assert_eq!(config, json!({"language_codes": ["en"], "keyterms_prompt": ["Sayso", "Kubernetes"]}));
    assert_eq!((parts[1].filename.as_deref(), parts[1].content_type.as_deref()), (Some("audio.wav"), Some("audio/wav")));
    assert_eq!(parts[1].data, WAV);
}

#[test]
fn assemblyai_config_is_empty_without_hints() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"text": ""})));
    let p = provider(SpeechProviderKind::AssemblyAi, &server);
    assert_eq!(transcribe(&p, Some("k"), &request("universal-3-5-pro", None, &[])), Ok(String::new()));
    let config: serde_json::Value = serde_json::from_str(&server.requests()[0].field("config").unwrap()).unwrap();
    assert_eq!(config, json!({}));

    let vocabulary: Vec<String> = (0..150).map(|i| format!("w{i}")).collect();
    transcribe(&p, Some("k"), &request("universal-3-5-pro", None, &vocabulary)).unwrap();
    let config: serde_json::Value = serde_json::from_str(&server.requests()[1].field("config").unwrap()).unwrap();
    assert_eq!(config["keyterms_prompt"].as_array().unwrap().len(), 100);
    assert!(config.get("language_codes").is_none());
}

#[test]
fn assemblyai_error_texts_and_bad_responses() {
    let cases = [
        (r#"{"error":"Invalid API key","error_code":"unauthorized"}"#, "Invalid API key"),
        (r#"{"detail":"Not allowed","error_code":"x"}"#, "Not allowed"),
        (r#"{"error_code":"audio_too_short"}"#, "audio_too_short"),
    ];
    for (body, message) in cases {
        let server = MockServer::start(move |_, _| Reply::raw(401, body));
        let p = provider(SpeechProviderKind::AssemblyAi, &server);
        let error = transcribe(&p, Some("k"), &request("universal-3-5-pro", None, &[])).unwrap_err();
        assert_eq!(error, SpeechError::Http { status: 401, message: message.into() });
    }
    let server = MockServer::start(|_, _| Reply::ok(json!({"words": []})));
    let p = provider(SpeechProviderKind::AssemblyAi, &server);
    assert!(matches!(transcribe(&p, Some("k"), &request("m", None, &[])), Err(SpeechError::InvalidResponse(_))));
}

#[test]
fn assemblyai_check_sends_half_a_second_of_silence() {
    let server = MockServer::start(|_, _| Reply::ok(json!({"text": ""})));
    check(&provider(SpeechProviderKind::AssemblyAi, &server), Some("aai-key"), Duration::from_secs(2)).unwrap();
    let sent = &server.requests()[0];
    assert_eq!(sent.path, "/v1/transcribe");
    assert_eq!(sent.header("authorization"), Some("aai-key"));
    assert_eq!(sent.header("x-aai-model"), Some("universal-3-5-pro"));
    let audio = sent.file("audio").data;
    assert_eq!(audio.len(), 44 + 16_000, "0.5 s at 16 kHz, 16-bit, mono");
    assert_eq!(&audio[..4], b"RIFF");
}

#[test]
fn assemblyai_check_accepts_a_bad_audio_answer_but_not_a_bad_key() {
    for (status, body, accepted) in [
        (400, r#"{"error":"Audio too short","error_code":"audio_too_short"}"#, true),
        (422, r#"{"error":"Bad audio","error_code":"bad_audio"}"#, true),
        (400, r#"{"error":"Bad config","error_code":"invalid_config"}"#, false),
        (401, r#"{"error":"Invalid API key","error_code":"unauthorized"}"#, false),
        (403, r#"{"error_code":"audio_too_short"}"#, false),
        (500, "oops", false),
    ] {
        let server = MockServer::start(move |_, _| Reply::raw(status, body));
        let result = check(&provider(SpeechProviderKind::AssemblyAi, &server), Some("k"), Duration::from_secs(2));
        assert_eq!(result.is_ok(), accepted, "HTTP {status} {body}: {result:?}");
        if !accepted {
            assert!(matches!(result, Err(SpeechError::Http { status: s, .. }) if s == status));
        }
    }
}
