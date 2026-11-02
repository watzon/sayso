//! ACP agents in the app: the agent registry, the download of an agent, the
//! check of a command, and the icons of the registry. The protocol and the
//! running agents are in `sayso_enhance` (`acp`).

use crate::model::AppModel;
use gpui_kit::*;
use sayso_core::config::{Provider, ProviderKind, RegistryRef};
use sayso_enhance::registry::{self, Registry, RegistryAgent};
use sayso_core::enhance::EnhanceError;
use sayso_enhance::AgentCheck;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

/// The registry as the app has it.
#[derive(Debug, Clone, Default)]
pub enum RegistryState {
    #[default]
    NotLoaded,
    Loading,
    Ready(Registry),
    /// The English error. The registry could not be fetched and no copy was saved.
    Failed(String),
}

/// How a registry agent can start on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentWay {
    /// Its own command is here (the path).
    Local(String),
    /// Sayso downloaded it before.
    Installed,
    /// It runs with `npx` or `uvx`, which are here.
    Package(&'static str),
    /// Sayso can download it for this computer.
    Download,
    /// It needs `npx` or `uvx`, which are not here: the name of the tool.
    Needs(&'static str),
    /// Nothing for this computer.
    Unavailable,
}

/// How a registry agent starts here. Reads the disk: call it off the UI thread.
fn way(agent: &RegistryAgent, root: &std::path::Path) -> AgentWay {
    if let Some(local) = registry::local_command(&agent.id) {
        return AgentWay::Local(local.command);
    }
    match registry::resolve_launch(agent, root) {
        Ok(launch) if launch.command == "npx" => AgentWay::Package("npx"),
        Ok(launch) if launch.command == "uvx" => AgentWay::Package("uvx"),
        Ok(_) => AgentWay::Installed,
        Err(_) if agent.binary().is_some() => AgentWay::Download,
        Err(_) if agent.distribution.npx.is_some() => AgentWay::Needs("npx"),
        Err(_) if agent.distribution.uvx.is_some() => AgentWay::Needs("uvx"),
        Err(_) => AgentWay::Unavailable,
    }
}

/// The agents of the app: the registry, the downloads that run, and the icons.
#[derive(Default)]
pub struct Agents {
    pub registry: RegistryState,
    /// How each registry agent starts here, by id. Filled when the registry loads.
    pub ways: HashMap<String, AgentWay>,
    /// Registry ids whose download runs.
    pub installing: HashSet<String>,
    /// The last error of a download or of a check, by registry id or "custom".
    pub errors: HashMap<String, String>,
    /// The result of the last check of the command of the custom form.
    pub check: Option<Result<AgentCheck, String>>,
    pub checking: bool,
    /// Icons that are downloading, or that failed: no second try this run.
    icons_asked: HashSet<String>,
}

/// How long the registry and an icon may take.
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a download of an agent may take.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
/// How long the check of a command may take: some agents start slowly.
const CHECK_TIMEOUT: Duration = Duration::from_secs(45);

impl AppModel {
    fn registry_cache(&self) -> PathBuf {
        self.paths.cache_dir.join("acp-registry.json")
    }

    fn agent_icons_dir(&self) -> PathBuf {
        self.paths.cache_dir.join("acp-icons")
    }

    /// The folder of the agents that Sayso downloads.
    pub fn agents_dir(&self) -> PathBuf {
        self.paths.data_dir.join("agents")
    }

    /// Fetch the registry, once a run unless `force`. The saved copy shows
    /// first, and is the answer when the network fails.
    pub fn load_registry(&mut self, force: bool, cx: &mut Context<Self>) {
        if !force && matches!(self.agents.registry, RegistryState::Loading | RegistryState::Ready(_)) {
            return;
        }
        let cache = self.registry_cache();
        if let Some(saved) = registry::cached_registry(&cache) {
            self.agents.registry = RegistryState::Ready(saved);
            self.find_agent_ways(cx);
        } else {
            self.agents.registry = RegistryState::Loading;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { registry::fetch_registry(&cache, FETCH_TIMEOUT) }).await;
            let _ = this.update(cx, |m, cx| {
                match result {
                    Ok(registry) => {
                        m.agents.registry = RegistryState::Ready(registry);
                        m.find_agent_ways(cx);
                    }
                    Err(e) => {
                        log::warn!("could not fetch the ACP registry: {e}");
                        if !matches!(m.agents.registry, RegistryState::Ready(_)) {
                            m.agents.registry = RegistryState::Failed(e.to_string());
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Find out how each registry agent starts here, in the background.
    pub fn find_agent_ways(&mut self, cx: &mut Context<Self>) {
        let RegistryState::Ready(registry) = &self.agents.registry else { return };
        let agents = registry.agents.clone();
        let root = self.agents_dir();
        cx.spawn(async move |this, cx| {
            let ways = cx.background_spawn(async move { agents.iter().map(|a| (a.id.clone(), way(a, &root))).collect::<HashMap<_, _>>() }).await;
            let _ = this.update(cx, |m, cx| {
                m.agents.ways = ways;
                cx.notify();
            });
        })
        .detach();
    }

    /// The registry agent with this id, when the registry is loaded.
    pub fn registry_agent(&self, id: &str) -> Option<&RegistryAgent> {
        match &self.agents.registry {
            RegistryState::Ready(registry) => registry.agents.iter().find(|a| a.id == id),
            _ => None,
        }
    }

    /// The saved icon of a registry agent. Starts its download when it is not there.
    pub fn agent_icon(&mut self, agent: &RegistryAgent, cx: &mut Context<Self>) -> Option<PathBuf> {
        let dir = self.agent_icons_dir();
        let safe: String = agent.id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
        let path = dir.join(format!("{safe}.svg"));
        if path.is_file() {
            return Some(path);
        }
        if agent.icon.is_some() && self.agents.icons_asked.insert(agent.id.clone()) {
            let agent = agent.clone();
            cx.spawn(async move |this, cx| {
                let got = cx.background_spawn(async move { registry::icon(&agent, &dir, FETCH_TIMEOUT) }).await;
                match got {
                    Ok(_) => {
                        let _ = this.update(cx, |_, cx| cx.notify());
                    }
                    Err(e) => log::debug!("no icon: {e}"),
                }
            })
            .detach();
        }
        None
    }

    /// How a registry agent starts on this computer without a download: its
    /// own command when it is here, else the download when it is installed,
    /// else `npx` or `uvx`. None when it needs a download first, or cannot run here.
    pub fn agent_launch(&self, agent: &RegistryAgent) -> Option<registry::Launch> {
        registry::local_command(&agent.id).or_else(|| registry::resolve_launch(agent, &self.agents_dir()).ok())
    }

    /// Add a registry agent as a provider, after its download when it needs
    /// one. `done` gets the id of the new provider.
    pub fn add_registry_agent(&mut self, agent: RegistryAgent, name: String, cx: &mut Context<Self>, done: impl FnOnce(&mut Self, String, &mut Context<Self>) + 'static) {
        if let Some(launch) = self.agent_launch(&agent) {
            let id = self.save_agent(&agent, name, launch, cx);
            done(self, id, cx);
            return;
        }
        if agent.binary().is_none() || !self.agents.installing.insert(agent.id.clone()) {
            return;
        }
        self.agents.errors.remove(&agent.id);
        cx.notify();
        let root = self.agents_dir();
        cx.spawn(async move |this, cx| {
            let job = agent.clone();
            let result = cx.background_spawn(async move { registry::install_binary(&job, &root, INSTALL_TIMEOUT).and_then(|_| registry::resolve_launch(&job, &root)) }).await;
            let _ = this.update(cx, |m, cx| {
                m.agents.installing.remove(&agent.id);
                match result {
                    Ok(launch) => {
                        m.agents.ways.insert(agent.id.clone(), AgentWay::Installed);
                        let id = m.save_agent(&agent, name, launch, cx);
                        done(m, id, cx);
                    }
                    Err(e) => {
                        m.agents.errors.insert(agent.id.clone(), e.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_agent(&mut self, agent: &RegistryAgent, name: String, launch: registry::Launch, cx: &mut Context<Self>) -> String {
        let id = self.new_provider_id(&name);
        let kind = ProviderKind::Acp {
            command: launch.command,
            args: launch.args,
            env: launch.env,
            secret_env: Vec::new(),
            registry: Some(RegistryRef { id: agent.id.clone(), version: agent.version.clone() }),
            model: None,
            keep_running: true,
        };
        self.save_provider(Provider { id: id.clone(), name, kind }, None, false, cx);
        id
    }

    /// Start a command, ask it what it is, and stop it. For the custom form.
    pub fn check_agent_command(&mut self, command: String, args: Vec<String>, env: BTreeMap<String, String>, cx: &mut Context<Self>) {
        if self.agents.checking {
            return;
        }
        self.agents.checking = true;
        self.agents.check = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { sayso_enhance::check_agent(&command, &args, &env, CHECK_TIMEOUT) }).await;
            let _ = this.update(cx, |m, cx| {
                m.agents.checking = false;
                m.agents.check = Some(result.map_err(|e: EnhanceError| e.to_string()));
                cx.notify();
            });
        })
        .detach();
    }

    /// Add a command as an ACP provider. A variable whose name ends in
    /// `_KEY`, `_TOKEN`, or `_SECRET` goes to the secret store, not the config.
    pub fn add_custom_agent(&mut self, name: String, command: String, args: Vec<String>, env: BTreeMap<String, String>, cx: &mut Context<Self>) -> String {
        let id = self.new_provider_id(&name);
        let (secret, plain): (BTreeMap<String, String>, BTreeMap<String, String>) = env.into_iter().partition(|(k, _)| is_secret_name(k));
        for (key, value) in &secret {
            if let Err(e) = self.services.secrets.set(&sayso_enhance::acp_secret_account(&id, key), value) {
                log::error!("could not store {key}: {e}");
            }
        }
        let kind = ProviderKind::Acp { command, args, env: plain, secret_env: secret.into_keys().collect(), registry: None, model: None, keep_running: true };
        self.save_provider(Provider { id: id.clone(), name, kind }, None, false, cx);
        self.agents.check = None;
        id
    }
}

/// True for the name of a variable that holds a secret.
pub fn is_secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ["_KEY", "_TOKEN", "_SECRET", "PASSWORD"].iter().any(|end| upper.ends_with(end)) || upper == "KEY" || upper == "TOKEN"
}

/// Split a command line into the program and its arguments. Single and
/// double quotes keep spaces in an argument. None for an empty line.
pub fn split_command(line: &str) -> Option<(String, Vec<String>)> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, c) => word.push(c),
        }
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    let mut words = words.into_iter();
    let program = words.next().filter(|p| !p.is_empty())?;
    Some((program, words.collect()))
}

/// A command line for the eye, with quotes around words that have spaces.
pub fn join_command(command: &str, args: &[String]) -> String {
    std::iter::once(command)
        .chain(args.iter().map(String::as_str))
        .map(|w| if w.is_empty() || w.contains(char::is_whitespace) { format!("\"{w}\"") } else { w.to_string() })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Read `KEY=value` lines (or words) into variables. Lines without `=` are skipped.
pub fn parse_env(text: &str) -> BTreeMap<String, String> {
    text.split(['\n', ';'])
        .flat_map(|line| line.split_whitespace())
        .filter_map(|pair| pair.split_once('='))
        .filter(|(k, _)| !k.is_empty())
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    // No glob import: `gpui_kit::*` has its own `test` attribute.
    use super::{is_secret_name, join_command, parse_env, split_command};

    #[test]
    fn a_command_line_splits_with_quotes() {
        assert_eq!(split_command("kiro-cli acp"), Some(("kiro-cli".into(), vec!["acp".into()])));
        assert_eq!(split_command("  \"/Apps/My Agent/agent\" --acp 'a b' \"\" "), Some(("/Apps/My Agent/agent".into(), vec!["--acp".into(), "a b".into(), "".into()])));
        assert_eq!(split_command("   "), None);
        assert_eq!(join_command("/Apps/My Agent/agent", &["--acp".into(), "a b".into()]), "\"/Apps/My Agent/agent\" --acp \"a b\"");
    }

    #[test]
    fn variables_and_secrets() {
        let env = parse_env("KIRO_PROFILE=work API_KEY=sk-1\nbad\nEMPTY=");
        assert_eq!(env.get("KIRO_PROFILE").map(String::as_str), Some("work"));
        assert_eq!(env.get("EMPTY").map(String::as_str), Some(""));
        assert_eq!(env.len(), 3);
        assert!(is_secret_name("API_KEY") && is_secret_name("github_token") && !is_secret_name("KIRO_PROFILE"));
    }
}
