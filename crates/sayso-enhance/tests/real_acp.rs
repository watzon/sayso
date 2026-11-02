//! Runs real ACP agents with the user's own logins: `opencode acp` when
//! `opencode` is on `PATH`, and the Claude agent of the ACP project through
//! `npx`. Each agent gets one tiny prompt, twice: the first run starts the
//! agent, the second uses the running agent. The latencies are printed. Run with
//! `cargo test -p sayso-enhance --test real_acp -- --ignored --nocapture --test-threads=1`.

use sayso_core::config::{Ai, Provider, ProviderKind};
use sayso_core::enhance::EnhanceRequest;
use sayso_enhance::{MemoryStore, SecretStore, auth_status, build_enhancer, check_agent, list_provider_models, registry};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

fn provider(id: &str, command: &str, args: &[&str]) -> Provider {
    Provider {
        id: id.into(),
        name: id.into(),
        kind: ProviderKind::Acp {
            command: command.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            env: BTreeMap::new(),
            secret_env: Vec::new(),
            registry: None,
            model: None,
            keep_running: true,
        },
    }
}

/// Check the agent, read its state and models, then send the prompt twice,
/// and once more with `model` when there is one.
fn run(id: &str, command: &str, args: &[&str], model: Option<&str>) {
    let started = Instant::now();
    let arg_list: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let check = check_agent(command, &arg_list, &BTreeMap::new(), Duration::from_secs(180)).expect("check_agent");
    eprintln!("{id}: check in {} ms: {check:?}", started.elapsed().as_millis());

    let p = provider(id, command, args);
    let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let started = Instant::now();
    let status = auth_status(&p, secrets.as_ref(), Duration::from_secs(180));
    eprintln!("{id}: status in {} ms: {status:?}", started.elapsed().as_millis());
    let started = Instant::now();
    match list_provider_models(&p, secrets.as_ref(), None, Duration::from_secs(60)) {
        Ok(models) => {
            let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
            eprintln!("{id}: {} models in {} ms (pool warm): {:?}", ids.len(), started.elapsed().as_millis(), ids.iter().take(8).collect::<Vec<_>>());
            if let Some(first) = models.first() {
                eprintln!("{id}: first model {first:?}");
            }
        }
        Err(e) => eprintln!("{id}: models failed after {} ms: {e}", started.elapsed().as_millis()),
    }

    // Stop the agent, so the first prompt also measures the start.
    sayso_enhance::stop_acp_agent(id);
    std::thread::sleep(Duration::from_secs(1));
    let enhancer = build_enhancer(&p, &secrets, &Ai::default(), None);
    let mut request = EnhanceRequest {
        system_prompt: "Fix the grammar of the transcript. Change nothing else.".into(),
        transcript: "me and him goes to the store".into(),
        model: None,
        temperature: None,
        timeout: Duration::from_secs(180),
    };
    for label in ["cold", "warm", "chosen model"] {
        if label == "chosen model" {
            let Some(model) = model else { break };
            request.model = Some(model.into());
        }
        let started = Instant::now();
        let done = enhancer.enhance(&request);
        let wall = started.elapsed().as_millis();
        match done {
            Ok(done) => eprintln!(
                "{id}: {label} prompt in {wall} ms (turn {} ms), model {}: {:?}",
                done.report.as_ref().and_then(|r| r.run_ms).unwrap_or_default(),
                done.model,
                done.text
            ),
            Err(e) => panic!("{id}: {label} prompt failed after {wall} ms: {e}"),
        }
    }
    sayso_enhance::stop_acp_agent(id);
}

#[test]
#[ignore = "needs opencode on PATH and its login; sends two tiny prompts"]
fn opencode_answers_a_prompt() {
    if !on_path("opencode") {
        eprintln!("opencode is not on PATH: skipped");
        return;
    }
    run("opencode", "opencode", &["acp"], None);
}

#[test]
#[ignore = "needs npx, the network, and a Claude login; sends two tiny prompts"]
fn the_claude_agent_answers_a_prompt() {
    if !on_path("npx") {
        eprintln!("npx is not on PATH: skipped");
        return;
    }
    run("claude-acp", "npx", &["-y", "@agentclientprotocol/claude-agent-acp"], Some("haiku"));
}

/// The commands of `registry::LOCAL_COMMANDS` that are on this computer
/// speak ACP. Only `initialize` and `session/new`: no prompt is sent.
#[test]
#[ignore = "starts the local agents that are installed"]
fn the_local_commands_speak_acp() {
    for id in ["opencode", "gemini", "cursor", "kimi"] {
        let Some(launch) = registry::local_command(id) else {
            eprintln!("{id}: not on this computer");
            continue;
        };
        let started = Instant::now();
        let check = check_agent(&launch.command, &launch.args, &launch.env, Duration::from_secs(60));
        eprintln!("{id}: {} {:?} in {} ms: {check:?}", launch.command, launch.args, started.elapsed().as_millis());
        assert!(check.is_ok(), "{id} does not speak ACP");
    }
}

/// The live registry parses, and this platform has downloads.
#[test]
#[ignore = "downloads the registry"]
fn the_live_registry_parses() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry::fetch_registry(&dir.path().join("registry.json"), Duration::from_secs(20)).expect("registry");
    let with_binary = registry.agents.iter().filter(|a| a.binary().is_some()).count();
    eprintln!("registry {}: {} agents, {with_binary} with a download for {:?}", registry.version, registry.agents.len(), registry::current_platform());
    assert!(registry.agents.len() > 10);
}
