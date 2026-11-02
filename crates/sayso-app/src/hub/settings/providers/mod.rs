//! Settings › Providers. See the Paper page "Providers · Redesign".
//!
//! With no provider yet, the page is one centered column that suggests where
//! to start. After that it has two panes: the providers on the left, in
//! groups, and the settings of the selected one on the right (`detail`). A
//! new provider comes from the gallery (`add`). The sign-in of a provider runs
//! in its pane: Sayso shows the page and takes the code when the tool asks.

mod add;
mod detail;
mod logo;

use super::kit;
use crate::hub::pages::ext::provider_detail;
use crate::hub::pages::kit::{self as pkit, plain, ui};
use crate::cleanup_models::CleanupModelState;
use crate::model::{AppModel, ProviderLogin, has_login};
use crate::model_picker;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::cleanup::{self, CleanupModelInfo};
use sayso_core::config::{Provider, ProviderKind, thinking_label};
use sayso_core::languages;
use sayso_core::style::{RunPlan, builtin_styles};
use sayso_enhance::AuthStatus;
use sayso_i18n::t;
use sayso_ui::a11y::{A11yExt as _, LiveStatus, Pressable, Role};
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::{ActivePaper, text};
use std::collections::HashMap;

#[derive(Clone)]
enum Test {
    Running,
    Passed(u64),
    Failed(String),
}

/// The selected provider, with its text fields.
struct Open {
    id: String,
    /// The folder of the login.
    folder: Entity<InputState>,
    /// The code from the sign-in page.
    code: Entity<InputState>,
    /// A field that the user edits in place: the address or the API key.
    edit: Option<(Edit, Entity<InputState>)>,
    _subs: [Subscription; 2],
}

/// What the field of [`Open::edit`] changes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit {
    Address,
    ApiKey,
    /// The command line of an ACP agent.
    Command,
}

/// What is wrong with a provider: a sentence for the user, and the words of
/// the tool or the server.
struct Problem {
    message: String,
    details: Option<String>,
}

/// The status line of a provider.
struct Status {
    dot: Hsla,
    line: String,
    color: Hsla,
    problem: Option<Problem>,
}

/// The groups of the provider list, in their order on the page.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Group {
    Subscriptions,
    OnThisMac,
    Servers,
    Agents,
}

impl Group {
    fn of(p: &Provider) -> Group {
        match p.kind {
            ProviderKind::ClaudeCli { .. } | ProviderKind::ChatGpt { .. } => Group::Subscriptions,
            ProviderKind::CleanupModel { .. } | ProviderKind::AppleIntelligence => Group::OnThisMac,
            ProviderKind::OpenAiCompatible { .. } => Group::Servers,
            ProviderKind::Acp { .. } => Group::Agents,
        }
    }

    fn title(self) -> String {
        match self {
            Group::Subscriptions => t!("settings.providers.group_subscriptions"),
            Group::OnThisMac => t!("settings.providers.group_on_this_computer", computer = crate::shell::computer()),
            Group::Servers => t!("settings.providers.group_servers"),
            Group::Agents => t!("settings.providers.group_agents"),
        }
    }
}

/// The providers in their groups, in the order of the config file. Apple
/// Intelligence comes last in its group: the larger models clean up better.
fn grouped(providers: &[Provider]) -> Vec<(Group, Vec<Provider>)> {
    let mut groups: Vec<(Group, Vec<Provider>)> = Vec::new();
    for p in providers {
        let group = Group::of(p);
        match groups.iter_mut().find(|(g, _)| *g == group) {
            Some((_, list)) => list.push(p.clone()),
            None => groups.push((group, vec![p.clone()])),
        }
    }
    groups.sort_by_key(|(g, _)| *g);
    for (_, list) in &mut groups {
        list.sort_by_key(|p| matches!(p.kind, ProviderKind::AppleIntelligence));
    }
    groups
}

/// The provider that shows when none is selected: the default, else the first in the list.
fn first_choice(providers: &[Provider], default: Option<&str>) -> Option<String> {
    default
        .filter(|d| providers.iter().any(|p| p.id == *d))
        .map(str::to_string)
        .or_else(|| grouped(providers).first().and_then(|(_, list)| list.first()).map(|p| p.id.clone()))
}

pub struct ProviderSettings {
    model: Entity<AppModel>,
    open: Option<Open>,
    confirm_remove: Option<String>,
    add: Option<add::AddFlow>,
    /// A provider that was added outside a render: it is selected at the next one.
    pending: Option<String>,
    tests: HashMap<String, Test>,
    /// The locale of the text in the placeholders of the input fields.
    locale: String,
    _observe: Subscription,
}

/// The name of the tool in a sentence, and the folder it uses by default.
/// ChatGPT has no folder: Sayso keeps its sign-in.
fn tool(p: &Provider) -> (&'static str, &'static str) {
    match p.kind {
        ProviderKind::ChatGpt { .. } => ("ChatGPT", ""),
        _ => ("Claude Code", "~/.claude"),
    }
}

/// The catalog entry of a cleanup model provider. None for another kind, and
/// for a model that this version does not know.
fn cleanup_info(p: &Provider) -> Option<&'static CleanupModelInfo> {
    match &p.kind {
        ProviderKind::CleanupModel { model } => cleanup::find(model),
        _ => None,
    }
}

/// "English", or "7 languages".
fn languages_short(info: &CleanupModelInfo) -> String {
    match info.languages {
        [one] => languages::display(one),
        many => t!("core.models.languages", count = many.len()),
    }
}

/// "A", "A and B", "A, B, and C", with the words of the interface language.
fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => t!("settings.providers.list_two", a = a, b = b),
        [head @ .., last] => t!("settings.providers.list_last", head = head.join(&t!("settings.providers.list_separator")), last = last),
    }
}

/// "English only", or "Best in English. Also German and French."
fn cleanup_language(info: &CleanupModelInfo) -> String {
    let names: Vec<String> = info.languages.iter().map(|code| languages::display(code)).collect();
    match names.as_slice() {
        [] => String::new(),
        [only] => t!("settings.providers.language_only", language = only),
        [first, others @ ..] => t!("settings.providers.language_best", first = first, others = list(others)),
    }
}

/// The names of the built-in styles that a cleanup model runs, and of those
/// that need a provider that reads prompts (with "your own styles" at the end).
fn style_names(p: &Provider) -> (Vec<String>, Vec<String>) {
    let (mut runs, mut needs) = (Vec::new(), Vec::new());
    for style in builtin_styles() {
        match style.run_plan(p) {
            RunPlan::Cleanup(_) => runs.push(style.display_name()),
            RunPlan::CleanupOnly => needs.push(style.display_name()),
            RunPlan::Prompt | RunPlan::Nothing => {}
        }
    }
    needs.push(t!("settings.providers.own_styles"));
    (runs, needs)
}

impl ProviderSettings {
    pub fn new(model: Entity<AppModel>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let ids: Vec<String> = model.read(cx).config.ai.providers.iter().filter(|p| has_login(p)).map(|p| p.id.clone()).collect();
        model.update(cx, |m, cx| {
            for id in &ids {
                m.check_provider_login(id, cx);
            }
        });
        Self { model, open: None, confirm_remove: None, add: None, pending: None, tests: HashMap::new(), locale: sayso_i18n::locale(), _observe: observe }
    }

    fn provider(&self, id: &str, cx: &App) -> Option<Provider> {
        self.model.read(cx).config.ai.providers.iter().find(|p| p.id == id).cloned()
    }

    /// Show the settings of a provider, and read its login again.
    fn open_row(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_remove = None;
        let Some(p) = self.provider(id, cx) else { return };
        let own = if self.sayso_login(&p, cx) { "" } else { p.config_dir().unwrap_or_default() };
        let folder = crate::widgets::input_state(tool(&p).1, own, window, cx);
        let code = crate::widgets::input_state(&t!("settings.providers.code_placeholder"), "", window, cx);
        let folder_sub = cx.subscribe_in(&folder, window, |this, state, ev: &InputEvent, _, cx| {
            if !matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                return;
            }
            let Some(id) = this.open.as_ref().map(|o| o.id.clone()) else { return };
            // The field is hidden for the login that Sayso keeps: do not replace that folder.
            if this.provider(&id, cx).is_none_or(|p| this.sayso_login(&p, cx)) {
                return;
            }
            let value = state.read(cx).value().trim().to_string();
            this.tests.remove(&id);
            this.model.update(cx, |m, cx| m.set_provider_config_dir(&id, Some(value), cx));
        });
        let code_sub = cx.subscribe_in(&code, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.send_code(window, cx);
            }
        });
        self.open = Some(Open { id: id.to_string(), folder, code, edit: None, _subs: [folder_sub, code_sub] });
        cx.notify();
        // A cleanup model has no login and no model list.
        if matches!(p.kind, ProviderKind::CleanupModel { .. }) {
            return;
        }
        let id = id.to_string();
        // After the render of this page: these update the model.
        let model = self.model.clone();
        cx.spawn(async move |_, cx| {
            model.update(cx, |m, cx| {
                m.check_provider_login(&id, cx);
                // The list has the thinking levels of the model.
                m.load_provider_models(&id, false, cx);
            });
        })
        .detach();
    }

    /// True when the login of `p` is in the folder that Sayso keeps for it.
    fn sayso_login(&self, p: &Provider, cx: &App) -> bool {
        let dir = self.model.read(cx).sayso_login_dir(&p.id);
        p.config_dir().is_some_and(|d| std::path::Path::new(d) == dir)
    }

    fn send_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = &self.open else { return };
        let code = open.code.read(cx).value().trim().to_string();
        if code.is_empty() {
            return;
        }
        let id = open.id.clone();
        open.code.update(cx, |s, cx| s.set_value("", window, cx));
        self.model.update(cx, |m, cx| m.send_sign_in_code(&id, &code, cx));
    }

    /// Use the login of the terminal (false) or a login for Sayso only (true).
    fn set_sayso_login(&mut self, id: &str, sayso: bool, window: &mut Window, cx: &mut Context<Self>) {
        let dir = sayso.then(|| self.model.read(cx).sayso_login_dir(id).display().to_string());
        self.tests.remove(id);
        self.model.update(cx, |m, cx| m.set_provider_config_dir(id, dir, cx));
        // The folder field starts again from the new value.
        self.open_row(id, window, cx);
        cx.notify();
    }

    fn test(&mut self, id: String, cx: &mut Context<Self>) {
        // A cleanup model has no prompt: its test cleans up a sample.
        if let Some(p) = self.provider(&id, cx).filter(|p| matches!(p.kind, ProviderKind::CleanupModel { .. })) {
            let Some(info) = cleanup_info(&p) else {
                self.tests.insert(id, Test::Failed("unknown cleanup model".into()));
                cx.notify();
                return;
            };
            self.tests.insert(id.clone(), Test::Running);
            let entity = cx.entity();
            self.model.update(cx, |m, cx| {
                m.test_cleanup_model(info.id, cx, move |_, result, cx| {
                    entity.update(cx, |this, cx| {
                        this.tests.insert(id, match result {
                            Ok(ms) => Test::Passed(ms),
                            Err(e) => Test::Failed(e),
                        });
                        cx.notify();
                    });
                })
            });
            cx.notify();
            return;
        }
        self.tests.insert(id.clone(), Test::Running);
        let entity = cx.entity();
        self.model.update(cx, |m, cx| {
            m.test_provider(&id.clone(), cx, move |m, result, cx| {
                // A test that fails can mean that the login ended.
                m.check_provider_login(&id, cx);
                entity.update(cx, |this, cx| {
                    this.tests.insert(id, match result {
                        Ok(ms) => Test::Passed(ms),
                        Err(e) => Test::Failed(e),
                    });
                    cx.notify();
                });
            })
        });
        cx.notify();
    }

    fn status(&self, p: &Provider, login: Option<&ProviderLogin>, cx: &App) -> Status {
        let c = cx.paper().colors;
        let test = self.tests.get(&p.id);
        let (name, _) = tool(p);
        let bad = |line: String, problem: Option<Problem>| Status { dot: c.danger, line, color: c.danger, problem };
        if let Some(info) = cleanup_info(p) {
            return self.cleanup_status(info, test, cx);
        }
        if login.is_some_and(|l| l.sign_in.is_some()) {
            return Status { dot: c.accent, line: t!("settings.providers.waiting_for_browser"), color: c.graphite, problem: None };
        }
        if matches!(test, Some(Test::Running)) {
            return Status { dot: c.accent, line: t!("settings.providers.testing"), color: c.graphite, problem: None };
        }
        if p.needs_model() {
            return bad(t!("settings.providers.choose_model_to_use"), None);
        }
        let mut signed_in = None;
        if has_login(p) {
            match login.and_then(|l| l.status.as_ref()) {
                None => return Status { dot: c.pencil, line: t!("settings.providers.checking_login"), color: c.graphite, problem: None },
                Some(AuthStatus::SignedIn { account }) => {
                    signed_in = Some(match account {
                        Some(account) => t!("settings.providers.signed_in_account", account = account),
                        None => t!("settings.providers.signed_in"),
                    });
                }
                Some(AuthStatus::SignedOut { detail }) => {
                    let message = if matches!(p.kind, ProviderKind::Acp { .. }) {
                        t!("settings.providers.signed_out_agent", name = p.name)
                    } else if matches!(p.kind, ProviderKind::ChatGpt { .. }) {
                        t!("settings.providers.signed_out_chatgpt")
                    } else if self.sayso_login(p, cx) {
                        t!("settings.providers.signed_out_sayso", tool = name)
                    } else {
                        t!("settings.providers.signed_out_folder", tool = name)
                    };
                    return bad(t!("settings.providers.not_signed_in"), Some(Problem { message, details: Some(detail.clone()) }));
                }
                Some(AuthStatus::NotInstalled) => {
                    return bad(
                        t!("settings.providers.not_installed"),
                        Some(Problem {
                            message: if matches!(p.kind, ProviderKind::Acp { .. }) {
                                t!("settings.providers.agent_missing", name = p.name)
                            } else {
                                t!("settings.providers.not_installed_detail", tool = name)
                            },
                            details: None,
                        }),
                    );
                }
                Some(AuthStatus::Unknown { detail }) => {
                    return bad(
                        t!("settings.providers.login_unknown"),
                        Some(Problem {
                            message: if matches!(p.kind, ProviderKind::Acp { .. }) {
                                t!("settings.providers.agent_unknown", name = p.name)
                            } else {
                                t!("settings.providers.login_unknown_detail", tool = name)
                            },
                            details: Some(detail.clone()),
                        }),
                    );
                }
            }
        }
        match test {
            Some(Test::Failed(raw)) => bad(t!("settings.providers.test_failed"), Some(Problem { message: test_error(raw), details: Some(raw.clone()) })),
            Some(Test::Passed(ms)) => Status { dot: c.success, line: t!("settings.providers.answered_in", ms = ms), color: c.success, problem: None },
            _ => match signed_in {
                Some(line) => Status { dot: c.success, line, color: c.graphite, problem: None },
                None => Status {
                    dot: if p.is_cloud() { c.pencil } else { c.success },
                    line: provider_detail(p),
                    color: c.graphite,
                    problem: None,
                },
            },
        }
    }

    /// The status line of a cleanup model: the download, then the last test.
    fn cleanup_status(&self, info: &CleanupModelInfo, test: Option<&Test>, cx: &App) -> Status {
        let c = cx.paper().colors;
        let bad = |line: String, problem: Problem| Status { dot: c.danger, line, color: c.danger, problem: Some(problem) };
        if matches!(test, Some(Test::Running)) {
            return Status { dot: c.accent, line: t!("settings.providers.testing"), color: c.graphite, problem: None };
        }
        match self.model.read(cx).cleanup_model_state(info.id) {
            CleanupModelState::Downloading(fraction) => Status {
                dot: c.accent,
                line: t!("settings.providers.cleanup_downloading", percent = (fraction * 100.).round() as u32),
                color: c.graphite,
                problem: None,
            },
            CleanupModelState::Failed(raw) => bad(
                t!("settings.providers.cleanup_download_failed"),
                Problem { message: t!("settings.providers.cleanup_download_failed_detail", name = info.name), details: Some(raw) },
            ),
            CleanupModelState::NotDownloaded => Status { dot: c.pencil, line: t!("settings.providers.cleanup_not_downloaded"), color: c.graphite, problem: None },
            CleanupModelState::Deleting => Status { dot: c.pencil, line: t!("settings.providers.cleanup_deleting"), color: c.graphite, problem: None },
            CleanupModelState::DeleteFailed(raw) => bad(
                t!("settings.providers.cleanup_delete_failed"),
                Problem { message: t!("settings.providers.cleanup_delete_failed_detail", name = info.name), details: Some(raw) },
            ),
            CleanupModelState::Ready => match test {
                Some(Test::Failed(raw)) => {
                    // The cleanup runtime of this package needs instructions that the processor lacks: a new
                    // download of the model does not help.
                    let message = if sayso_cleanup_client::runtime::is_unsupported_cpu(raw) {
                        t!("settings.providers.cleanup_test_failed_cpu_detail")
                    } else {
                        t!("settings.providers.cleanup_test_failed_detail")
                    };
                    bad(t!("settings.providers.test_failed"), Problem { message, details: Some(raw.clone()) })
                }
                Some(Test::Passed(ms)) => Status { dot: c.success, line: t!("settings.providers.answered_in", ms = ms), color: c.success, problem: None },
                _ => Status {
                    dot: c.success,
                    line: t!("settings.providers.cleanup_ready", computer = crate::shell::computer(), languages = languages_short(info)),
                    color: c.graphite,
                    problem: None,
                },
            },
        }
    }

    /// The mark of a provider. An agent of the registry without a bundled
    /// mark uses its downloaded icon, which loads the first time.
    fn mark(&self, p: &Provider, cx: &mut Context<Self>) -> logo::Mark {
        let mark = logo::of(p);
        if mark != logo::AGENT {
            return mark;
        }
        let ProviderKind::Acp { registry: Some(entry), .. } = &p.kind else { return mark };
        let id = entry.id.clone();
        self.model
            .update(cx, |m, cx| {
                let agent = m.registry_agent(&id).cloned()?;
                m.agent_icon(&agent, cx)
            })
            .map(|path| logo::Mark::External(path.to_string_lossy().into_owned().into()))
            .unwrap_or(mark)
    }

    /// The model and thinking level of a provider, for its row: "Haiku 5.5 · Low".
    fn summary(&self, p: &Provider, cx: &App) -> String {
        let m = self.model.read(cx);
        if let Some(info) = cleanup_info(p) {
            return format!("{} · {}", t!("settings.providers.no_prompts"), languages_short(info));
        }
        let level = p.thinking_sent().filter(|sent| m.thinking_choices(p).iter().any(|level| level == sent)).map(thinking_label);
        let label = match p.model() {
            Some(id) => m.model_label(&p.id, id),
            None if p.needs_model() => t!("settings.providers.choose_model"),
            None if p.kind == ProviderKind::AppleIntelligence => match m.model_lists.get(&p.id) {
                Some(crate::model::ModelList::Ready(list)) if !list.is_empty() => list[0].name.clone(),
                _ => t!("settings.providers.default_model"),
            },
            None => t!("settings.providers.default_model"),
        };
        match level {
            Some(level) => format!("{label} · {level}"),
            None => label,
        }
    }

    /// One provider in the list: its mark, its name, its model, and a dot.
    /// A problem or a sign-in replaces the model line.
    fn list_row(&self, p: &Provider, selected: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let login = self.model.read(cx).logins.get(&p.id).cloned();
        let status = self.status(p, login.as_ref(), cx);
        // The status line of a problem or of a running task, else the model.
        let line = if status.color == c.danger || status.color == c.accent || status.dot == c.accent {
            (status.line.clone(), status.color)
        } else {
            (self.summary(p, cx), c.graphite)
        };
        let name = cleanup_info(p).map_or_else(|| p.name.clone(), |i| i.name.to_string());
        let id = p.id.clone();
        let mut row = Pressable::new(SharedString::from(format!("provider-{}", p.id)))
            .label(format!("{name}, {}", line.0))
            .role(Role::Tab)
            .aria_selected(selected)
            .flex()
            .items_center()
            .w_full()
            .gap(px(11.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(10.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| this.open_row(&id, window, cx)))
            .child(logo::tile(self.mark(p, cx), 30., cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.))
                    .child(plain(name, 14., 18., FontWeight::SEMIBOLD, c.ink).truncate())
                    .child(
                        LiveStatus::new(SharedString::from(format!("status-{}", p.id)), line.0.clone())
                            .min_w_0()
                            .child(plain(line.0, 12., 15., FontWeight::NORMAL, line.1).truncate()),
                    ),
            )
            .child(div().flex_none().size(px(8.)).rounded_full().bg(status.dot));
        if selected {
            row = row.bg(c.sheet_raised).shadow(sayso_ui::paper::raised_small(&c));
        }
        row.into_any_element()
    }

    /// The list on the left: the groups, and Add a provider at the bottom.
    fn list_pane(&self, providers: &[Provider], selected: Option<&str>, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let mut column = div().id("provider-list").role(Role::TabList).aria_label(t!("settings.providers.your_providers")).flex().flex_col().gap(px(2.));
        for (group, list) in grouped(providers) {
            column = column.child(div().px(px(10.)).pt(px(14.)).pb(px(6.)).child(pkit::caps(group.title(), 11., c.pencil)));
            for p in &list {
                column = column.child(self.list_row(p, selected == Some(p.id.as_str()), cx));
            }
        }
        let add = pkit::dashed("add-provider", cx)
            .label(t!("settings.providers.add_a_provider"))
            .aria_description(t!("settings.providers.add_a_provider_detail"))
            .justify_start()
            .gap(px(11.))
            .p(px(10.))
            .rounded(px(10.))
            .on_click(cx.listener(|this, _, window, cx| this.open_add(None, window, cx)))
            .child(div().flex().flex_none().items_center().justify_center().size(px(30.)).rounded(px(8.)).bg(c.sheet_raised).shadow(sayso_ui::paper::raised_small(&c)).child(icon(Icon::Plus, 14., c.ink)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap(px(1.))
                    .child(plain(t!("settings.providers.add_a_provider"), 14., 18., FontWeight::SEMIBOLD, c.ink))
                    .child(plain(t!("settings.providers.add_a_provider_short"), 12., 15., FontWeight::NORMAL, c.graphite)),
            );
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(264.))
            .h_full()
            .pr(px(16.))
            .border_r_1()
            .border_color(c.rule)
            .child(div().id("provider-list-scroll").flex_1().min_h_0().overflow_y_scroll().child(column))
            .child(div().flex_none().pt(px(12.)).pb(px(16.)).child(add))
            .into_any_element()
    }

    /// The title, the text under it, and the switch for AI.
    fn header(&self, enabled: bool, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let model = self.model.clone();
        let switch = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(px(2.))
                    .child(plain(t!("settings.providers.use_ai"), 13., 16., FontWeight::SEMIBOLD, c.ink))
                    .child(plain(t!("settings.providers.use_ai_short"), 12., 15., FontWeight::NORMAL, c.graphite)),
            )
            .child(Switch::new("ai-on", enabled).label(t!("settings.providers.use_ai")).on_toggle(move |on, _, cx| model.update(cx, |m, cx| m.set_ai_enabled(on, cx))));
        crate::widgets::page_header(&t!("settings.providers.title"), Some(&t!("settings.providers.subtitle")), Some(switch.into_any_element()), cx)
    }
}

/// A provider test error in plain words, with what to do next.
fn test_error(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("not signed in") {
        t!("settings.providers.error_not_signed_in")
    } else if lower.contains("refused") || lower.contains("connect") || lower.contains("dns") {
        t!("settings.providers.error_cannot_connect")
    } else if lower.contains("timed out") || lower.contains("timeout") {
        t!("settings.providers.error_timeout")
    } else if lower.contains("401") || lower.contains("403") || lower.contains("unauthorized") || lower.contains("api key") {
        t!("settings.providers.error_refused_key")
    } else if lower.contains("404") || lower.contains("model") {
        t!("settings.providers.error_model")
    } else {
        t!("settings.providers.error_no_answer")
    }
}

impl Render for ProviderSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The input fields keep their placeholders, so they change with the language.
        if self.locale != sayso_i18n::locale() {
            self.locale = sayso_i18n::locale();
            if let Some(open) = &self.open {
                open.code.update(cx, |s, cx| s.set_placeholder(t!("settings.providers.code_placeholder"), window, cx));
            }
            if let Some(add) = &self.add {
                add.relabel(window, cx);
            }
        }
        let (providers, default, enabled) = {
            let m = self.model.read(cx);
            (m.config.ai.providers.clone(), m.config.ai.default_provider.clone(), m.config.ai.enabled)
        };
        let modal = self.add.as_ref().map(|_| self.add_modal(window, cx));

        // No provider yet: one centered column until the user picks one.
        if providers.is_empty() {
            self.open = None;
            return div().relative().size_full().child(self.empty_state(cx)).children(modal).into_any_element();
        }

        if let Some(id) = self.pending.take() {
            self.open_row(&id, window, cx);
        }
        // A provider that is gone shows the default instead.
        let current = self.open.as_ref().map(|o| o.id.clone()).filter(|id| providers.iter().any(|p| &p.id == id));
        let selected = match current {
            Some(id) => id,
            None => {
                let first = first_choice(&providers, default.as_deref()).unwrap_or_default();
                self.open_row(&first, window, cx);
                first
            }
        };
        let list = self.list_pane(&providers, Some(&selected), cx);
        let detail = providers.iter().find(|p| p.id == selected).map(|p| {
            let is_default = default.as_deref() == Some(p.id.as_str());
            self.detail(p, is_default, cx)
        });
        let c = cx.paper().colors;
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .id("providers-page")
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .pt(px(40.))
                    .px(px(44.))
                    .child(self.header(enabled, cx).pb(px(22.)).flex_none())
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .border_t_1()
                            .border_color(c.rule)
                            .child(list)
                            .child(div().id("provider-detail").flex_1().min_w_0().h_full().pl(px(36.)).overflow_y_scrollbar().children(detail)),
                    ),
            )
            .children(modal)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // No glob import: `gpui_kit::*` has its own `test` attribute.
    use super::{Group, cleanup, cleanup_language, first_choice, grouped, languages_short, list, style_names};
    use sayso_core::config::{Provider, ProviderKind};

    fn cleanup_provider(id: &str) -> Provider {
        Provider { id: id.into(), name: id.into(), kind: ProviderKind::CleanupModel { model: id.into() } }
    }

    #[test]
    fn a_model_lists_the_styles_it_runs_and_the_styles_it_cannot() {
        let (runs, needs) = style_names(&cleanup_provider("s1-mini"));
        assert_eq!(list(&runs), "Clean, Message, Email, and Notes");
        assert_eq!(list(&needs), "Polished, Rambling, and your own styles");
        let (runs, needs) = style_names(&cleanup_provider("spokenly-cleanup-1"));
        assert_eq!(list(&runs), "Clean and Message");
        assert_eq!(list(&needs), "Polished, Rambling, Email, Notes, and your own styles");
    }

    #[test]
    fn the_language_line_names_the_other_languages() {
        let s1 = cleanup::find("s1-mini").unwrap();
        let one = cleanup::find("spokenly-cleanup-1").unwrap();
        assert_eq!(cleanup_language(s1), "English only");
        assert_eq!(cleanup_language(one), "Best in English. Also German, Spanish, French, Chinese, Russian, and Japanese.");
        assert_eq!(languages_short(s1), "English");
        assert_eq!(languages_short(one), "7 languages");
    }

    fn kind(id: &str, kind: ProviderKind) -> Provider {
        Provider { id: id.into(), name: id.into(), kind }
    }

    #[test]
    fn the_list_groups_providers_and_puts_apple_intelligence_last() {
        let providers = vec![
            kind("apple", ProviderKind::AppleIntelligence),
            kind("router", ProviderKind::OpenAiCompatible { base_url: "https://openrouter.ai/api/v1".into(), model: "m".into(), api_key_account: None, zero_data_retention: false, thinking: None }),
            cleanup_provider("s1-mini"),
            kind("chatgpt", ProviderKind::ChatGpt { model: None, thinking: None }),
        ];
        let groups = grouped(&providers);
        let ids: Vec<(Group, Vec<&str>)> = groups.iter().map(|(g, list)| (*g, list.iter().map(|p| p.id.as_str()).collect())).collect();
        assert_eq!(ids, [(Group::Subscriptions, vec!["chatgpt"]), (Group::OnThisMac, vec!["s1-mini", "apple"]), (Group::Servers, vec!["router"])]);
        assert_eq!(first_choice(&providers, Some("router")).as_deref(), Some("router"));
        assert_eq!(first_choice(&providers, Some("gone")).as_deref(), Some("chatgpt"), "the first of the list, not Apple Intelligence");
        assert_eq!(first_choice(&[], None), None);
    }
}
