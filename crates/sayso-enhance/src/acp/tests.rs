//! The ACP provider against fake agents that run in threads of the test, on
//! in-memory pipes. No process starts and no model is called.

use super::pool::{Spawner, Tail, Transport};
use super::*;
use crate::secrets::MemoryStore;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::sync::atomic::AtomicUsize;

/// What a fake agent does after a message.
enum Step {
    Go,
    /// Stop: close the pipes without an answer.
    Exit,
}

/// The side of the fake agent: it reads what Sayso sends, and writes to Sayso.
struct Peer {
    input: BufReader<PipeReader>,
    out: PipeWriter,
    /// The number of this start of the agent, from 1.
    run: usize,
    next_id: u64,
}

impl Peer {
    fn send(&mut self, message: Value) {
        writeln!(self.out, "{message}").unwrap();
    }

    fn reply(&mut self, request: &Value, result: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": request["id"], "result": result }));
    }

    fn fail(&mut self, request: &Value, code: i64, message: &str) {
        self.send(json!({ "jsonrpc": "2.0", "id": request["id"], "error": { "code": code, "message": message } }));
    }

    fn update(&mut self, session: &str, update: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": session, "update": update } }));
    }

    fn chunk(&mut self, session: &str, text: &str) {
        self.update(session, json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": text } }));
    }

    /// Send a request to Sayso and read its answer.
    fn ask(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = format!("agent-{}", self.next_id);
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let mut line = String::new();
        self.input.read_line(&mut line).unwrap();
        let answer: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(answer["id"], id);
        answer
    }
}

type Script = Arc<dyn Fn(&mut Peer, &Value) -> Step + Send + Sync>;

/// What the fake agents of a pool saw and did.
#[derive(Clone, Default)]
struct Log {
    /// Every message that Sayso sent, with the number of the run.
    seen: Arc<Mutex<Vec<(usize, Value)>>>,
    starts: Arc<AtomicUsize>,
    /// The runs that read the end of their input or exited.
    ended: Arc<AtomicUsize>,
}

impl Log {
    fn methods(&self) -> Vec<String> {
        self.seen.lock().iter().filter_map(|(_, m)| m["method"].as_str().map(str::to_string)).collect()
    }

    fn find(&self, method: &str) -> Vec<Value> {
        self.seen.lock().iter().filter(|(_, m)| m["method"] == method).map(|(_, m)| m.clone()).collect()
    }

    fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }

    /// Wait until `n` runs ended.
    fn wait_ended(&self, n: usize) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if self.ended.load(Ordering::SeqCst) >= n {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }
}

fn spawner(script: Script, log: Log) -> Spawner {
    Box::new(move |_spec| {
        let (our_reader, out) = std::io::pipe().unwrap();
        let (input, our_writer) = std::io::pipe().unwrap();
        let run = log.starts.fetch_add(1, Ordering::SeqCst) + 1;
        let (script, log) = (script.clone(), log.clone());
        std::thread::spawn(move || {
            let mut peer = Peer { input: BufReader::new(input), out, run, next_id: 0 };
            loop {
                let mut line = String::new();
                if peer.input.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let message: Value = serde_json::from_str(&line).unwrap();
                log.seen.lock().push((run, message.clone()));
                if message.get("method").is_some() && matches!(script(&mut peer, &message), Step::Exit) {
                    break;
                }
            }
            log.ended.fetch_add(1, Ordering::SeqCst);
        });
        Ok(Transport {
            reader: Box::new(our_reader),
            writer: Box::new(our_writer),
            child: None,
            stderr: Tail::finished(),
            workdir: tempfile::tempdir().unwrap(),
        })
    })
}

fn pool(script: Script, idle: Duration) -> (Arc<Pool>, Log) {
    let log = Log::default();
    (Arc::new(Pool::new(spawner(script, log.clone()), idle)), log)
}

const REPLY: &str = "{\"text\":\"He and I.\"}";

/// A well-behaved agent. `shape` is the model list of `session/new`: "options", "models" or "none".
fn standard(peer: &mut Peer, message: &Value, shape: &str) -> Step {
    let method = message["method"].as_str().unwrap_or_default();
    match method {
        "initialize" => peer.reply(
            message,
            json!({
                "protocolVersion": 1,
                "agentCapabilities": { "sessionCapabilities": { "close": {} } },
                "agentInfo": { "name": "fake", "title": "Fake Agent", "version": "0.1.0" },
                "authMethods": [{ "id": "browser", "name": "Sign in with a browser" }],
            }),
        ),
        "session/new" => {
            let id = format!("s-{}-{}", peer.run, peer.next_id);
            peer.next_id += 1;
            let result = match shape {
                "options" => json!({ "sessionId": id, "configOptions": [
                    { "id": "model-pick", "name": "Model", "category": "model", "type": "select", "currentValue": "fast",
                      "options": [{ "value": "fast", "name": "Fast" }, { "value": "big", "name": "Big", "description": "Slow" }] },
                    { "id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": "low",
                      "options": [{ "value": "low", "name": "Low" }, { "value": "high", "name": "High" }] },
                ]}),
                "models" => json!({ "sessionId": id, "models": { "currentModelId": "a", "availableModels": [
                    { "modelId": "a", "name": "Model A" }, { "modelId": "b", "name": "Model B" }] } }),
                _ => json!({ "sessionId": id }),
            };
            peer.reply(message, result);
        }
        "session/set_config_option" | "session/set_model" | "session/close" | "authenticate" => peer.reply(message, json!({})),
        "session/prompt" => {
            let session = message["params"]["sessionId"].as_str().unwrap().to_string();
            peer.update(&session, json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "thinking" } }));
            let (first, rest) = REPLY.split_at(9);
            peer.chunk(&session, first);
            peer.chunk(&session, rest);
            peer.reply(message, json!({ "stopReason": "end_turn" }));
        }
        "session/cancel" => {}
        other => peer.fail(message, -32601, &format!("Method not found: {other}")),
    }
    Step::Go
}

fn script(f: impl Fn(&mut Peer, &Value) -> Step + Send + Sync + 'static) -> Script {
    Arc::new(f)
}

fn shaped(shape: &'static str) -> Script {
    script(move |peer, message| standard(peer, message, shape))
}

fn provider(keep_running: bool, model: Option<&str>) -> Provider {
    Provider {
        id: "fake".into(),
        name: "Fake".into(),
        kind: ProviderKind::Acp {
            command: "fake-agent".into(),
            args: vec!["acp".into()],
            env: BTreeMap::new(),
            secret_env: Vec::new(),
            registry: None,
            model: model.map(str::to_string),
            keep_running,
        },
    }
}

fn agent(pool: &Arc<Pool>, provider: &Provider) -> AcpAgent {
    AcpAgent::new(provider, &MemoryStore::new()).with_pool(pool.clone())
}

fn request(timeout: Duration) -> EnhanceRequest {
    EnhanceRequest { system_prompt: "Fix the grammar.".into(), transcript: "me and him".into(), model: None, temperature: None, timeout }
}

const SOON: Duration = Duration::from_secs(5);

#[test]
fn a_request_chooses_the_model_streams_the_reply_and_refuses_the_tools() {
    let answers = Arc::new(Mutex::new(Vec::new()));
    let seen_answers = answers.clone();
    let (pool, log) = pool(
        script(move |peer, message| {
            if message["method"] == "session/prompt" {
                let session = message["params"]["sessionId"].as_str().unwrap();
                let options = json!([
                    { "optionId": "ok", "name": "Allow", "kind": "allow_once" },
                    { "optionId": "no", "name": "Reject", "kind": "reject_once" },
                ]);
                let permission = peer.ask("session/request_permission", json!({ "sessionId": session, "toolCall": { "toolCallId": "t1" }, "options": options }));
                let file = peer.ask("fs/read_text_file", json!({ "sessionId": session, "path": "/etc/hosts" }));
                let terminal = peer.ask("terminal/create", json!({ "sessionId": session, "command": "ls" }));
                seen_answers.lock().extend([permission, file, terminal]);
            }
            standard(peer, message, "options")
        }),
        IDLE_LIMIT,
    );
    let done = agent(&pool, &provider(true, Some("big"))).enhance(&request(SOON)).unwrap();
    assert_eq!((done.text.as_str(), done.provider_id.as_str(), done.model.as_str(), done.mode.as_str()), ("He and I.", "fake", "big", "acp"));
    assert!(done.report.unwrap().run_ms.is_some());

    {
        let answers = answers.lock();
        assert_eq!(answers[0]["result"], json!({ "outcome": { "outcome": "selected", "optionId": "no" } }));
        assert_eq!(answers[1]["error"]["code"], -32601);
        assert_eq!(answers[2]["error"]["code"], -32601);
    }

    let init = &log.find("initialize")[0]["params"];
    assert_eq!(init["protocolVersion"], 1);
    assert_eq!(init["clientCapabilities"], json!({ "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false }));
    assert_eq!(init["clientInfo"]["name"], "sayso");
    let new = &log.find("session/new")[0]["params"];
    assert_eq!(new["mcpServers"], json!([]));
    let cwd = std::path::Path::new(new["cwd"].as_str().unwrap());
    assert!(cwd.is_absolute() && cwd.is_dir() && std::fs::read_dir(cwd).unwrap().next().is_none(), "an empty folder");
    let set = &log.find("session/set_config_option")[0]["params"];
    assert_eq!((set["configId"].as_str(), set["value"].as_str()), (Some("model-pick"), Some("big")));
    let prompt = &log.find("session/prompt")[0]["params"]["prompt"];
    assert_eq!(prompt.as_array().unwrap().len(), 1);
    let text = prompt[0]["text"].as_str().unwrap();
    assert!(text.starts_with("Fix the grammar.\n\nDo not run commands or read files."), "{text}");
    assert!(text.ends_with("<transcript>\nme and him\n</transcript>\n"), "{text}");

    // A second request uses the same agent, in a new session, and the default model.
    let again = agent(&pool, &provider(true, None)).enhance(&request(SOON)).unwrap();
    assert_eq!(again.model, "fast");
    assert_eq!(log.starts(), 1);
    // The agent reads in order: the close of the first session came before the second session.
    assert!(!log.find("session/close").is_empty(), "the session is closed after the turn");
    assert_eq!(log.methods().iter().filter(|m| *m == "session/new").count(), 2);
}

#[test]
fn the_older_model_list_uses_set_model() {
    let (pool, log) = pool(shaped("models"), IDLE_LIMIT);
    let done = agent(&pool, &provider(true, Some("b"))).enhance(&request(SOON)).unwrap();
    assert_eq!(done.model, "b");
    assert_eq!(log.find("session/set_model")[0]["params"]["modelId"], "b");
    // The current model needs no request.
    agent(&pool, &provider(true, Some("a"))).enhance(&request(SOON)).unwrap();
    assert_eq!(log.find("session/set_model").len(), 1);
    let models = list_models_in(&pool, &provider(true, None), &MemoryStore::new(), SOON).unwrap();
    assert_eq!(models.iter().map(|m| (m.id.as_str(), m.name.as_str())).collect::<Vec<_>>(), [("a", "Model A"), ("b", "Model B")]);
}

#[test]
fn the_model_list_has_the_thinking_levels_of_the_agent() {
    let (pool, _) = pool(shaped("options"), IDLE_LIMIT);
    let models = list_models_in(&pool, &provider(true, None), &MemoryStore::new(), SOON).unwrap();
    assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["fast", "big"]);
    assert_eq!(models[1].description.as_deref(), Some("Slow"));
    assert_eq!(models[0].thinking, ["low", "high"], "the current model");
    assert!(models[1].thinking.is_empty());
}

#[test]
fn a_model_choice_on_an_agent_without_models_is_not_configured() {
    let (pool, _) = pool(shaped("none"), IDLE_LIMIT);
    let result = agent(&pool, &provider(true, Some("big"))).enhance(&request(SOON));
    assert!(matches!(result, Err(EnhanceError::NotConfigured(m)) if m.contains("choose a model")));
    assert_eq!(agent(&pool, &provider(true, None)).enhance(&request(SOON)).unwrap().model, "Fake Agent");
}

#[test]
fn an_agent_that_needs_a_sign_in_is_signed_out() {
    let (pool, _) = pool(
        script(|peer, message| match message["method"].as_str() {
            Some("session/new") => {
                peer.fail(message, -32000, "Authentication required");
                Step::Go
            }
            _ => standard(peer, message, "none"),
        }),
        IDLE_LIMIT,
    );
    let p = provider(true, None);
    let result = agent(&pool, &p).enhance(&request(SOON));
    assert!(matches!(&result, Err(EnhanceError::NotConfigured(m)) if m.starts_with("not signed in to Fake Agent")), "{result:?}");
    assert!(matches!(status_in(&pool, &p, &MemoryStore::new(), SOON), AuthStatus::SignedOut { detail } if detail.contains("not signed in")));
    assert!(matches!(list_models_in(&pool, &p, &MemoryStore::new(), SOON), Err(EnhanceError::NotConfigured(_))));
    let check = check_agent_in(&pool, "fake-agent", &[], &BTreeMap::new(), SOON).unwrap();
    assert!(!check.signed_in);
    assert!(check.models.is_empty());
}

#[test]
fn the_stop_reasons_map_to_their_errors() {
    for (reason, chunk) in [("refusal", "I will not."), ("max_tokens", "{\"te"), ("cancelled", ""), ("strange", REPLY)] {
        let (pool, _) = pool(
            script(move |peer, message| {
                if message["method"] == "session/prompt" {
                    let session = message["params"]["sessionId"].as_str().unwrap().to_string();
                    peer.chunk(&session, chunk);
                    peer.reply(message, json!({ "stopReason": reason }));
                    return Step::Go;
                }
                standard(peer, message, "none")
            }),
            IDLE_LIMIT,
        );
        let result = agent(&pool, &provider(true, None)).enhance(&request(SOON));
        match reason {
            "refusal" => assert_eq!(result, Err(EnhanceError::Refused("I will not.".into()))),
            "max_tokens" => assert!(matches!(&result, Err(EnhanceError::InvalidOutput(m)) if m.contains("max_tokens")), "{result:?}"),
            "cancelled" => assert!(matches!(&result, Err(EnhanceError::Cli(m)) if m.contains("cancelled")), "{result:?}"),
            _ => assert!(matches!(&result, Err(EnhanceError::InvalidOutput(m)) if m.contains("strange")), "{result:?}"),
        }
    }
    // A reply that is not the JSON object is invalid.
    let (pool, _) = pool(
        script(|peer, message| {
            if message["method"] == "session/prompt" {
                let session = message["params"]["sessionId"].as_str().unwrap().to_string();
                peer.chunk(&session, "Sure! Here it is.");
                peer.reply(message, json!({ "stopReason": "end_turn" }));
                return Step::Go;
            }
            standard(peer, message, "none")
        }),
        IDLE_LIMIT,
    );
    assert!(matches!(agent(&pool, &provider(true, None)).enhance(&request(SOON)), Err(EnhanceError::InvalidOutput(_))));
}

#[test]
fn a_timeout_cancels_the_turn_and_stops_the_agent() {
    let (pool, log) = pool(
        script(|peer, message| {
            // The first run never answers a prompt.
            if message["method"] == "session/prompt" && peer.run == 1 {
                return Step::Go;
            }
            standard(peer, message, "none")
        }),
        IDLE_LIMIT,
    );
    let p = provider(true, None);
    let started = Instant::now();
    let result = agent(&pool, &p).enhance(&request(Duration::from_millis(300)));
    assert_eq!(result, Err(EnhanceError::Timeout(300)));
    assert!(started.elapsed() < Duration::from_secs(2));
    let prompt_session = log.find("session/prompt")[0]["params"]["sessionId"].clone();
    assert!(log.wait_ended(1), "the agent that timed out is stopped");
    assert_eq!(log.find("session/cancel")[0]["params"]["sessionId"], prompt_session, "the turn was cancelled first");
    assert_eq!(agent(&pool, &p).enhance(&request(SOON)).unwrap().text, "He and I.");
    assert_eq!(log.starts(), 2, "the next request starts a new agent");
}

#[test]
fn a_dead_agent_is_started_again() {
    let (pool, log) = pool(
        script(|peer, message| match (peer.run, message["method"].as_str()) {
            // The first run dies when the second request opens its session.
            (1, Some("session/new")) if peer.next_id > 0 => Step::Exit,
            // The second run dies after its first answer.
            (2, Some("session/prompt")) => {
                standard(peer, message, "none");
                Step::Exit
            }
            _ => standard(peer, message, "none"),
        }),
        IDLE_LIMIT,
    );
    let p = provider(true, None);
    for _ in 0..3 {
        assert_eq!(agent(&pool, &p).enhance(&request(SOON)).unwrap().text, "He and I.");
    }
    // The second request found the agent dead in the middle and tried once more; the third found it dead before it started.
    assert_eq!(log.starts(), 3);
}

#[test]
fn an_agent_that_does_not_keep_running_stops_after_each_request() {
    let (pool, log) = pool(shaped("none"), IDLE_LIMIT);
    let p = provider(false, None);
    for n in 1..=2 {
        agent(&pool, &p).enhance(&request(SOON)).unwrap();
        assert!(log.wait_ended(n));
    }
    assert_eq!(log.starts(), 2);
}

#[test]
fn an_idle_agent_stops_after_the_idle_limit() {
    let (pool, log) = pool(shaped("none"), Duration::from_millis(100));
    agent(&pool, &provider(true, None)).enhance(&request(SOON)).unwrap();
    assert!(log.wait_ended(1), "the reaper stopped the idle agent");
    agent(&pool, &provider(true, None)).enhance(&request(SOON)).unwrap();
    assert_eq!(log.starts(), 2);
}

#[test]
fn a_changed_launch_or_a_stop_starts_a_new_agent() {
    let (pool, log) = pool(shaped("none"), IDLE_LIMIT);
    let mut p = provider(true, None);
    agent(&pool, &p).enhance(&request(SOON)).unwrap();
    let ProviderKind::Acp { env, .. } = &mut p.kind else { unreachable!() };
    env.insert("MODE".into(), "quiet".into());
    agent(&pool, &p).enhance(&request(SOON)).unwrap();
    assert_eq!(log.starts(), 2);
    assert!(log.wait_ended(1), "the agent of the old launch is stopped");
    pool.stop("fake");
    assert!(log.wait_ended(2));
}

#[test]
fn secret_variables_come_from_the_secret_store() {
    let secrets = MemoryStore::new();
    let mut p = provider(true, None);
    let ProviderKind::Acp { secret_env, env, .. } = &mut p.kind else { unreachable!() };
    secret_env.push("AGENT_KEY".into());
    env.insert("PLAIN".into(), "1".into());
    assert!(matches!(target(&p, &secrets), Err(EnhanceError::NotConfigured(m)) if m.contains("AGENT_KEY")));
    secrets.set(&secret_account("fake", "AGENT_KEY"), "sk-1").unwrap();
    let spec = target(&p, &secrets).unwrap().spec;
    assert_eq!(spec.env, [("PLAIN".to_string(), "1".to_string()), ("AGENT_KEY".to_string(), "sk-1".to_string())]);
    assert_eq!(secret_account("fake", "AGENT_KEY"), "acp.fake.AGENT_KEY");
}

#[test]
fn the_sign_in_runs_the_method_of_the_agent_then_reads_the_state() {
    let signed_in = Arc::new(AtomicBool::new(false));
    let flag = signed_in.clone();
    let (pool, log) = pool(
        script(move |peer, message| match message["method"].as_str() {
            Some("initialize") => {
                peer.reply(
                    message,
                    json!({ "protocolVersion": 1, "agentInfo": { "name": "fake", "version": "1" }, "authMethods": [
                        { "id": "tty", "name": "Terminal", "type": "terminal", "args": ["--login"] },
                        { "id": "browser", "name": "Browser", "description": "Opens a page" },
                    ]}),
                );
                Step::Go
            }
            Some("session/new") if !flag.load(Ordering::SeqCst) => {
                peer.fail(message, -32000, "Authentication required");
                Step::Go
            }
            Some("authenticate") => {
                flag.store(true, Ordering::SeqCst);
                peer.reply(message, json!({}));
                Step::Go
            }
            _ => standard(peer, message, "none"),
        }),
        IDLE_LIMIT,
    );
    let p = provider(true, None);
    let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    assert!(matches!(status_in(&pool, &p, secrets.as_ref(), SOON), AuthStatus::SignedOut { .. }));
    let sign_in = AcpSignIn::start_in(pool.clone(), &p, secrets, SOON).unwrap();
    assert_eq!(sign_in.next_event(), Some(SignInEvent::Finished(AuthStatus::SignedIn { account: None })));
    assert_eq!(sign_in.next_event(), None);
    assert_eq!(log.find("authenticate")[0]["params"], json!({ "methodId": "browser" }));
    assert_eq!(log.starts(), 1, "the same agent ran the sign-in");
}

#[test]
fn a_cancelled_sign_in_stops_and_reports_the_state() {
    let (pool, log) = pool(
        script(|peer, message| match message["method"].as_str() {
            // The sign-in waits for a browser that never comes.
            Some("authenticate") => Step::Go,
            Some("session/new") => {
                peer.fail(message, -32000, "Authentication required");
                Step::Go
            }
            _ => standard(peer, message, "none"),
        }),
        IDLE_LIMIT,
    );
    let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let sign_in = AcpSignIn::start_in(pool, &provider(true, None), secrets, SOON).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    sign_in.cancel();
    let Some(SignInEvent::Finished(AuthStatus::SignedOut { detail })) = sign_in.next_event() else { panic!("no SignedOut") };
    assert!(detail.contains("cancelled"), "{detail}");
    assert!(log.wait_ended(1), "the agent of the cancelled sign-in is stopped");
}

#[test]
fn a_terminal_sign_in_names_the_command_to_run() {
    let (pool, _) = pool(
        script(|peer, message| {
            if message["method"] == "initialize" {
                let methods = json!([{ "id": "tty", "name": "Terminal", "type": "terminal", "args": ["--login"] }]);
                peer.reply(message, json!({ "protocolVersion": 1, "agentInfo": { "name": "fake", "version": "1" }, "authMethods": methods }));
                return Step::Go;
            }
            standard(peer, message, "none")
        }),
        IDLE_LIMIT,
    );
    let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let error = AcpSignIn::start_in(pool, &provider(true, None), secrets, SOON).err().unwrap();
    assert!(matches!(&error, EnhanceError::NotConfigured(m) if m.contains("not signed in to fake") && m.contains("`fake-agent acp --login`")), "{error:?}");
}

#[test]
fn check_agent_reports_the_agent_and_stops_it() {
    let (pool, log) = pool(shaped("options"), IDLE_LIMIT);
    let check = check_agent_in(&pool, "fake-agent", &["acp".into()], &BTreeMap::new(), SOON).unwrap();
    assert_eq!(check.name, "Fake Agent");
    assert_eq!((check.version.as_deref(), check.protocol_version), (Some("0.1.0"), 1));
    assert_eq!(check.auth_methods, ["Sign in with a browser"]);
    assert!(check.signed_in);
    assert_eq!(check.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["fast", "big"]);
    assert!(log.wait_ended(1));
}

#[test]
fn another_protocol_version_is_not_configured() {
    let (pool, _) = pool(
        script(|peer, message| {
            peer.reply(message, json!({ "protocolVersion": 2, "agentInfo": { "name": "future", "version": "9" } }));
            Step::Go
        }),
        IDLE_LIMIT,
    );
    let result = check_agent_in(&pool, "fake-agent", &[], &BTreeMap::new(), SOON);
    assert!(matches!(&result, Err(EnhanceError::NotConfigured(m)) if m.contains("version 2")), "{result:?}");
}
