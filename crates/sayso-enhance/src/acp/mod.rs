//! ACP agents: any program that speaks the Agent Client Protocol
//! (<https://agentclientprotocol.com>) on its standard input and output, for
//! example `opencode acp`, `gemini --acp`, or the Claude agent of the ACP
//! project through `npx`. Sayso is the client.
//!
//! Sayso starts the agent once and keeps it running (see `pool`). Each
//! request opens a new session in an empty folder, with no MCP servers and
//! no tools: Sayso refuses every permission, file and terminal request of the
//! agent. ACP has no system prompt, so the style prompt, the rules of the
//! reply and the transcript go in one prompt.
//!
//! The agent keeps its own sign-in. Secret environment variables of a
//! provider are in the [`SecretStore`] under [`secret_account`].

mod client;
mod pool;
pub mod registry;
mod rpc;

use crate::auth::{AuthStatus, SignInEvent};
use crate::secrets::SecretStore;
use client::{AuthKind, Deadline};
use parking_lot::Mutex;
use pool::{Agent, LaunchSpec, Pool};
use sayso_core::config::{Provider, ProviderKind};
use sayso_core::enhance::{EnhanceError, EnhanceRequest, EnhanceResponse, Enhancer, ModelChoice, ProviderReport, parse_output, user_message};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

/// How long an agent that stays running may wait for a request before Sayso stops it.
pub const IDLE_LIMIT: Duration = Duration::from_secs(600);

/// How long a sign-in waits for the agent to start, before the sign-in itself.
const START_LIMIT: Duration = Duration::from_secs(60);

/// How long Sayso waits for the agent when it reads the state at the end of a sign-in.
const STATUS_TIMEOUT: Duration = Duration::from_secs(30);

/// The agents of the app.
static POOL: LazyLock<Arc<Pool>> = LazyLock::new(|| Arc::new(Pool::new(Box::new(pool::spawn_process), IDLE_LIMIT)));

/// The secret store account of the secret variable `name` of a provider.
pub fn secret_account(provider_id: &str, name: &str) -> String {
    format!("acp.{provider_id}.{name}")
}

/// Start the agent of a provider in the background, so the next request does
/// not wait for it to start. Does nothing when the agent does not stay running.
pub fn warm(provider: &Provider, secrets: &dyn SecretStore) {
    let Ok(target) = target(provider, secrets) else { return };
    if !target.keep_running {
        return;
    }
    std::thread::spawn(move || {
        if let Err(e) = POOL.run(&target.key, &target.spec, true, Deadline::after(START_LIMIT), |_| Ok(())) {
            log::debug!("could not start the agent of {}: {e}", target.key);
        }
    });
}

/// Stop the running agent of a provider, for example after the provider was removed.
pub fn stop_agent(provider_id: &str) {
    POOL.stop(provider_id);
}

/// How to run the agent of one provider.
#[derive(Debug, Clone)]
struct Target {
    key: String,
    spec: LaunchSpec,
    keep_running: bool,
}

/// The launch of an ACP provider, with the secret values from `secrets`.
fn target(provider: &Provider, secrets: &dyn SecretStore) -> Result<Target, EnhanceError> {
    let ProviderKind::Acp { command, args, env, secret_env, keep_running, .. } = &provider.kind else {
        return Err(EnhanceError::NotConfigured("this provider is not an ACP agent".into()));
    };
    if command.trim().is_empty() {
        return Err(EnhanceError::NotConfigured("the agent has no command".into()));
    }
    let mut vars: Vec<(String, String)> = env.iter().map(|(n, v)| (n.clone(), v.clone())).collect();
    for name in secret_env {
        let value = secrets
            .get(&secret_account(&provider.id, name))
            .ok_or_else(|| EnhanceError::NotConfigured(format!("no value is saved for {name}")))?;
        vars.push((name.clone(), value));
    }
    Ok(Target { key: provider.id.clone(), spec: LaunchSpec { command: command.trim().into(), args: args.clone(), env: vars }, keep_running: *keep_running })
}

/// ACP has no system prompt, so everything goes in one prompt.
fn build_prompt(request: &EnhanceRequest) -> String {
    format!(
        "{}\n\nDo not run commands or read files. Process only the transcript between the tags \
         and reply with a JSON object that has one field, \"text\", holding the final text.\n\n{}\n",
        request.system_prompt,
        user_message(&request.transcript)
    )
}

/// An ACP agent as an [`Enhancer`].
pub struct AcpAgent {
    id: String,
    /// Err when the provider cannot run: a missing secret value, or no command.
    target: Result<Target, EnhanceError>,
    model: Option<String>,
    default_timeout: Duration,
    pool: Arc<Pool>,
}

impl AcpAgent {
    /// The agent of a provider of the `acp` kind. A missing secret value gives
    /// `NotConfigured` from `enhance`.
    pub fn new(provider: &Provider, secrets: &dyn SecretStore) -> Self {
        Self {
            id: provider.id.clone(),
            target: target(provider, secrets),
            model: provider.model().map(str::to_string),
            default_timeout: Duration::from_secs(8),
            pool: POOL.clone(),
        }
    }

    /// The timeout used when a request has a zero timeout.
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    #[cfg(test)]
    fn with_pool(mut self, pool: Arc<Pool>) -> Self {
        self.pool = pool;
        self
    }
}

impl Enhancer for AcpAgent {
    fn id(&self) -> &str {
        &self.id
    }

    /// The agent sends the text to its model, which is usually in the cloud.
    fn is_cloud(&self) -> bool {
        true
    }

    fn enhance(&self, request: &EnhanceRequest) -> Result<EnhanceResponse, EnhanceError> {
        let timeout = if request.timeout.is_zero() { self.default_timeout } else { request.timeout };
        let started = Instant::now();
        let deadline = Deadline::after(timeout);
        let target = self.target.clone()?;
        let prompt = build_prompt(request);
        let wanted = request.model.clone().or_else(|| self.model.clone()).filter(|m| !m.trim().is_empty());
        let (text, model, run_ms) = self.pool.run(&target.key, &target.spec, target.keep_running, deadline, |agent| {
            let mut session = agent.client.new_session(agent.cwd(), deadline)?;
            let turn = run_turn(agent, &mut session, wanted.as_deref(), &prompt, deadline);
            agent.client.close_session(&session);
            let (turn, run) = turn?;
            let model = session.current_model.clone().unwrap_or_else(|| agent.client.label().to_string());
            Ok((turn.text, model, run.as_millis() as u64))
        })?;
        Ok(EnhanceResponse {
            text: parse_output(&text)?,
            provider_id: self.id.clone(),
            model,
            elapsed_ms: started.elapsed().as_millis() as u64,
            mode: "acp".into(),
            report: Some(ProviderReport { run_ms: Some(run_ms), ..ProviderReport::default() }),
        })
    }
}

/// Choose the model, then run the prompt. Returns the turn and its length.
fn run_turn(agent: &Agent, session: &mut client::Session, model: Option<&str>, prompt: &str, deadline: Deadline) -> Result<(client::Turn, Duration), EnhanceError> {
    if let Some(model) = model {
        agent.client.set_model(session, model, deadline)?;
    }
    let started = Instant::now();
    let turn = agent.client.prompt(&session.id, prompt, deadline)?;
    Ok((turn, started.elapsed()))
}

/// The models of the agent of a provider, from a new session.
pub(crate) fn list_models(provider: &Provider, secrets: &dyn SecretStore, timeout: Duration) -> Result<Vec<ModelChoice>, EnhanceError> {
    list_models_in(&POOL, provider, secrets, timeout)
}

fn list_models_in(pool: &Pool, provider: &Provider, secrets: &dyn SecretStore, timeout: Duration) -> Result<Vec<ModelChoice>, EnhanceError> {
    let target = target(provider, secrets)?;
    let deadline = Deadline::after(timeout);
    pool.run(&target.key, &target.spec, target.keep_running, deadline, |agent| {
        let session = agent.client.new_session(agent.cwd(), deadline)?;
        agent.client.close_session(&session);
        Ok(session.models)
    })
}

/// The sign-in state of the agent of a provider: an agent that opens a session is signed in.
pub(crate) fn status(provider: &Provider, secrets: &dyn SecretStore, timeout: Duration) -> AuthStatus {
    status_in(&POOL, provider, secrets, timeout)
}

fn status_in(pool: &Pool, provider: &Provider, secrets: &dyn SecretStore, timeout: Duration) -> AuthStatus {
    let target = match target(provider, secrets) {
        Ok(target) => target,
        Err(e) => return AuthStatus::SignedOut { detail: e.to_string() },
    };
    let deadline = Deadline::after(timeout);
    let opened = pool.run(&target.key, &target.spec, target.keep_running, deadline, |agent| {
        agent.client.new_session(agent.cwd(), deadline).map(|session| agent.client.close_session(&session))
    });
    match opened {
        Ok(()) => AuthStatus::SignedIn { account: None },
        Err(EnhanceError::NotConfigured(detail)) if detail.contains("not signed in") => AuthStatus::SignedOut { detail },
        // The command was not found, or may not run.
        Err(EnhanceError::NotConfigured(detail)) if detail.contains("could not start") || detail.contains("was not found") => {
            AuthStatus::NotInstalled
        }
        Err(EnhanceError::Timeout(ms)) => AuthStatus::Unknown { detail: format!("{} did not answer within {ms} ms", target.spec.label()) },
        Err(e) => AuthStatus::Unknown { detail: e.to_string() },
    }
}

/// A running sign-in of an ACP agent: an `authenticate` request for the first
/// method that the agent runs itself. The agent opens its own page in the browser.
pub(crate) struct AcpSignIn {
    cancelled: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
    events: Mutex<Option<Receiver<SignInEvent>>>,
}

impl AcpSignIn {
    /// Start the agent and its sign-in. Err when the agent does not start, or
    /// has no sign-in that Sayso can run: a terminal sign-in gives a message
    /// with the command to run.
    pub fn start(provider: &Provider, secrets: Arc<dyn SecretStore>, limit: Duration) -> Result<AcpSignIn, EnhanceError> {
        Self::start_in(POOL.clone(), provider, secrets, limit)
    }

    fn start_in(pool: Arc<Pool>, provider: &Provider, secrets: Arc<dyn SecretStore>, limit: Duration) -> Result<AcpSignIn, EnhanceError> {
        let target = target(provider, secrets.as_ref())?;
        let deadline = Deadline::after(limit);
        let (label, methods) = pool.run(&target.key, &target.spec, target.keep_running, deadline.within(START_LIMIT), |agent| {
            Ok((agent.client.label().to_string(), agent.client.info().auth_methods.clone()))
        })?;
        let method = match methods.iter().find(|m| m.kind == AuthKind::Agent) {
            Some(method) => method.id.clone(),
            None => return Err(no_sign_in(&label, &target.spec, &methods)),
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let (cancel, gone, provider) = (cancelled.clone(), dropped.clone(), provider.clone());
        std::thread::spawn(move || {
            let result = pool.run(&target.key, &target.spec, target.keep_running, deadline, |agent| {
                agent.client.authenticate(&method, deadline, &cancel)
            });
            if gone.load(Ordering::SeqCst) {
                return;
            }
            let status = status_in(&pool, &provider, secrets.as_ref(), STATUS_TIMEOUT);
            let status = match (result, status) {
                (Err(e), AuthStatus::SignedOut { .. }) => AuthStatus::SignedOut { detail: e.to_string() },
                (_, status) => status,
            };
            let _ = tx.send(SignInEvent::Finished(status));
        });
        Ok(AcpSignIn { cancelled, dropped, events: Mutex::new(Some(rx)) })
    }

    pub fn next_event(&self) -> Option<SignInEvent> {
        let mut events = self.events.lock();
        let event = events.as_ref()?.recv().ok();
        *events = None;
        event
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

impl Drop for AcpSignIn {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
        self.cancel();
    }
}

/// The error of an agent without a sign-in that Sayso can run.
fn no_sign_in(label: &str, spec: &LaunchSpec, methods: &[client::AuthMethod]) -> EnhanceError {
    let terminal = methods.iter().find_map(|m| match &m.kind {
        AuthKind::Terminal { command: Some(command), args } => Some(std::iter::once(command.clone()).chain(args.iter().cloned()).collect::<Vec<_>>()),
        // The method runs the agent's own command with extra arguments.
        AuthKind::Terminal { command: None, args } => {
            Some(std::iter::once(spec.command.clone()).chain(spec.args.iter().cloned()).chain(args.iter().cloned()).collect())
        }
        _ => None,
    });
    EnhanceError::NotConfigured(match terminal {
        Some(words) => format!("not signed in to {label}: sign in in a terminal with `{}`, then try again", words.join(" ")),
        None => format!("not signed in to {label}: it has no sign-in that Sayso can start; sign in with the agent's own command, then try again"),
    })
}

/// What [`check_agent`] found out about an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCheck {
    /// The title or the name that the agent gives, else the name of the command.
    pub name: String,
    pub version: Option<String>,
    /// The protocol version of the agent. Sayso speaks version 1.
    pub protocol_version: u64,
    /// The names of the sign-in methods of the agent.
    pub auth_methods: Vec<String>,
    /// False when the agent asked for a sign-in before it opened a session.
    pub signed_in: bool,
    /// The models of a new session. Empty when no session opened, or when the agent lists none.
    pub models: Vec<ModelChoice>,
}

/// Start an agent, initialize it, and try to open a session, for the "Add
/// your own agent" form. The agent is stopped afterwards. `env` holds every
/// variable, also the secret ones. `timeout` bounds the whole call.
pub fn check_agent(command: &str, args: &[String], env: &BTreeMap<String, String>, timeout: Duration) -> Result<AgentCheck, EnhanceError> {
    check_agent_in(&POOL, command, args, env, timeout)
}

fn check_agent_in(pool: &Pool, command: &str, args: &[String], env: &BTreeMap<String, String>, timeout: Duration) -> Result<AgentCheck, EnhanceError> {
    if command.trim().is_empty() {
        return Err(EnhanceError::NotConfigured("the agent has no command".into()));
    }
    let spec = LaunchSpec { command: command.trim().into(), args: args.to_vec(), env: env.iter().map(|(n, v)| (n.clone(), v.clone())).collect() };
    let deadline = Deadline::after(timeout);
    let agent: Agent = pool.start(&spec, deadline)?;
    let info = agent.client.info().clone();
    let (signed_in, models) = match agent.client.new_session(agent.cwd(), deadline) {
        Ok(session) => (true, session.models),
        Err(EnhanceError::NotConfigured(m)) if m.contains("not signed in") => (false, Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(AgentCheck {
        name: agent.client.label().to_string(),
        version: info.version,
        protocol_version: info.protocol_version,
        auth_methods: info.auth_methods.into_iter().map(|m| m.name).collect(),
        signed_in,
        models,
    })
}

#[cfg(test)]
mod tests;
