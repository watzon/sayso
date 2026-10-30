//! `enhance` over a real socket, with the real identity check (see `common`)
//! and fake services: the styles of a fresh install and a style step that
//! gives `[<style>] <text>`.

use sayso_core::config::{Provider, ProviderKind};
use sayso_core::enhance::EnhanceError;
use sayso_core::style::StyleLibrary;
use sayso_extension::protocol::{Capability, EnhanceReply, ErrorCode};
use sayso_extension::Client;
use serde_json::json;
use std::sync::Arc;

mod common;
use common::*;

struct World {
    _host: sayso_host::Host,
    _setup: Setup,
    _styles: tempfile::TempDir,
    services: Arc<FakeServices>,
    client: Client,
}

fn provider(id: &str, name: &str) -> Provider {
    Provider { id: id.into(), name: name.into(), kind: ProviderKind::ClaudeCli { path: None, model: "haiku".into(), config_dir: None, thinking: None } }
}

fn world(granted: &[&str]) -> World {
    let setup = setup();
    write_apps(&setup.apps, granted);
    let services = FakeServices::new(None);
    {
        let mut config = services.config.lock();
        config.ai.enabled = true;
        config.ai.providers = vec![provider("claude", "Claude"), provider("work", "Work account")];
        config.ai.default_provider = Some("claude".into());
        config.ai.active_style = "polished".into();
    }
    // A folder without style files: the built-in styles.
    let styles = tempfile::tempdir().unwrap();
    *services.styles.lock() = Some(StyleLibrary::load(styles.path()));
    let host = start_with(&setup, services.clone());
    let client = Client::connect_to(&setup.socket).unwrap();
    World { _host: host, _setup: setup, _styles: styles, services, client }
}

impl World {
    fn enhance(&self, body: serde_json::Value) -> Answer {
        self.client.request("enhance", fields(body))
    }

    fn ok(&self, body: serde_json::Value) -> EnhanceReply {
        self.enhance(body).unwrap().parse().unwrap()
    }

    fn code(&self, body: serde_json::Value) -> Option<ErrorCode> {
        refused(self.enhance(body)).code
    }
}

#[test]
fn enhance_runs_the_active_style_with_the_default_provider() {
    let world = world(&["ai.enhance"]);
    let reply = world.ok(json!({"text": "um so we was late"}));
    assert_eq!(reply, EnhanceReply { text: "[polished] um so we was late".into(), provider: "claude".into(), model: "fake-model".into() });
    let runs = world.services.style_runs.lock().clone();
    assert_eq!(runs.len(), 1);
    assert_eq!((runs[0].0.as_str(), runs[0].2.as_str(), runs[0].3.as_str()), ("polished", "claude", "um so we was late"));
}

#[test]
fn enhance_takes_a_style_a_prompt_and_a_provider() {
    let world = world(&["ai.enhance"]);
    assert_eq!(world.ok(json!({"text": "hi", "style": "email"})).text, "[email] hi");
    assert_eq!(world.ok(json!({"text": "hi", "style": "Notes"})).text, "[notes] hi", "a style by its name");
    let reply = world.ok(json!({"text": "hi", "prompt": "Translate to French.", "provider": "work"}));
    assert_eq!((reply.text.as_str(), reply.provider.as_str()), ("[app-prompt] hi", "work"));
    let runs = world.services.style_runs.lock().clone();
    assert_eq!(runs[2].1, "Translate to French.", "the prompt is the instruction of the style");
    assert_eq!(world.ok(json!({"text": "hi", "provider": "Work account"})).provider, "work", "a provider by its name");
}

#[test]
fn a_request_that_cannot_run_gets_a_code() {
    let world = world(&["ai.enhance"]);
    assert_eq!(world.code(json!({"style": "email"})), Some(ErrorCode::BadRequest), "no text");
    assert_eq!(world.code(json!({"text": " "})), Some(ErrorCode::BadRequest), "empty text");
    assert_eq!(world.code(json!({"text": "hi", "style": "email", "prompt": "p"})), Some(ErrorCode::BadRequest), "style and prompt");
    assert_eq!(world.code(json!({"text": "hi", "style": "pirate"})), Some(ErrorCode::BadRequest), "an unknown style");
    assert_eq!(world.code(json!({"text": "hi", "style": "raw"})), Some(ErrorCode::BadRequest), "a style without AI");
    assert_eq!(world.code(json!({"text": "hi", "provider": "openai"})), Some(ErrorCode::BadRequest), "an unknown provider");
    assert_eq!(world.code(json!({"text": 7})), Some(ErrorCode::BadRequest), "a field of the wrong type");
    assert!(world.services.style_runs.lock().is_empty(), "nothing ran");

    world.services.config.lock().ai.enabled = false;
    assert_eq!(world.code(json!({"text": "hi"})), Some(ErrorCode::Unavailable), "AI is off");
    world.services.config.lock().ai.enabled = true;
    *world.services.styles.lock() = None;
    assert_eq!(world.code(json!({"text": "hi"})), Some(ErrorCode::Unavailable), "a host without AI");
}

#[test]
fn a_failure_of_the_provider_gets_a_code_and_no_text() {
    let world = world(&["ai.enhance"]);
    for (error, code) in [
        (EnhanceError::Timeout(4000), Some(ErrorCode::Timeout)),
        (EnhanceError::Refused("the private text".into()), Some(ErrorCode::Refused)),
        (EnhanceError::Network("down".into()), Some(ErrorCode::Unavailable)),
        (EnhanceError::Http { status: 401, body: "the private text".into() }, Some(ErrorCode::Unavailable)),
        (EnhanceError::InvalidOutput("the private text".into()), None),
    ] {
        *world.services.style_error.lock() = Some(error.clone());
        let refusal = refused(world.enhance(json!({"text": "the private text"})));
        assert_eq!(refusal.code, code, "{error:?}");
        assert!(!refusal.message.contains("private"), "{}", refusal.message);
    }
}

#[test]
fn enhance_needs_its_capability() {
    let world = world(&[]);
    let refusal = refused(world.enhance(json!({"text": "hi"})));
    assert_eq!(refusal.code, Some(ErrorCode::Capability));
    assert_eq!(refusal.capability, Some(Capability::AiEnhance));
    assert!(world.services.style_runs.lock().is_empty());
}
