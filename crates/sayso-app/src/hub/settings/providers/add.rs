//! A new provider: the gallery of every kind, grouped by how it is paid for
//! or where it runs, the step that sets up a server, and the centered page of
//! a Sayso with no provider yet. Apple Intelligence is never the first choice:
//! the larger models clean up better.

use super::logo::{self, Mark};
use super::*;
use crate::agents::{AgentWay, RegistryState, join_command, parse_env, split_command};
use sayso_core::config::is_local_url;
use sayso_core::models::format_size;

/// The agents of the registry that the gallery shows first, when the registry has them.
const FEATURED_AGENTS: [&str; 5] = ["claude-acp", "codex-acp", "opencode", "gemini", "cursor"];

/// A server that the server step knows the address of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Preset {
    OpenRouter,
    OpenAi,
    Ollama,
    LmStudio,
    Other,
}

impl Preset {
    fn name(self) -> &'static str {
        match self {
            Preset::OpenRouter => "OpenRouter",
            Preset::OpenAi => "OpenAI",
            Preset::Ollama => "Ollama",
            Preset::LmStudio => "LM Studio",
            Preset::Other => "",
        }
    }

    fn url(self) -> &'static str {
        match self {
            Preset::OpenRouter => "https://openrouter.ai/api/v1",
            Preset::OpenAi => "https://api.openai.com/v1",
            Preset::Ollama => "http://localhost:11434/v1",
            Preset::LmStudio => "http://localhost:1234/v1",
            Preset::Other => "",
        }
    }

    fn mark(self) -> Mark {
        match self {
            Preset::Other => logo::SERVER,
            preset => logo::server(preset.url()),
        }
    }
}

/// Where the flow is.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum Step {
    Gallery,
    Server(Preset),
    /// The list of the agents of the registry.
    Agents,
    /// The setup of one registry agent, by its id.
    Agent(String),
    /// An agent from a command of the user's.
    CustomAgent,
}

/// Why a server was not saved. The flow keeps this, not the text, so the
/// message changes with the interface language.
#[derive(Clone, Copy)]
enum AddError {
    NoName,
    BadUrl,
}

impl AddError {
    fn text(self) -> String {
        match self {
            AddError::NoName => t!("settings.providers.error_no_name"),
            AddError::BadUrl => t!("settings.providers.error_bad_url"),
        }
    }
}

/// The open gallery, or the step after it.
pub(super) struct AddFlow {
    step: Step,
    search: Entity<InputState>,
    name: Entity<InputState>,
    url: Entity<InputState>,
    key: Entity<InputState>,
    /// The command line of a custom agent.
    command: Entity<InputState>,
    /// The variables of a custom agent: `KEY=value`, one or more.
    env: Entity<InputState>,
    error: Option<AddError>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl AddFlow {
    /// Put the placeholders in the interface language again.
    pub(super) fn relabel(&self, window: &mut Window, cx: &mut App) {
        self.search.update(cx, |s, cx| s.set_placeholder(t!("settings.providers.search"), window, cx));
        self.name.update(cx, |s, cx| s.set_placeholder(t!("settings.providers.name_placeholder"), window, cx));
        self.key.update(cx, |s, cx| s.set_placeholder(t!(crate::shell::os_text!("settings.providers.key_placeholder_mac", "settings.providers.key_placeholder_windows", "settings.providers.key_placeholder_other")), window, cx));
    }
}

/// What a tile of the gallery does.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Action {
    /// The setup of a registry agent.
    Agent(String),
    MoreAgents,
    ChatGpt,
    Claude,
    Cleanup(&'static str),
    Apple,
    /// Ollama runs here: add it with its first model.
    Ollama,
    Server(Preset),
}

/// A tile of the gallery.
struct Entry {
    key: String,
    name: String,
    detail: String,
    mark: Mark,
    /// A small label at the top right, and whether it is good news.
    tag: Option<(String, bool)>,
    enabled: bool,
    action: Action,
}

impl ProviderSettings {
    /// Open the gallery, or a step of it.
    pub(super) fn open_add(&mut self, step: Option<Step>, window: &mut Window, cx: &mut Context<Self>) {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("settings.providers.search")));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(t!("settings.providers.name_placeholder")));
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("https://openrouter.ai/api/v1"));
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!(crate::shell::os_text!("settings.providers.key_placeholder_mac", "settings.providers.key_placeholder_windows", "settings.providers.key_placeholder_other")))
                .masked(true)
        });
        let command = cx.new(|cx| InputState::new(window, cx).placeholder("kiro-cli acp"));
        let env = cx.new(|cx| InputState::new(window, cx).placeholder("KIRO_PROFILE=work"));
        let subs = vec![
            cx.subscribe_in(&search, window, |_, _, _: &InputEvent, _, cx| cx.notify()),
            cx.subscribe_in(&url, window, |_, _, _: &InputEvent, _, cx| cx.notify()),
            cx.subscribe_in(&name, window, |_, _, _: &InputEvent, _, cx| cx.notify()),
            cx.subscribe_in(&command, window, |_, _, _: &InputEvent, _, cx| cx.notify()),
        ];
        let focus = cx.focus_handle();
        let mut flow = AddFlow { step: Step::Gallery, search, name, url, key, command, env, error: None, focus, _subs: subs };
        self.model.update(cx, |m, cx| m.load_registry(false, cx));
        if let Some(step) = step {
            Self::enter(&mut flow, step, window, cx);
        } else {
            flow.search.update(cx, |s, cx| s.focus(window, cx));
        }
        self.add = Some(flow);
        cx.notify();
    }

    /// Go to a step, with its fields filled in.
    fn enter(flow: &mut AddFlow, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        flow.step = step.clone();
        flow.error = None;
        match &step {
            Step::Agents => flow.search.update(cx, |s, cx| s.focus(window, cx)),
            Step::Agent(_) => {}
            Step::CustomAgent => {
                flow.name.update(cx, |s, cx| s.set_value("", window, cx));
                flow.command.update(cx, |s, cx| s.set_value("", window, cx));
                flow.env.update(cx, |s, cx| s.set_value("", window, cx));
                flow.name.update(cx, |s, cx| s.focus(window, cx));
            }
            _ => {}
        }
        if let Step::Server(preset) = step {
            flow.name.update(cx, |s, cx| s.set_value(preset.name(), window, cx));
            flow.url.update(cx, |s, cx| s.set_value(preset.url(), window, cx));
            flow.key.update(cx, |s, cx| s.set_value("", window, cx));
            let first = if preset == Preset::Other { &flow.name } else { &flow.key };
            first.update(cx, |s, cx| s.focus(window, cx));
        }
    }

    fn close_add(&mut self, cx: &mut Context<Self>) {
        self.add = None;
        cx.notify();
    }

    /// The tiles of the gallery, in their groups.
    fn entries(&self, cx: &App) -> Vec<(String, Vec<Entry>)> {
        let m = self.model.read(cx);
        let providers = &m.config.ai.providers;
        let detected = &m.detected;
        let has = |f: &dyn Fn(&ProviderKind) -> bool| providers.iter().any(|p| f(&p.kind));
        let added = || Some((t!("settings.providers.tag_added"), false));
        let found = || Some((t!("settings.providers.tag_found"), true));

        let chatgpt = Entry {
            key: "chatgpt".into(),
            name: "ChatGPT".into(),
            detail: t!("settings.providers.gallery_chatgpt"),
            mark: logo::OPENAI,
            tag: has(&|k| matches!(k, ProviderKind::ChatGpt { .. })).then(added).flatten(),
            enabled: true,
            action: Action::ChatGpt,
        };
        let has_claude = has(&|k| matches!(k, ProviderKind::ClaudeCli { .. }));
        let claude = Entry {
            key: "claude".into(),
            name: "Claude Code".into(),
            detail: if detected.claude_cli.is_some() { t!("settings.providers.gallery_claude") } else { t!("settings.providers.gallery_claude_missing") },
            mark: logo::CLAUDE,
            tag: if has_claude { added() } else if detected.claude_cli.is_some() { found() } else { None },
            enabled: detected.claude_cli.is_some() && !has_claude,
            action: Action::Claude,
        };

        let mut local: Vec<Entry> = cleanup::catalog()
            .iter()
            .map(|info| {
                let have = providers.iter().any(|p| cleanup_info(p).is_some_and(|i| i.id == info.id));
                Entry {
                    key: info.id.into(),
                    name: info.name.into(),
                    detail: t!("settings.providers.gallery_cleanup", size = format_size(info.size_bytes)),
                    mark: logo::cleanup(info.id),
                    tag: have.then(added).flatten(),
                    enabled: !have,
                    action: Action::Cleanup(info.id),
                }
            })
            .collect();
        let ollama_running = detected.ollama.as_ref();
        local.push(Entry {
            key: "ollama".into(),
            name: "Ollama".into(),
            detail: match ollama_running {
                Some(models) if !models.is_empty() => t!("settings.providers.gallery_ollama", count = models.len()),
                Some(_) => t!("settings.providers.gallery_ollama_empty"),
                None => t!("settings.providers.gallery_server_local"),
            },
            mark: logo::OLLAMA,
            tag: ollama_running.and_then(|_| found()),
            enabled: true,
            action: if ollama_running.is_some() { Action::Ollama } else { Action::Server(Preset::Ollama) },
        });
        local.push(Entry {
            key: "lm-studio".into(),
            name: "LM Studio".into(),
            detail: t!("settings.providers.gallery_server_local"),
            mark: logo::LM_STUDIO,
            tag: None,
            enabled: true,
            action: Action::Server(Preset::LmStudio),
        });
        let has_apple = has(&|k| matches!(k, ProviderKind::AppleIntelligence));
        local.push(Entry {
            key: "apple".into(),
            name: "Apple Intelligence".into(),
            detail: if detected.apple_intelligence.is_some() { t!("settings.providers.gallery_apple") } else { t!("settings.providers.gallery_apple_unavailable") },
            mark: logo::APPLE,
            tag: has_apple.then(added).flatten(),
            enabled: detected.apple_intelligence.is_some() && !has_apple,
            action: Action::Apple,
        });

        let servers = vec![
            Entry { key: "openrouter".into(), name: "OpenRouter".into(), detail: t!("settings.providers.gallery_openrouter"), mark: logo::OPENROUTER, tag: None, enabled: true, action: Action::Server(Preset::OpenRouter) },
            Entry { key: "openai".into(), name: "OpenAI".into(), detail: t!("settings.providers.gallery_openai"), mark: logo::OPENAI, tag: None, enabled: true, action: Action::Server(Preset::OpenAi) },
            Entry { key: "other".into(), name: t!("settings.providers.gallery_other"), detail: t!("settings.providers.gallery_other_detail"), mark: logo::SERVER, tag: None, enabled: true, action: Action::Server(Preset::Other) },
        ];
        vec![
            (t!("settings.providers.gallery_group_plans"), vec![chatgpt, claude]),
            (t!("settings.providers.group_on_this_computer", computer = crate::shell::computer()), local),
            (t!("settings.providers.gallery_group_keys"), servers),
            (t!("settings.providers.gallery_group_agents"), self.agent_entries(cx)),
        ]
    }

    /// The agents that the gallery shows: the featured agents that the
    /// registry has, the ones found on this computer first, and More agents.
    fn agent_entries(&self, cx: &App) -> Vec<Entry> {
        let m = self.model.read(cx);
        let added: Vec<&str> = m.config.ai.providers.iter().filter_map(|p| match &p.kind {
            ProviderKind::Acp { registry: Some(r), .. } => Some(r.id.as_str()),
            _ => None,
        }).collect();
        let mut entries: Vec<(bool, Entry)> = FEATURED_AGENTS
            .iter()
            .filter_map(|id| m.registry_agent(id))
            .map(|agent| {
                let way = m.agents.ways.get(&agent.id);
                let have = added.contains(&agent.id.as_str());
                let tag = if have {
                    Some((t!("settings.providers.tag_added"), false))
                } else if matches!(way, Some(AgentWay::Local(_))) {
                    Some((t!("settings.providers.tag_found"), true))
                } else {
                    None
                };
                let entry = Entry {
                    key: format!("agent-{}", agent.id),
                    name: agent.name.clone(),
                    detail: way_text(way),
                    mark: logo::agent(&agent.id).unwrap_or(logo::AGENT),
                    tag,
                    enabled: !have && !matches!(way, Some(AgentWay::Unavailable)),
                    action: Action::Agent(agent.id.clone()),
                };
                (matches!(way, Some(AgentWay::Local(_))), entry)
            })
            .collect();
        entries.sort_by_key(|(found, _)| !*found);
        let mut entries: Vec<Entry> = entries.into_iter().map(|(_, e)| e).take(5).collect();
        entries.push(Entry {
            key: "more-agents".into(),
            name: t!("settings.providers.gallery_more_agents"),
            detail: t!("settings.providers.gallery_more_agents_detail"),
            mark: Mark::Glyph(Icon::More),
            tag: None,
            enabled: true,
            action: Action::MoreAgents,
        });
        entries
    }

    /// Do what a tile says.
    fn pick(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        let detected = self.model.read(cx).detected.clone();
        let make = |m: &AppModel, name: &str, kind: ProviderKind| Provider { id: m.new_provider_id(name), name: name.to_string(), kind };
        let id = match action {
            Action::Agent(agent) => {
                let name = self.model.read(cx).registry_agent(&agent).map(|a| a.name.clone()).unwrap_or_default();
                if let Some(flow) = self.add.as_mut() {
                    flow.name.update(cx, |s, cx| s.set_value(name, window, cx));
                    Self::enter(flow, Step::Agent(agent), window, cx);
                }
                cx.notify();
                return;
            }
            Action::MoreAgents => {
                if let Some(flow) = self.add.as_mut() {
                    flow.search.update(cx, |s, cx| s.set_value("", window, cx));
                    Self::enter(flow, Step::Agents, window, cx);
                }
                cx.notify();
                return;
            }
            Action::Server(preset) => {
                if let Some(flow) = self.add.as_mut() {
                    Self::enter(flow, Step::Server(preset), window, cx);
                } else {
                    self.open_add(Some(Step::Server(preset)), window, cx);
                }
                cx.notify();
                return;
            }
            Action::Cleanup(model) => {
                self.model.update(cx, |m, cx| {
                    let make_default = m.config.ai.default_provider.is_none();
                    m.add_cleanup_model(model, make_default, cx);
                });
                model.to_string()
            }
            Action::ChatGpt => {
                let p = make(self.model.read(cx), "ChatGPT", ProviderKind::ChatGpt { model: None, thinking: None });
                let id = p.id.clone();
                self.model.update(cx, |m, cx| {
                    m.save_provider(p, None, false, cx);
                    m.sign_in_provider(&id, cx);
                });
                id
            }
            Action::Claude => {
                // Haiku is the fast choice for cleanup; the picker offers the rest.
                let kind = ProviderKind::ClaudeCli { path: detected.claude_cli.map(|p| p.display().to_string()), model: "haiku".into(), config_dir: None, thinking: None };
                let p = make(self.model.read(cx), "Claude Code", kind);
                let id = p.id.clone();
                self.model.update(cx, |m, cx| m.save_provider(p, None, false, cx));
                id
            }
            Action::Apple => {
                let p = make(self.model.read(cx), "Apple Intelligence", ProviderKind::AppleIntelligence);
                let id = p.id.clone();
                self.model.update(cx, |m, cx| m.save_provider(p, None, false, cx));
                id
            }
            Action::Ollama => {
                // With no model pulled, the picker opens after Add instead.
                let first = detected.ollama.and_then(|models| models.first().cloned()).unwrap_or_default();
                let kind = ProviderKind::OpenAiCompatible { base_url: Preset::Ollama.url().into(), model: first, api_key_account: None, zero_data_retention: false, thinking: None };
                let p = make(self.model.read(cx), "Ollama", kind);
                let id = p.id.clone();
                self.model.update(cx, |m, cx| m.save_provider(p, None, false, cx));
                id
            }
        };
        self.after_add(&id, window, cx);
    }

    /// Close the flow, show the new provider, fetch its models, and open the
    /// picker when it cannot run without a model.
    fn after_add(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.add = None;
        self.open_row(id, window, cx);
        let needs_model = self.provider(id, cx).is_some_and(|p| p.needs_model());
        let key = id.to_string();
        self.model.update(cx, |m, cx| m.load_provider_models(&key, true, cx));
        if needs_model {
            // The trigger records its place when it is drawn; open after that.
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(120)).await;
                let _ = this.update(cx, |this, cx| this.open_models(&key, cx));
            })
            .detach();
        }
        cx.notify();
    }

    /// Save the server of the server step.
    fn save_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(flow) = self.add.as_mut() else { return };
        let name = flow.name.read(cx).value().trim().to_string();
        let url = flow.url.read(cx).value().trim().trim_end_matches('/').to_string();
        if name.is_empty() {
            flow.error = Some(AddError::NoName);
        } else if !(url.starts_with("http://") || url.starts_with("https://")) {
            flow.error = Some(AddError::BadUrl);
        }
        if flow.error.is_some() {
            cx.notify();
            return;
        }
        let key = Some(flow.key.read(cx).value().trim().to_string()).filter(|k| !k.is_empty());
        let id = self.model.read(cx).new_provider_id(&name);
        let kind = ProviderKind::OpenAiCompatible {
            api_key_account: key.as_ref().map(|_| format!("provider.{id}")),
            zero_data_retention: url.contains("openrouter.ai"),
            base_url: url,
            model: String::new(),
            thinking: None,
        };
        let provider = Provider { id: id.clone(), name, kind };
        self.model.update(cx, |m, cx| m.save_provider(provider, key, false, cx));
        self.after_add(&id, window, cx);
    }

    /// The flow over the page. Escape, the backdrop, or Close dismiss it.
    pub(super) fn add_modal(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let Some(flow) = self.add.as_ref() else { return div().into_any_element() };
        let (title, body, width) = match flow.step.clone() {
            Step::Gallery => (t!("settings.providers.add_title"), self.gallery(flow, cx), 720.),
            Step::Server(preset) => (if preset == Preset::Other { t!("settings.providers.gallery_other") } else { preset.name().to_string() }, self.server_step(flow, preset, cx), 560.),
            Step::Agents => (t!("settings.providers.agents_title"), self.agents_step(flow, cx), 640.),
            Step::Agent(id) => {
                let name = self.model.read(cx).registry_agent(&id).map(|a| a.name.clone()).unwrap_or_else(|| id.clone());
                (name, self.agent_step(flow, &id, cx), 560.)
            }
            Step::CustomAgent => (t!("settings.providers.custom_agent_title"), self.custom_agent_step(flow, cx), 580.),
        };
        let crumb = match flow.step {
            Step::Gallery => None,
            _ => Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(pkit::link("add-back", t!("settings.providers.add_title"), 13., cx).on_click(cx.listener(|this, _, window, cx| {
                        if let Some(flow) = this.add.as_mut() {
                            Self::enter(flow, Step::Gallery, window, cx);
                            flow.search.update(cx, |s, cx| s.focus(window, cx));
                        }
                        cx.notify();
                    })))
                    .child(plain("›", 13., 16., FontWeight::MEDIUM, c.pencil))
                    .child(plain(title.clone(), 13., 16., FontWeight::SEMIBOLD, c.ink)),
            ),
        };
        let close = pkit::icon_button("add-close", Icon::Close, 14., cx).label(t!("settings.providers.close")).on_click(cx.listener(|this, _, _, cx| this.close_add(cx)));
        let top = match crumb {
            Some(crumb) => div().flex().items_center().justify_between().child(crumb).child(close),
            None => div().flex().items_center().justify_between().child(text::display(title.clone(), 26., &c)).child(close),
        };
        div()
            .id("add-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(c.shadow(0.22))
            .occlude()
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close_add(cx);
                }
            }))
            // A click outside the sheet closes it. Close is the keyboard way.
            .child(Pressable::new("add-scrim").label(t!("settings.providers.close")).tab_stop(false).absolute().inset_0().on_click(cx.listener(|this, _, _, cx| this.close_add(cx))))
            .child(
                div()
                    .id("add-modal")
                    .role(Role::Dialog)
                    .aria_label(title)
                    .track_focus(&flow.focus)
                    .occlude()
                    .flex()
                    .flex_col()
                    .w(px(width))
                    .max_h(relative(0.88))
                    .px(px(26.))
                    .pt(px(22.))
                    .pb(px(24.))
                    .rounded(px(16.))
                    .bg(c.sheet_raised)
                    .shadow(sayso_ui::paper::floating(&c))
                    .child(top)
                    .child(div().id("add-body").flex_1().min_h_0().overflow_y_scroll().child(body)),
            )
            .into_any_element()
    }

    /// The search field and the groups of tiles.
    fn gallery(&self, flow: &AddFlow, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let query = flow.search.read(cx).value().trim().to_lowercase();
        let mut col = div().flex().flex_col().child(div().pt(px(14.)).child(pkit::input_well_named(&flow.search, Some(t!("settings.providers.search").into()), cx)));
        let mut any = false;
        for (title, entries) in self.entries(cx) {
            let shown: Vec<Entry> = entries.into_iter().filter(|e| query.is_empty() || e.name.to_lowercase().contains(&query) || e.detail.to_lowercase().contains(&query)).collect();
            if shown.is_empty() {
                continue;
            }
            any = true;
            col = col.child(div().pt(px(18.)).pb(px(8.)).child(pkit::caps(title, 11., c.graphite)));
            for chunk in shown.chunks(3) {
                let mut row = div().flex().gap(px(10.)).pb(px(10.));
                for entry in chunk {
                    row = row.child(self.tile(entry, cx));
                }
                for _ in chunk.len()..3 {
                    row = row.child(div().flex_1().min_w_0());
                }
                col = col.child(row);
            }
        }
        if !any {
            col = col.child(div().py(px(28.)).child(ui(t!("settings.providers.search_none"), 13., 18., FontWeight::NORMAL, c.graphite)));
        }
        col.into_any_element()
    }

    fn tile(&self, entry: &Entry, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let action = entry.action.clone();
        let tag = entry.tag.clone().map(|(label, good)| {
            let (bg, fg) = if good { (c.success.opacity(0.14), c.success) } else { (c.deboss, c.graphite) };
            div().ml_auto().px(px(6.)).py(px(2.)).rounded(px(5.)).bg(bg).child(pkit::caps(label, 10., fg))
        });
        let mut tile = Pressable::new(SharedString::from(format!("gallery-{}", entry.key)))
            .label(format!("{}, {}", entry.name, entry.detail))
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(8.))
            .p(px(12.))
            .rounded(px(12.))
            .bg(c.sheet)
            .border_1()
            .border_color(c.rule)
            .child(div().flex().items_center().child(logo::tile(entry.mark.clone(), 30., cx)).children(tag))
            .child(div().flex().flex_col().gap(px(2.)).child(plain(entry.name.clone(), 14., 18., FontWeight::SEMIBOLD, c.ink)).child(plain(entry.detail.clone(), 12., 16., FontWeight::NORMAL, c.graphite)));
        if entry.enabled {
            tile = tile.cursor_pointer().on_click(cx.listener(move |this, _, window, cx| this.pick(action.clone(), window, cx)));
        } else {
            tile = tile.opacity(0.6).disabled(true);
        }
        tile.into_any_element()
    }

    /// Name, address, and key of a server.
    fn server_step(&self, flow: &AddFlow, preset: Preset, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let url = flow.url.read(cx).value().to_string();
        let cloud = !url.trim().is_empty() && !is_local_url(&url);
        let about = match preset {
            Preset::OpenRouter => t!("settings.providers.server_openrouter"),
            Preset::OpenAi => t!("settings.providers.server_openai"),
            Preset::Ollama | Preset::LmStudio => t!("settings.providers.server_local", name = preset.name()),
            Preset::Other => t!("settings.providers.server_other"),
        };
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .pt(px(16.))
            .child(div().flex().items_center().gap(px(14.)).child(logo::tile(preset.mark(), 44., cx)).child(ui(about, 13., 18., FontWeight::NORMAL, c.graphite).flex_1()))
            .child(pkit::field(&t!("settings.providers.name"), pkit::input_well_named(&flow.name, Some(t!("settings.providers.name").into()), cx), None, cx))
            .child(pkit::field(&t!("settings.providers.base_url"), pkit::input_well_named(&flow.url, Some(t!("settings.providers.base_url").into()), cx).font_family(sayso_ui::fonts::MONO), None, cx))
            .child(pkit::field(&t!("settings.providers.api_key"), pkit::secret_well_named(&flow.key, t!("settings.providers.api_key").into(), cx), Some(&t!("settings.providers.api_key_hint")), cx));
        if cloud {
            col = col.child(Banner::new(BannerKind::Info, t!("settings.providers.cloud_notice")));
        }
        if let Some(error) = flow.error {
            col = col.child(Banner::new(BannerKind::Warning, error.text()));
        }
        col.child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .child(Button::new("add-server-back", t!("settings.providers.back")).ghost().on_click(cx.listener(|this, _, window, cx| {
                    if let Some(flow) = this.add.as_mut() {
                        Self::enter(flow, Step::Gallery, window, cx);
                    }
                    cx.notify();
                })))
                .child(Button::new("add-server-save", t!("settings.providers.add_provider")).primary().on_click(cx.listener(|this, _, window, cx| this.save_server(window, cx)))),
        )
        .into_any_element()
    }

    /// The page of a Sayso with no provider: three places to start, and the gallery.
    pub(super) fn empty_state(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let detected = self.model.read(cx).detected.clone();
        // A subscription first. Apple Intelligence last, when it can run here.
        let mut cards: Vec<(Mark, String, String, String, Option<String>, Action)> = Vec::new();
        if detected.claude_cli.is_some() {
            cards.push((logo::CLAUDE, t!("settings.providers.start_plan"), "Claude Code".into(), t!("settings.providers.start_claude"), Some(t!("settings.providers.tag_found")), Action::Claude));
        }
        cards.push((logo::OPENAI, t!("settings.providers.start_plan"), "ChatGPT".into(), t!("settings.providers.start_chatgpt"), None, Action::ChatGpt));
        if detected.claude_cli.is_none() {
            cards.push((logo::OPENROUTER, t!("settings.providers.start_key"), "OpenRouter".into(), t!("settings.providers.start_openrouter"), None, Action::Server(Preset::OpenRouter)));
        }
        if detected.apple_intelligence.is_some() {
            cards.push((logo::APPLE, t!("settings.providers.start_private"), "Apple Intelligence".into(), t!("settings.providers.start_apple"), None, Action::Apple));
        }
        let mut row = div().flex().justify_center().gap(px(16.)).pt(px(34.));
        for (i, (mark, kicker, name, detail, tag, action)) in cards.into_iter().enumerate() {
            let label = match action {
                Action::ChatGpt => t!("settings.providers.sign_in"),
                Action::Claude => t!("settings.providers.start_add", name = name),
                Action::Apple => t!("settings.providers.start_use", name = name),
                _ => t!("settings.providers.start_set_up"),
            };
            let action = action.clone();
            let mut button = Button::new(SharedString::from(format!("start-{i}")), label.clone()).a11y_label(format!("{label}, {name}"));
            if i == 0 {
                button = button.primary();
            }
            let button = button.on_click(cx.listener(move |this, _, window, cx| this.pick(action.clone(), window, cx)));
            let tag = tag.map(|tag| div().px(px(6.)).py(px(2.)).rounded(px(5.)).bg(c.success.opacity(0.14)).child(pkit::caps(tag, 10., c.success)));
            row = row.child(
                div()
                    .id(SharedString::from(format!("start-card-{i}")))
                    .role(Role::Group)
                    .aria_label(name.clone())
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(14.))
                    .w(px(236.))
                    .px(px(18.))
                    .pt(px(24.))
                    .pb(px(20.))
                    .rounded(px(16.))
                    .bg(c.sheet_raised)
                    .shadow(sayso_ui::paper::raised(&c))
                    .child(logo::tile(mark, 48., cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(4.))
                            .child(div().flex().items_center().gap(px(6.)).h(px(16.)).child(pkit::caps(kicker, 11., c.pencil)).children(tag))
                            .child(plain(name, 18., 22., FontWeight::SEMIBOLD, c.ink))
                            .child(plain(detail, 13., 18., FontWeight::NORMAL, c.graphite).text_center()),
                    )
                    .child(button),
            );
        }
        let marks = div().flex().children([logo::cleanup("s1-mini"), logo::OLLAMA, logo::OPENROUTER, logo::LM_STUDIO, logo::AGENT].into_iter().enumerate().map(|(i, mark)| {
            div().when(i > 0, |d| d.ml(px(-6.))).child(logo::tile(mark, 24., cx))
        }));
        let more = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .mt(px(30.))
            .pl(px(14.))
            .pr(px(8.))
            .py(px(8.))
            .rounded_full()
            .debossed(&c)
            .child(marks)
            .child(ui(t!("settings.providers.start_more"), 13., 16., FontWeight::NORMAL, c.graphite))
            .child(Button::new("start-see-all", t!("settings.providers.see_all")).small().on_click(cx.listener(|this, _, window, cx| this.open_add(None, window, cx))));
        div()
            .id("providers-empty")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px(px(44.))
            .pb(px(24.))
            .overflow_y_scroll()
            .child(pkit::caps(t!("settings.providers.title"), 12., c.graphite))
            .child(div().pt(px(14.)).child(text::display(t!("settings.providers.start_title"), 44., &c).text_center()))
            .child(div().pt(px(14.)).max_w(px(600.)).child(text::body(t!("settings.providers.start_body"), &c).text_center()))
            .child(row)
            .child(more)
            .into_any_element()
    }
}

/// How a registry agent starts here, in a few words.
fn way_text(way: Option<&AgentWay>) -> String {
    match way {
        None => t!("settings.providers.agent_way_unknown"),
        Some(AgentWay::Local(_)) => t!("settings.providers.agent_way_found"),
        Some(AgentWay::Installed) => t!("settings.providers.agent_way_installed"),
        Some(AgentWay::Package(tool)) => t!("settings.providers.agent_way_package", tool = tool),
        Some(AgentWay::Download) => t!("settings.providers.agent_way_download"),
        Some(AgentWay::Needs(tool)) => t!("settings.providers.agent_way_needs", tool = tool),
        Some(AgentWay::Unavailable) => t!("settings.providers.agent_way_unavailable"),
    }
}

impl ProviderSettings {
    /// The mark of a registry agent: a bundled mark, else its downloaded icon.
    fn registry_mark(&self, agent: &sayso_enhance::registry::RegistryAgent, cx: &mut Context<Self>) -> Mark {
        if let Some(mark) = logo::agent(&agent.id) {
            return mark;
        }
        let agent = agent.clone();
        self.model
            .update(cx, |m, cx| m.agent_icon(&agent, cx))
            .map(|path| Mark::External(path.to_string_lossy().into_owned().into()))
            .unwrap_or(logo::AGENT)
    }

    /// A line in a colored box: good news, a problem, or a note.
    fn note_line(text: String, tone: Tone, cx: &App) -> Div {
        let c = cx.paper().colors;
        let (bg, fg) = match tone {
            Tone::Good => (c.success.opacity(0.12), c.success),
            Tone::Bad => (c.danger_wash, c.danger),
            Tone::Info => (c.accent_wash, c.accent),
        };
        div().flex().items_center().gap(px(8.)).px(px(12.)).py(px(9.)).rounded(px(10.)).bg(bg).child(div().flex_none().size(px(8.)).rounded_full().bg(fg)).child(ui(text, 13., 18., FontWeight::MEDIUM, fg).flex_1())
    }

    /// The agents of the registry, with a search, and Your own command at the end.
    fn agents_step(&self, flow: &AddFlow, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let query = flow.search.read(cx).value().trim().to_lowercase();
        let (state, ways, added) = {
            let m = self.model.read(cx);
            let added: Vec<String> = m.config.ai.providers.iter().filter_map(|p| match &p.kind {
                ProviderKind::Acp { registry: Some(r), .. } => Some(r.id.clone()),
                _ => None,
            }).collect();
            (m.agents.registry.clone(), m.agents.ways.clone(), added)
        };
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(10.))
            .child(ui(t!("settings.providers.agents_intro"), 13., 18., FontWeight::NORMAL, c.graphite))
            .child(pkit::input_well_named(&flow.search, Some(t!("settings.providers.search_agents").into()), cx));
        match state {
            RegistryState::NotLoaded | RegistryState::Loading => {
                col = col.child(div().py(px(18.)).child(ui(t!("settings.providers.agents_loading"), 13., 18., FontWeight::NORMAL, c.graphite)));
            }
            RegistryState::Failed(error) => {
                col = col.child(Self::note_line(t!("settings.providers.agents_failed"), Tone::Bad, cx)).child(div().child(
                    Button::new("registry-retry", t!("settings.providers.try_again")).small().on_click(cx.listener(|this, _, _, cx| this.model.update(cx, |m, cx| m.load_registry(true, cx)))),
                ));
                log::debug!("registry: {error}");
            }
            RegistryState::Ready(registry) => {
                let rank = |id: &str| match ways.get(id) {
                    Some(AgentWay::Local(_)) => 0,
                    Some(AgentWay::Installed | AgentWay::Package(_)) => 1,
                    Some(AgentWay::Download) => 2,
                    _ => 3,
                };
                let mut agents: Vec<_> = registry
                    .agents
                    .iter()
                    .filter(|a| query.is_empty() || a.name.to_lowercase().contains(&query) || a.description.to_lowercase().contains(&query) || a.authors.iter().any(|x| x.to_lowercase().contains(&query)))
                    .collect();
                agents.sort_by_key(|a| (rank(&a.id), a.name.to_lowercase()));
                let mut list = div().id("agent-list").flex().flex_col();
                for agent in agents {
                    let way = ways.get(&agent.id);
                    let have = added.contains(&agent.id);
                    let usable = !matches!(way, Some(AgentWay::Unavailable | AgentWay::Needs(_)));
                    let by = if agent.authors.is_empty() { agent.description.clone() } else { agent.authors.join(", ") };
                    let id = agent.id.clone();
                    let right = if have {
                        div().child(pkit::caps(t!("settings.providers.tag_added"), 10., c.graphite))
                    } else {
                        let mut right = div().flex().items_center().gap(px(10.)).child(plain(way_text(way), 12., 16., FontWeight::NORMAL, if matches!(way, Some(AgentWay::Local(_))) { c.success } else { c.graphite }));
                        if usable {
                            right = right.child(Button::new(SharedString::from(format!("add-agent-{}", agent.id)), t!("settings.providers.add")).small().a11y_label(format!("{}, {}", t!("settings.providers.add"), agent.name)).on_click(cx.listener(move |this, _, window, cx| this.pick(Action::Agent(id.clone()), window, cx))));
                        }
                        right
                    };
                    list = list.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .py(px(9.))
                            .border_b_1()
                            .border_color(c.rule)
                            .child(logo::tile(self.registry_mark(agent, cx), 30., cx))
                            .child(div().flex().flex_col().flex_1().min_w_0().gap(px(1.)).child(plain(agent.name.clone(), 14., 18., FontWeight::SEMIBOLD, c.ink)).child(plain(by, 12., 16., FontWeight::NORMAL, c.graphite).truncate()))
                            .child(right),
                    );
                }
                col = col.child(list);
            }
        }
        let own = pkit::dashed("own-agent", cx)
            .label(t!("settings.providers.custom_agent_title"))
            .justify_start()
            .gap(px(12.))
            .p(px(12.))
            .rounded(px(12.))
            .on_click(cx.listener(|this, _, window, cx| {
                if let Some(flow) = this.add.as_mut() {
                    Self::enter(flow, Step::CustomAgent, window, cx);
                }
                cx.notify();
            }))
            .child(logo::tile(logo::AGENT, 30., cx))
            .child(div().flex().flex_col().flex_1().gap(px(1.)).child(plain(t!("settings.providers.custom_agent_title"), 14., 18., FontWeight::SEMIBOLD, c.ink)).child(plain(t!("settings.providers.custom_agent_short"), 12., 16., FontWeight::NORMAL, c.graphite)));
        col.child(own).into_any_element()
    }

    /// The setup of one registry agent: where it comes from, its name, and Add.
    fn agent_step(&self, flow: &AddFlow, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let (agent, way, installing, error, launch) = {
            let m = self.model.read(cx);
            let agent = m.registry_agent(id).cloned();
            let launch = agent.as_ref().and_then(|a| m.agent_launch(a));
            (agent, m.agents.ways.get(id).cloned(), m.agents.installing.contains(id), m.agents.errors.get(id).cloned(), launch)
        };
        let Some(agent) = agent else { return ui(t!("settings.providers.agents_loading"), 13., 18., FontWeight::NORMAL, c.graphite).into_any_element() };
        let where_from = match &way {
            Some(AgentWay::Local(path)) => Self::note_line(t!("settings.providers.agent_found_at", path = path), Tone::Good, cx),
            Some(AgentWay::Installed) => Self::note_line(t!("settings.providers.agent_way_installed"), Tone::Good, cx),
            Some(AgentWay::Package(tool)) => Self::note_line(t!("settings.providers.agent_runs_with", tool = tool), Tone::Info, cx),
            Some(AgentWay::Download) => Self::note_line(t!("settings.providers.agent_will_download", version = agent.version.clone()), Tone::Info, cx),
            Some(AgentWay::Needs(tool)) => Self::note_line(t!("settings.providers.agent_needs", tool = tool), Tone::Bad, cx),
            Some(AgentWay::Unavailable) | None => Self::note_line(t!("settings.providers.agent_way_unavailable"), Tone::Bad, cx),
        };
        let can_add = !installing && matches!(way, Some(AgentWay::Local(_) | AgentWay::Installed | AgentWay::Package(_) | AgentWay::Download));
        let label = if installing {
            t!("settings.providers.agent_downloading")
        } else if matches!(way, Some(AgentWay::Download)) {
            t!("settings.providers.agent_download_add")
        } else {
            t!("settings.providers.agent_add")
        };
        let key = id.to_string();
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .pt(px(16.))
            .child(div().flex().items_center().gap(px(14.)).child(logo::tile(self.registry_mark(&agent, cx), 44., cx)).child(ui(agent.description.clone(), 13., 18., FontWeight::NORMAL, c.graphite).flex_1()))
            .child(where_from)
            .child(pkit::field(&t!("settings.providers.name"), pkit::input_well_named(&flow.name, Some(t!("settings.providers.name").into()), cx), None, cx));
        if let Some(launch) = launch {
            col = col.child(pkit::field(
                &t!("settings.providers.command"),
                div().px(px(12.)).py(px(9.)).rounded(px(10.)).debossed(&c).child(text::mono(join_command(&launch.command, &launch.args), 12., c.graphite).font_weight(FontWeight::NORMAL)),
                Some(&t!("settings.providers.command_registry")),
                cx,
            ));
        }
        col = col.child(Banner::new(BannerKind::Info, t!("settings.providers.agent_notice")));
        if let Some(error) = error {
            col = col.child(Self::note_line(error, Tone::Bad, cx));
        }
        col.child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .child(Button::new("agent-back", t!("settings.providers.back")).ghost().on_click(cx.listener(|this, _, window, cx| {
                    if let Some(flow) = this.add.as_mut() {
                        Self::enter(flow, Step::Agents, window, cx);
                    }
                    cx.notify();
                })))
                .child(Button::new("agent-add", label).primary().disabled(!can_add).on_click(cx.listener(move |this, _, _, cx| this.add_agent(&key, cx)))),
        )
        .into_any_element()
    }

    /// Add the agent of the agent step. A download runs first when it needs one.
    fn add_agent(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(flow) = self.add.as_ref() else { return };
        let Some(agent) = self.model.read(cx).registry_agent(id).cloned() else { return };
        let name = Some(flow.name.read(cx).value().trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| agent.name.clone());
        let page = cx.entity().downgrade();
        self.model.update(cx, |m, cx| {
            m.add_registry_agent(agent, name, cx, move |_, id, cx| {
                // This can run inside the click on this page: select after it.
                cx.defer(move |cx| {
                    let _ = page.update(cx, |this, cx| {
                        this.add = None;
                        this.pending = Some(id);
                        cx.notify();
                    });
                });
            })
        });
        cx.notify();
    }

    /// An agent from a command: its name, its command line, its variables, Check, and Add.
    fn custom_agent_step(&self, flow: &AddFlow, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let (checking, check) = {
            let m = self.model.read(cx);
            (m.agents.checking, m.agents.check.clone())
        };
        let has_command = split_command(&flow.command.read(cx).value()).is_some();
        let has_name = !flow.name.read(cx).value().trim().is_empty();
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .pt(px(12.))
            .child(ui(t!("settings.providers.custom_agent_intro"), 13., 18., FontWeight::NORMAL, c.graphite))
            .child(pkit::field(&t!("settings.providers.name"), pkit::input_well_named(&flow.name, Some(t!("settings.providers.name").into()), cx), None, cx))
            .child(pkit::field(
                &t!("settings.providers.command"),
                pkit::input_well_named(&flow.command, Some(t!("settings.providers.command").into()), cx).font_family(sayso_ui::fonts::MONO),
                Some(&t!("settings.providers.command_hint")),
                cx,
            ))
            .child(pkit::field(
                &t!("settings.providers.environment"),
                pkit::input_well_named(&flow.env, Some(t!("settings.providers.environment").into()), cx).font_family(sayso_ui::fonts::MONO),
                Some(&t!(crate::shell::os_text!("settings.providers.environment_hint_mac", "settings.providers.environment_hint_windows", "settings.providers.environment_hint_other"))),
                cx,
            ));
        if checking {
            col = col.child(Self::note_line(t!("settings.providers.agent_checking"), Tone::Info, cx));
        } else if let Some(check) = check {
            col = col.child(match check {
                Ok(found) => {
                    let mut parts = vec![format!("{}{}", found.name, found.version.map(|v| format!(" {v}")).unwrap_or_default()), format!("ACP {}", found.protocol_version)];
                    parts.push(if found.signed_in { t!("settings.providers.signed_in") } else { t!("settings.providers.not_signed_in") });
                    if !found.models.is_empty() {
                        parts.push(t!("settings.providers.agent_models", count = found.models.len()));
                    }
                    Self::note_line(t!("settings.providers.agent_answered", details = parts.join(" · ")), Tone::Good, cx)
                }
                Err(error) => Self::note_line(error, Tone::Bad, cx),
            });
        }
        col.child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .child(Button::new("custom-back", t!("settings.providers.back")).ghost().on_click(cx.listener(|this, _, window, cx| {
                    if let Some(flow) = this.add.as_mut() {
                        Self::enter(flow, Step::Agents, window, cx);
                    }
                    cx.notify();
                })))
                .child(Button::new("custom-check", t!("settings.providers.check")).disabled(!has_command || checking).on_click(cx.listener(|this, _, _, cx| this.check_custom(cx))))
                .child(Button::new("custom-add", t!("settings.providers.agent_add")).primary().disabled(!has_command || !has_name).on_click(cx.listener(|this, _, window, cx| this.save_custom(window, cx)))),
        )
        .into_any_element()
    }

    /// The name, the program, its arguments, and the variables of the custom form.
    fn custom_fields(&self, cx: &App) -> Option<CustomAgent> {
        let flow = self.add.as_ref()?;
        let (command, args) = split_command(&flow.command.read(cx).value())?;
        let name = flow.name.read(cx).value().trim().to_string();
        Some((name, command, args, parse_env(&flow.env.read(cx).value())))
    }

    fn check_custom(&mut self, cx: &mut Context<Self>) {
        let Some((_, command, args, env)) = self.custom_fields(cx) else { return };
        self.model.update(cx, |m, cx| m.check_agent_command(command, args, env, cx));
    }

    fn save_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((name, command, args, env)) = self.custom_fields(cx) else { return };
        if name.is_empty() {
            return;
        }
        let id = self.model.update(cx, |m, cx| m.add_custom_agent(name, command, args, env, cx));
        self.after_add(&id, window, cx);
    }
}

/// The fields of the custom agent form: name, program, arguments, variables.
type CustomAgent = (String, String, Vec<String>, std::collections::BTreeMap<String, String>);

/// The color of a note line.
#[derive(Clone, Copy)]
enum Tone {
    Good,
    Bad,
    Info,
}
