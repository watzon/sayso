//! Settings › Providers. See the Paper page "Providers".
//!
//! One row for each AI provider. A row opens in place and shows the login,
//! the folder of the login, the model, and the whole text of a problem. The
//! sign-in of a CLI provider runs here: the tool does the sign-in, and Sayso
//! shows the page and takes the code.

use super::kit::{self, group};
use crate::hub::pages::ext::provider_detail;
use crate::hub::pages::kit::{self as pkit, plain, ui};
use crate::cleanup_models::CleanupModelState;
use crate::model::{AppModel, ProviderLogin};
use crate::model_picker;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::cleanup::{self, CleanupModelInfo};
use sayso_core::config::{Provider, ProviderKind, is_local_url, thinking_label};
use sayso_core::languages;
use sayso_core::models::format_size;
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

#[derive(Clone, Copy, PartialEq)]
enum AddKind {
    OpenAi,
    Claude,
    Codex,
}

struct AddProvider {
    kind: AddKind,
    name: Entity<InputState>,
    url: Entity<InputState>,
    key: Entity<InputState>,
    error: Option<AddError>,
    _sub: Subscription,
}

/// The row that is open, with its text fields.
struct Open {
    id: String,
    /// The folder of the login.
    folder: Entity<InputState>,
    /// The code from the sign-in page.
    code: Entity<InputState>,
    _subs: [Subscription; 2],
}

/// What is wrong with a provider: a sentence for the user, and the words of
/// the tool or the server.
struct Problem {
    message: String,
    details: Option<String>,
}

/// The status line of a row.
struct Status {
    dot: Hsla,
    line: String,
    color: Hsla,
    problem: Option<Problem>,
}

/// (name, base URL). The model is chosen from the provider's list after it is added.
const PRESETS: [(&str, &str); 4] = [
    ("OpenRouter", "https://openrouter.ai/api/v1"),
    ("OpenAI", "https://api.openai.com/v1"),
    ("Ollama", "http://localhost:11434/v1"),
    ("LM Studio", "http://localhost:1234/v1"),
];

/// Why a new provider was not saved. The panel keeps this, not the text, so
/// the message changes with the interface language.
#[derive(Clone, Copy)]
enum AddError {
    NoName,
    BadUrl,
    NoClaudeCli,
    NoCodexCli,
}

impl AddError {
    fn text(self) -> String {
        match self {
            AddError::NoName => t!("settings.providers.error_no_name"),
            AddError::BadUrl => t!("settings.providers.error_bad_url"),
            AddError::NoClaudeCli => t!("settings.providers.error_no_claude_cli"),
            AddError::NoCodexCli => t!("settings.providers.error_no_codex_cli"),
        }
    }
}

pub struct ProviderSettings {
    model: Entity<AppModel>,
    open: Option<Open>,
    confirm_remove: Option<String>,
    add: Option<AddProvider>,
    tests: HashMap<String, Test>,
    /// The locale of the text in the placeholders of the input fields.
    locale: String,
    _observe: Subscription,
}

fn is_cli(p: &Provider) -> bool {
    matches!(p.kind, ProviderKind::ClaudeCli { .. } | ProviderKind::CodexCli { .. })
}

/// The name of the tool in a sentence, and the folder it uses by default.
fn tool(p: &Provider) -> (&'static str, &'static str) {
    match p.kind {
        ProviderKind::CodexCli { .. } => ("Codex", "~/.codex"),
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
        let ids: Vec<String> = model.read(cx).config.ai.providers.iter().filter(|p| is_cli(p)).map(|p| p.id.clone()).collect();
        model.update(cx, |m, cx| {
            for id in &ids {
                m.check_provider_login(id, cx);
            }
        });
        Self { model, open: None, confirm_remove: None, add: None, tests: HashMap::new(), locale: sayso_i18n::locale(), _observe: observe }
    }

    fn provider(&self, id: &str, cx: &App) -> Option<Provider> {
        self.model.read(cx).config.ai.providers.iter().find(|p| p.id == id).cloned()
    }

    /// True when the login of `p` is in the folder that Sayso keeps for it.
    fn sayso_login(&self, p: &Provider, cx: &App) -> bool {
        let dir = self.model.read(cx).sayso_login_dir(&p.id);
        p.config_dir().is_some_and(|d| std::path::Path::new(d) == dir)
    }

    // -----------------------------------------------------------------------
    // Rows
    // -----------------------------------------------------------------------

    fn toggle(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_remove = None;
        if self.open.as_ref().is_some_and(|o| o.id == id) {
            self.open = None;
        } else {
            self.open_row(id, window, cx);
        }
        cx.notify();
    }

    /// Open the row of a provider, and read its login again.
    fn open_row(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
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
        self.open = Some(Open { id: id.to_string(), folder, code, _subs: [folder_sub, code_sub] });
        // A cleanup model has no login and no model list.
        if matches!(p.kind, ProviderKind::CleanupModel { .. }) {
            return;
        }
        self.model.update(cx, |m, cx| {
            m.check_provider_login(id, cx);
            // The list has the thinking levels of the model.
            m.load_provider_models(id, false, cx);
        });
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
        if is_cli(p) {
            match login.and_then(|l| l.status.as_ref()) {
                None => return Status { dot: c.pencil, line: t!("settings.providers.checking_login"), color: c.graphite, problem: None },
                Some(AuthStatus::SignedIn { account }) => {
                    signed_in = Some(match account {
                        Some(account) => t!("settings.providers.signed_in_account", account = account),
                        None => t!("settings.providers.signed_in"),
                    });
                }
                Some(AuthStatus::SignedOut { detail }) => {
                    let message = if self.sayso_login(p, cx) {
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
                            message: t!("settings.providers.not_installed_detail", tool = name),
                            details: None,
                        }),
                    );
                }
                Some(AuthStatus::Unknown { detail }) => {
                    return bad(
                        t!("settings.providers.login_unknown"),
                        Some(Problem { message: t!("settings.providers.login_unknown_detail", tool = name), details: Some(detail.clone()) }),
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

    fn provider_row(&self, p: &Provider, default: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let login = self.model.read(cx).logins.get(&p.id).cloned();
        let status = self.status(p, login.as_ref(), cx);
        let open = self.open.as_ref().filter(|o| o.id == p.id);
        let signing_in = login.as_ref().is_some_and(|l| l.sign_in.is_some());
        let signed_out = matches!(login.as_ref().and_then(|l| l.status.as_ref()), Some(AuthStatus::SignedOut { .. }));

        let toggle_id = p.id.clone();
        // The row is one button that opens the detail. The status line is a
        // live node inside it, so a change (a test, a sign-in) is read.
        let cleanup = cleanup_info(p);
        // The license of S1-mini asks for its exact name with its publisher.
        let shown_name = cleanup.map_or_else(|| p.name.clone(), |i| i.name.to_string());
        let by = cleanup.map(|i| t!("settings.providers.by", publisher = i.publisher));
        let mut name = shown_name.clone();
        if let Some(by) = &by {
            name = format!("{name} {by}, {}", t!("settings.providers.no_prompts"));
        }
        if default {
            name = format!("{name}, {}", t!("settings.providers.default_badge"));
        }
        let label = Pressable::new(SharedString::from(format!("provider-{}", p.id)))
            .label(name)
            .aria_expanded(open.is_some())
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(14.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| this.toggle(&toggle_id, window, cx)))
            .child(div().flex_none().size(px(8.)).rounded_full().bg(status.dot))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(plain(shown_name.clone(), 15., 18., FontWeight::SEMIBOLD, c.ink).truncate())
                            .when_some(by.clone(), |d, by| d.child(plain(by, 13., 18., FontWeight::NORMAL, c.graphite).flex_none()))
                            .when(cleanup.is_some(), |d| {
                                d.child(div().flex_none().px(px(6.)).py(px(2.)).rounded(px(5.)).bg(c.accent_wash).child(pkit::caps(t!("settings.providers.no_prompts"), 11., c.accent)))
                            })
                            .when(default, |d| {
                                d.child(div().flex_none().px(px(6.)).py(px(2.)).rounded(px(5.)).bg(c.deboss).child(pkit::caps(t!("settings.providers.default_badge"), 11., c.graphite)))
                            }),
                    )
                    .child(
                        LiveStatus::new(SharedString::from(format!("status-{}", p.id)), status.line.clone())
                            .min_w_0()
                            .child(plain(status.line.clone(), 13., 16., FontWeight::NORMAL, status.color).truncate()),
                    ),
            );

        let mut head = div().flex().items_center().gap(px(14.)).child(label);
        if signing_in {
            let id = p.id.clone();
            head = head.child(
                Button::new(SharedString::from(format!("cancel-sign-in-{}", p.id)), t!("settings.providers.cancel"))
                    .a11y_label(format!("{}, {}", t!("settings.providers.cancel"), p.name))
                    .on_click(cx.listener(move |this, _, _, cx| this.model.update(cx, |m, cx| m.cancel_sign_in(&id, cx)))),
            );
        } else if signed_out {
            let id = p.id.clone();
            head = head.child(Button::new(SharedString::from(format!("sign-in-{}", p.id)), t!("settings.providers.sign_in"))
                .a11y_label(format!("{}, {}", t!("settings.providers.sign_in"), p.name))
                .primary()
                .on_click(cx.listener(
                move |this, _, window, cx| {
                    if this.open.as_ref().is_none_or(|o| o.id != id) {
                        this.open_row(&id, window, cx);
                    }
                    this.model.update(cx, |m, cx| m.sign_in_provider(&id, cx));
                    cx.notify();
                },
            )));
        } else if let Some(info) = cleanup {
            head = head.children(self.cleanup_head_action(p, info, cx));
        } else if open.is_none() {
            // The open row has the picker in its Model line.
            head = head.child(self.model_picker(p, cx));
        }
        let chevron_id = p.id.clone();
        head = head.child(
            // The row button does the same, so the keyboard skips this one.
            pkit::icon_button(SharedString::from(format!("chevron-{}", p.id)), if open.is_some() { Icon::ChevronDown } else { Icon::ChevronRight }, 14., cx)
                .label(format!("{}, {}", t!("settings.providers.details"), p.name))
                .aria_expanded(open.is_some())
                .tab_stop(false)
                .on_click(cx.listener(move |this, _, window, cx| this.toggle(&chevron_id, window, cx))),
        );

        // The id scopes the text of the row: the ids of the nodes below are unique.
        let mut row = div()
            .id(SharedString::from(format!("provider-row-{}", p.id)))
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .py(px(14.))
            .border_b_1()
            .border_color(c.rule)
            .child(head);
        if let Some(CleanupModelState::Downloading(fraction)) = cleanup.map(|i| self.model.read(cx).cleanup_model_state(i.id)) {
            // Under the dot and the gap of the row.
            row = row.child(div().pl(px(22.)).pt(px(8.)).child(Progress::new(fraction).height(4.).label(t!("settings.providers.cleanup_downloading_label", name = shown_name))));
        }
        if let Some(open) = open {
            row = row.child(self.detail(p, default, open, login.as_ref(), status.problem, cx));
        }
        row.into_any_element()
    }

    /// The button at the right of a cleanup model row, when the model needs
    /// one: Cancel while it downloads, Try again after an error, Download
    /// when the files are not there.
    fn cleanup_head_action(&self, p: &Provider, info: &'static CleanupModelInfo, cx: &mut Context<Self>) -> Option<Button> {
        let id = SharedString::from(format!("cleanup-action-{}", p.id));
        let model = info.id;
        let (label, primary) = match self.model.read(cx).cleanup_model_state(model) {
            CleanupModelState::Ready | CleanupModelState::Deleting => return None,
            CleanupModelState::Downloading(_) => (t!("settings.providers.cancel"), false),
            CleanupModelState::Failed(_) | CleanupModelState::DeleteFailed(_) => (t!("settings.providers.try_again"), true),
            CleanupModelState::NotDownloaded => (t!("settings.providers.download"), true),
        };
        let state = self.model.read(cx).cleanup_model_state(model);
        let downloading = matches!(state, CleanupModelState::Downloading(_));
        let delete_failed = matches!(state, CleanupModelState::DeleteFailed(_));
        let button = Button::new(id, label.clone()).a11y_label(format!("{label}, {}", info.name)).on_click(cx.listener(move |this, _, _, cx| {
            this.model.update(cx, |m, cx| {
                if downloading {
                    m.cancel_cleanup_download(model);
                } else if delete_failed {
                    m.delete_cleanup_model(model, cx);
                } else {
                    m.download_cleanup_model(model, cx);
                }
            });
            cx.notify();
        }));
        Some(if primary { button.primary() } else { button.ghost() })
    }

    /// The open part of a row.
    fn detail(&self, p: &Provider, default: bool, open: &Open, login: Option<&ProviderLogin>, problem: Option<Problem>, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let (name, _) = tool(p);
        let mut col = div().flex().flex_col().w_full().min_w_0().pl(px(22.)).pt(px(14.));

        if let Some(sign_in) = login.and_then(|l| l.sign_in.as_ref()) {
            col = col.child(self.sign_in_panel(p, open, sign_in.takes_code(), login.and_then(|l| l.url.clone()), cx));
        } else if let Some(problem) = problem {
            col = col.child(self.problem(&p.id, problem, cx));
        }
        if let Some(error) = login.and_then(|l| l.error.as_ref()) {
            col = col.child(kit::banner(BannerKind::Warning, error.text(), cx));
        }

        if let Some(info) = cleanup_info(p) {
            return col
                .child(self.cleanup_summary(p, cx))
                .child(kit::row_s(t!("settings.providers.language"), cleanup_language(info), div(), cx))
                .child(self.cleanup_file(info, cx))
                .child(kit::row_s(
                    t!("settings.providers.test"),
                    t!("settings.providers.test_detail", style = t!("styles.builtin.clean.name")),
                    self.cleanup_actions(p, info, default, cx),
                    cx,
                ));
        }
        if is_cli(p) {
            let sayso = self.sayso_login(p, cx);
            let entity = cx.entity();
            let id = p.id.clone();
            let choice = Segmented::new(SharedString::from(format!("login-{}", p.id)), [t!("settings.providers.login_mine", tool = name), t!("settings.providers.login_sayso")], usize::from(sayso))
                .label(t!("settings.providers.login"))
                .on_select(move |i, window, cx| entity.update(cx, |this, cx| this.set_sayso_login(&id, i == 1, window, cx)));
            let about = if sayso {
                t!("settings.providers.login_sayso_detail", tool = name)
            } else {
                t!("settings.providers.login_mine_detail", tool = name)
            };
            col = col.child(kit::row_s(t!("settings.providers.login"), about, choice, cx));
            if !sayso {
                let field = div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .w(px(320.))
                    .h(px(36.))
                    .px(px(12.))
                    .rounded(px(9.))
                    .debossed(&c)
                    .font_family(sayso_ui::fonts::MONO)
                    .text_size(px(12.))
                    .child(gpui_kit::component::input::Input::new(&open.folder).aria_label(t!("settings.providers.config_folder")).appearance(false).w_full());
                col = col.child(kit::row(
                    &t!("settings.providers.config_folder"),
                    &t!("settings.providers.config_folder_detail"),
                    field,
                    cx,
                ));
            }
        } else if let ProviderKind::OpenAiCompatible { base_url, .. } = &p.kind {
            let place = if is_local_url(base_url) { t!("settings.providers.address_local", computer = crate::shell::computer()) } else { t!("settings.providers.address_cloud") };
            col = col.child(kit::row_s(t!("settings.providers.address"), place, text::mono(base_url.clone(), 12., c.graphite).font_weight(FontWeight::NORMAL), cx));
        }

        // macOS and the catalog choose the model of these two kinds.
        if !matches!(p.kind, ProviderKind::AppleIntelligence | ProviderKind::CleanupModel { .. }) {
            col = col.child(kit::row(&t!("settings.providers.model"), &t!("settings.providers.model_detail"), self.model_picker(p, cx), cx));
        }
        let path = match &p.kind {
            ProviderKind::ClaudeCli { path, .. } | ProviderKind::CodexCli { path, .. } => path.clone(),
            _ => None,
        };
        let mut left = div().flex().flex_col().flex_1().min_w_0().gap(px(2.));
        if let Some(path) = path {
            left = left
                .child(ui(t!("settings.providers.program"), 15., 18., FontWeight::SEMIBOLD, c.ink))
                .child(text::mono(path, 12., c.graphite).font_weight(FontWeight::NORMAL));
        }
        col.child(div().flex().items_center().w_full().gap(px(24.)).pt(px(14.)).child(left).child(self.actions(p, default, cx)))
    }

    /// What a cleanup model runs, from the plan of each built-in style.
    fn cleanup_summary(&self, p: &Provider, cx: &App) -> Div {
        let c = cx.paper().colors;
        let (runs, needs) = style_names(p);
        div().w_full().min_w_0().pb(px(6.)).child(
            div()
                .id(SharedString::from(format!("cleanup-runs-{}", p.id)))
                .role(Role::Group)
                .aria_label(t!("settings.providers.cleanup_runs", styles = list(&runs)))
                .flex()
                .flex_col()
                .w_full()
                .min_w_0()
                .gap(px(3.))
                .px(px(16.))
                .py(px(14.))
                .rounded(px(10.))
                .bg(c.accent_wash)
                .border_1()
                .border_color(c.accent.opacity(0.18))
                .child(plain(t!("settings.providers.cleanup_runs", styles = list(&runs)), 14., 18., FontWeight::SEMIBOLD, c.ink))
                .child(ui(t!("settings.providers.cleanup_needs", styles = list(&needs)), 13., 18., FontWeight::NORMAL, c.graphite)),
        )
    }

    /// The model file: its size, its name, and a button that shows it.
    fn cleanup_file(&self, info: &'static CleanupModelInfo, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let path = self.model.read(cx).cleanup_model_file(info.id);
        let reveal = path.map(|path| {
            let label = t!(crate::shell::os_text!("settings.providers.show_in_finder", "settings.providers.show_in_explorer", "settings.providers.show_in_folder"));
            Button::new(SharedString::from(format!("reveal-{}", info.id)), label.clone())
                .a11y_label(format!("{label}, {}", info.name))
                .small()
                .on_click(move |_, _, _| crate::shell::reveal(&path))
        });
        let control = div().flex().flex_none().items_center().gap(px(12.)).child(text::mono(info.file, 12., c.graphite).font_weight(FontWeight::NORMAL)).children(reveal);
        kit::row_s(t!("settings.providers.model_file"), format_size(info.size_bytes), control, cx)
    }

    /// Run test, Make default, and Delete download. Delete asks first, in place.
    fn cleanup_actions(&self, p: &Provider, info: &'static CleanupModelInfo, default: bool, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let mut row = div().flex().flex_none().items_center().gap(px(8.));
        let id = p.id.clone();
        if self.confirm_remove.as_deref() == Some(p.id.as_str()) {
            let question = t!("settings.providers.delete_confirm", name = info.name);
            let delete = t!("settings.providers.delete");
            return row
                .child(
                    div()
                        .id("delete-confirm")
                        .role(Role::Alert)
                        .aria_label(question.clone())
                        .a11y_live(question.clone(), true)
                        .child(plain(question, 13., 16., FontWeight::MEDIUM, c.ink)),
                )
                .child(Button::new("delete-yes", delete.clone()).a11y_label(format!("{delete}, {}", info.name)).danger().on_click(cx.listener(move |this, _, _, cx| {
                    this.confirm_remove = None;
                    this.open = None;
                    this.tests.remove(&id);
                    this.model.update(cx, |m, cx| m.delete_cleanup_model(info.id, cx));
                })))
                .child(Button::new("delete-no", t!("settings.providers.cancel")).a11y_label(format!("{}, {}", t!("settings.providers.cancel"), info.name)).ghost().on_click(cx.listener(|this, _, _, cx| {
                    this.confirm_remove = None;
                    cx.notify();
                })));
        }
        let ready = self.model.read(cx).cleanup_model_state(info.id) == CleanupModelState::Ready;
        let running = matches!(self.tests.get(&p.id), Some(Test::Running));
        let test_id = p.id.clone();
        row = row.child(
            Button::new("test-cleanup", t!("settings.providers.run_test"))
                .a11y_label(format!("{}, {}", t!("settings.providers.run_test"), info.name))
                .disabled(running || !ready)
                .on_click(cx.listener(move |this, _, _, cx| this.test(test_id.clone(), cx))),
        );
        if !default {
            let id = p.id.clone();
            row = row.child(Button::new("default-cleanup", t!("settings.providers.make_default")).a11y_label(format!("{}, {}", t!("settings.providers.make_default"), info.name)).on_click(cx.listener(move |this, _, _, cx| {
                let id = id.clone();
                this.model.update(cx, |m, cx| m.edit_config(cx, |c| c.ai.default_provider = Some(id)));
            })));
        }
        let id = p.id.clone();
        row.child(Button::new("delete-cleanup", t!("settings.providers.delete_download")).a11y_label(format!("{}, {}", t!("settings.providers.delete_download"), info.name)).danger().on_click(cx.listener(move |this, _, _, cx| {
            this.confirm_remove = Some(id.clone());
            cx.notify();
        })))
    }

    /// Test, Make default, and Remove. Remove asks first, in place.
    fn actions(&self, p: &Provider, default: bool, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let mut row = div().flex().flex_none().items_center().gap(px(8.));
        if self.confirm_remove.as_deref() == Some(p.id.as_str()) {
            let id = p.id.clone();
            // The question is an alert: a screen reader reads it when it shows.
            let question = t!("settings.providers.remove_confirm", name = p.name);
            return row
                .child(
                    div()
                        .id("remove-confirm")
                        .role(Role::Alert)
                        .aria_label(question.clone())
                        .a11y_live(question.clone(), true)
                        .child(plain(question, 13., 16., FontWeight::MEDIUM, c.ink)),
                )
                .child(Button::new("remove-yes", t!("settings.providers.remove")).a11y_label(format!("{}, {}", t!("settings.providers.remove"), p.name)).danger().on_click(cx.listener(move |this, _, _, cx| {
                    this.confirm_remove = None;
                    this.open = None;
                    this.tests.remove(&id);
                    this.model.update(cx, |m, cx| m.remove_provider(&id, cx));
                })))
                .child(Button::new("remove-no", t!("settings.providers.cancel")).a11y_label(format!("{}, {}", t!("settings.providers.cancel"), p.name)).ghost().on_click(cx.listener(|this, _, _, cx| {
                    this.confirm_remove = None;
                    cx.notify();
                })));
        }
        let running = matches!(self.tests.get(&p.id), Some(Test::Running));
        let id = p.id.clone();
        row = row.child(
            Button::new("test-provider", t!("settings.providers.test_connection"))
                .a11y_label(format!("{}, {}", t!("settings.providers.test_connection"), p.name))
                .disabled(running || p.needs_model())
                .on_click(cx.listener(move |this, _, _, cx| this.test(id.clone(), cx))),
        );
        if !default {
            let id = p.id.clone();
            row = row.child(Button::new("default-provider", t!("settings.providers.make_default")).a11y_label(format!("{}, {}", t!("settings.providers.make_default"), p.name)).on_click(cx.listener(move |this, _, _, cx| {
                let id = id.clone();
                this.model.update(cx, |m, cx| m.edit_config(cx, |c| c.ai.default_provider = Some(id)));
            })));
        }
        let id = p.id.clone();
        row.child(Button::new("remove-provider", t!("settings.providers.remove")).a11y_label(format!("{}, {}", t!("settings.providers.remove"), p.name)).danger().on_click(cx.listener(move |this, _, _, cx| {
            this.confirm_remove = Some(id.clone());
            cx.notify();
        })))
    }

    /// The whole text of a problem. The text wraps, and Copy takes the words
    /// of the tool.
    fn problem(&self, id: &str, problem: Problem, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let text_color = if c.is_dark() { c.danger } else { hsla(0.02, 0.6, 0.23, 1.0) };
        let copy = problem.details.clone().map(|details| {
            let all = format!("{}\n{details}", problem.message);
            pkit::link(SharedString::from(format!("copy-problem-{id}")), t!("settings.providers.copy_details"), 13., cx)
                .text_color(c.danger)
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(all.clone())))
        });
        let summary = problem.message.clone();
        div().w_full().min_w_0().pb(px(6.)).child(
            div()
                // An alert, as `kit::notice` is: a screen reader reads a new problem.
                .id(SharedString::from(format!("problem-{id}")))
                .role(Role::Alert)
                .aria_label(summary.clone())
                .a11y_live(summary, true)
                .flex()
                .items_start()
                .w_full()
                .min_w_0()
                .gap(px(12.))
                .px(px(14.))
                .py(px(12.))
                .rounded(px(10.))
                .bg(c.danger_wash)
                .border_1()
                .border_color(c.danger.opacity(0.18))
                .child(icon(Icon::Warning, 16., c.danger).mt(px(1.)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(6.))
                        .child(plain(problem.message, 13., 18., FontWeight::NORMAL, text_color))
                        .when_some(problem.details, |d, details| {
                            d.child(div().w_full().min_w_0().font_family(sayso_ui::fonts::MONO).text_size(px(12.)).line_height(px(17.)).text_color(text_color.opacity(0.85)).child(details))
                        }),
                )
                .children(copy),
        )
    }

    /// The panel of a sign-in that runs: the link to the page, and the field
    /// for the code when the tool takes one.
    fn sign_in_panel(&self, p: &Provider, open: &Open, takes_code: bool, url: Option<String>, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let (name, _) = tool(p);
        let about = if takes_code {
            t!("settings.providers.sign_in_code", tool = name)
        } else {
            t!("settings.providers.sign_in_wait", tool = name)
        };
        let link = url.filter(|u| u.starts_with("https://") || u.starts_with("http://")).map(|url| {
            pkit::link(SharedString::from(format!("sign-in-page-{}", p.id)), t!("settings.providers.open_sign_in_page"), 13., cx)
                .font_weight(FontWeight::SEMIBOLD)
                .on_click(move |_, _, _| AppModel::open_url(&url))
        });
        let mut panel = div()
            .id(SharedString::from(format!("sign-in-panel-{}", p.id)))
            .role(Role::Group)
            .aria_label(t!("settings.providers.finish_sign_in"))
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap(px(12.))
            .px(px(16.))
            .py(px(14.))
            .rounded(px(10.))
            .bg(c.accent_wash)
            .border_1()
            .border_color(c.accent.opacity(0.18))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(3.))
                            .child(plain(t!("settings.providers.finish_sign_in"), 14., 18., FontWeight::SEMIBOLD, c.ink))
                            .child(ui(about, 13., 18., FontWeight::NORMAL, c.graphite)),
                    )
                    .children(link),
            );
        if takes_code {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(pkit::input_well_named(&open.code, Some(t!("settings.providers.code_placeholder").into()), cx).flex_1().min_w_0().font_family(sayso_ui::fonts::MONO))
                    .child(Button::new("send-code", t!("settings.providers.send_code")).primary().on_click(cx.listener(|this, _, window, cx| this.send_code(window, cx)))),
            );
        }
        div().w_full().min_w_0().pb(px(6.)).child(panel)
    }

    /// The model of a provider, with its thinking level: "Haiku 5.5 · Low".
    /// A click opens the provider's list.
    fn model_picker(&self, p: &Provider, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let level = p.thinking_sent().filter(|sent| m.thinking_choices(p).iter().any(|level| level == sent)).map(thinking_label);
        let label = match p.model() {
            Some(id) => m.model_label(&p.id, id),
            None if p.needs_model() => t!("settings.providers.choose_model"),
            // macOS chooses the model. Its name comes with the list.
            None if p.kind == ProviderKind::AppleIntelligence => match m.model_lists.get(&p.id) {
                Some(crate::model::ModelList::Ready(list)) if !list.is_empty() => list[0].name.clone(),
                _ => t!("settings.providers.default_model"),
            },
            None => t!("settings.providers.default_model"),
        };
        let label = match level {
            Some(level) => format!("{label} · {level}"),
            None => label,
        };
        let pid = p.id.clone();
        model_picker::trigger(&format!("provider:{}", p.id), label, !p.needs_model(), cx)
            .on_click(cx.listener(move |this, _, _, cx| this.open_models(&pid, cx)))
            .into_any_element()
    }

    /// Open the model list of a saved provider under its trigger.
    fn open_models(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(p) = self.provider(id, cx) else { return };
        self.model.update(cx, |m, cx| m.load_provider_models(id, false, cx));
        let page = cx.entity().downgrade();
        let model = self.model.clone();
        let (pick_id, refresh_id) = (p.id.clone(), p.id.clone());
        let request = model_picker::Request {
            key: format!("provider:{}", p.id),
            list_key: p.id.clone(),
            selected: p.model().map(str::to_string),
            extra: None,
            thinking_for: Some(p.id.clone()),
            on_pick: Box::new(move |pick, cx| {
                let Some(choice) = pick else { return };
                let id = pick_id.clone();
                let _ = page.update(cx, |this, cx| {
                    this.tests.remove(&id);
                    this.model.update(cx, |m, cx| m.set_provider_model(&id, choice, cx));
                });
            }),
            on_refresh: Box::new(move |cx| model.update(cx, |m, cx| m.load_provider_models(&refresh_id, true, cx))),
        };
        model_picker::open(&self.model, request, cx);
    }

    /// A provider that Sayso found on this computer and that is not added yet.
    /// `by` is the publisher of a cleanup model, in a muted line after the title.
    #[allow(clippy::too_many_arguments)]
    fn suggestion(
        id: impl Into<SharedString>,
        title: &str,
        by: Option<String>,
        detail: String,
        button: String,
        on_add: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.paper().colors;
        let id: SharedString = id.into();
        div()
            .id(id.clone())
            .flex()
            .items_center()
            .w_full()
            .gap(px(14.))
            .py(px(14.))
            .border_b_1()
            .border_color(c.rule)
            .child(div().flex_none().size(px(8.)).rounded_full().border_1().border_color(c.success))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(ui(title.to_string(), 15., 18., FontWeight::SEMIBOLD, c.ink))
                            .children(by.map(|by| ui(by, 13., 18., FontWeight::NORMAL, c.graphite))),
                    )
                    .child(ui(detail, 13., 16., FontWeight::NORMAL, c.graphite)),
            )
            .child(Button::new(SharedString::from(format!("{id}-add")), button.clone())
                .a11y_label(format!("{button}, {title}"))
                .on_click(cx.listener(move |this, _, window, cx| on_add(this, window, cx))))
            .into_any_element()
    }

    /// The providers found on this computer that are not set up yet.
    fn suggestions(&self, providers: &[Provider], cx: &mut Context<Self>) -> Vec<AnyElement> {
        let detected = self.model.read(cx).detected.clone();
        let new_id = |name: &str| self.model.read(cx).new_provider_id(name);
        let mut found: Vec<(&'static str, &'static str, String, Provider)> = Vec::new();
        let has_ollama = providers.iter().any(|p| matches!(&p.kind, ProviderKind::OpenAiCompatible { base_url, .. } if base_url.contains(":11434")));
        if let (Some(models), false) = (&detected.ollama, has_ollama) {
            // With no model pulled, the picker opens after Add instead.
            let first = models.first().cloned().unwrap_or_default();
            let kind = ProviderKind::OpenAiCompatible { base_url: "http://localhost:11434/v1".into(), model: first, api_key_account: None, zero_data_retention: false, thinking: None };
            let detail = if models.is_empty() {
                t!("settings.providers.suggest_ollama_empty", computer = crate::shell::computer())
            } else {
                t!("settings.providers.suggest_ollama", count = models.len(), computer = crate::shell::computer())
            };
            found.push(("suggest-ollama", "Ollama", detail, Provider { id: new_id("ollama"), name: "Ollama".into(), kind }));
        }
        let has_claude = providers.iter().any(|p| matches!(p.kind, ProviderKind::ClaudeCli { .. }));
        if let (Some(path), false) = (&detected.claude_cli, has_claude) {
            let kind = ProviderKind::ClaudeCli { path: Some(path.display().to_string()), model: "haiku".into(), config_dir: None, thinking: Default::default() };
            let detail = t!("settings.providers.suggest_claude", computer = crate::shell::computer());
            found.push(("suggest-claude", "Claude CLI", detail, Provider { id: new_id("claude"), name: "Claude CLI".into(), kind }));
        }
        let has_codex = providers.iter().any(|p| matches!(p.kind, ProviderKind::CodexCli { .. }));
        if let (Some(path), false) = (&detected.codex_cli, has_codex) {
            let kind = ProviderKind::CodexCli { path: Some(path.display().to_string()), model: None, config_dir: None, thinking: Default::default() };
            let detail = t!("settings.providers.suggest_codex", computer = crate::shell::computer());
            found.push(("suggest-codex", "Codex CLI", detail, Provider { id: new_id("codex"), name: "Codex CLI".into(), kind }));
        }
        let has_apple = providers.iter().any(|p| matches!(p.kind, ProviderKind::AppleIntelligence));
        if let (Some(model), false) = (&detected.apple_intelligence, has_apple) {
            let p = Provider { id: new_id("apple-intelligence"), name: "Apple Intelligence".into(), kind: ProviderKind::AppleIntelligence };
            found.push(("suggest-apple", "Apple Intelligence", t!("settings.providers.suggest_apple", model = model), p));
        }
        let mut rows: Vec<AnyElement> = found
            .into_iter()
            .map(|(id, title, detail, provider)| {
                let add = t!("settings.providers.add");
                Self::suggestion(id, title, None, detail, add, move |this, window, cx| this.added(provider.clone(), None, window, cx), cx)
            })
            .collect();
        // The cleanup models need no tool on this computer, only a download.
        for info in cleanup::catalog().iter().filter(|info| !providers.iter().any(|p| cleanup_info(p).is_some_and(|i| i.id == info.id))) {
            let detail = t!("settings.providers.suggest_cleanup", computer = crate::shell::computer(), languages = languages_short(info), size = format_size(info.size_bytes));
            let by = t!("settings.providers.by", publisher = info.publisher);
            let model = info.id;
            rows.push(Self::suggestion(
                format!("suggest-{model}"),
                info.name,
                Some(by),
                detail,
                t!("settings.providers.download"),
                move |this, _, cx| {
                    // The first provider becomes the default.
                    this.model.update(cx, |m, cx| {
                        let make_default = m.config.ai.default_provider.is_none();
                        m.add_cleanup_model(model, make_default, cx);
                    });
                },
                cx,
            ));
        }
        rows
    }

    // -----------------------------------------------------------------------
    // Add a provider
    // -----------------------------------------------------------------------

    fn open_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.add.is_some() {
            self.add = None;
            cx.notify();
            return;
        }
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(t!("settings.providers.name_placeholder")));
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("https://openrouter.ai/api/v1"));
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!(crate::shell::os_text!("settings.providers.key_placeholder_mac", "settings.providers.key_placeholder_windows", "settings.providers.key_placeholder_other")))
                .masked(true)
        });
        let sub = cx.subscribe_in(&url, window, |_, _, _: &InputEvent, _, cx| cx.notify());
        name.update(cx, |s, cx| s.focus(window, cx));
        self.add = Some(AddProvider { kind: AddKind::OpenAi, name, url, key, error: None, _sub: sub });
        cx.notify();
    }

    fn apply_preset(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(add) = self.add.as_mut() else { return };
        let (name, url) = PRESETS[i];
        add.name.update(cx, |s, cx| s.set_value(name, window, cx));
        add.url.update(cx, |s, cx| s.set_value(url, window, cx));
        add.error = None;
        cx.notify();
    }

    fn save_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(add) = self.add.as_mut() else { return };
        let name = add.name.read(cx).value().trim().to_string();
        let detected = self.model.read(cx).detected.clone();
        let (name, kind, key) = match add.kind {
            AddKind::OpenAi => {
                let url = add.url.read(cx).value().trim().trim_end_matches('/').to_string();
                if name.is_empty() {
                    add.error = Some(AddError::NoName);
                } else if !(url.starts_with("http://") || url.starts_with("https://")) {
                    add.error = Some(AddError::BadUrl);
                }
                if add.error.is_some() {
                    cx.notify();
                    return;
                }
                let key = add.key.read(cx).value().trim().to_string();
                (name, Some(url), (!key.is_empty()).then_some(key))
            }
            AddKind::Claude | AddKind::Codex => {
                let claude = add.kind == AddKind::Claude;
                let found = if claude { detected.claude_cli.is_some() } else { detected.codex_cli.is_some() };
                if !found {
                    add.error = Some(if claude { AddError::NoClaudeCli } else { AddError::NoCodexCli });
                    cx.notify();
                    return;
                }
                (if claude { "Claude CLI".into() } else { "Codex CLI".into() }, None, None)
            }
        };
        let id = self.model.read(cx).new_provider_id(&name);
        let kind = match (add.kind, kind) {
            (AddKind::OpenAi, Some(base_url)) => ProviderKind::OpenAiCompatible {
                api_key_account: key.as_ref().map(|_| format!("provider.{id}")),
                zero_data_retention: base_url.contains("openrouter.ai"),
                base_url,
                model: String::new(),
                thinking: None,
            },
            // Haiku is the fast choice for cleanup; the picker offers the rest.
            (AddKind::Claude, _) => ProviderKind::ClaudeCli { path: detected.claude_cli.map(|p| p.display().to_string()), model: "haiku".into(), config_dir: None, thinking: Default::default() },
            _ => ProviderKind::CodexCli { path: detected.codex_cli.map(|p| p.display().to_string()), model: None, config_dir: None, thinking: Default::default() },
        };
        let provider = Provider { id, name, kind };
        self.add = None;
        self.added(provider, key, window, cx);
    }

    /// Save a new provider, open its row, and fetch its models. A provider
    /// that cannot run without a model opens its picker. A CLI provider shows
    /// its login. Other providers are tested right away.
    fn added(&mut self, provider: Provider, key: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let id = provider.id.clone();
        let needs_model = provider.needs_model();
        let cli = is_cli(&provider);
        self.model.update(cx, |m, cx| {
            m.save_provider(provider, key, false, cx);
            m.load_provider_models(&id, true, cx);
        });
        self.open_row(&id, window, cx);
        if needs_model {
            // The trigger records its place when it is drawn; open after that.
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(120)).await;
                let _ = this.update(cx, |this, cx| this.open_models(&id, cx));
            })
            .detach();
        } else if !cli {
            self.test(id, cx);
        }
        cx.notify();
    }

    fn add_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = cx.paper().colors;
        let add = self.add.as_ref()?;
        let (claude, codex) = {
            let m = self.model.read(cx);
            (m.detected.claude_cli.clone(), m.detected.codex_cli.clone())
        };
        let selected = match add.kind {
            AddKind::OpenAi => 0,
            AddKind::Claude => 1,
            AddKind::Codex => 2,
        };
        let entity = cx.entity();
        let seg = Segmented::new("add-kind", [t!("settings.providers.kind_openai"), "Claude CLI".to_string(), "Codex CLI".to_string()], selected)
            .label(t!("settings.providers.add_title"))
            .on_select(move |i, _, cx| {
            entity.update(cx, |this, cx| {
                if let Some(a) = this.add.as_mut() {
                    a.kind = [AddKind::OpenAi, AddKind::Claude, AddKind::Codex][i];
                    a.error = None;
                }
                cx.notify();
            })
        });

        let mut body = div().flex().flex_col().gap(px(16.));
        match add.kind {
            AddKind::OpenAi => {
                let mut presets = div().id("presets").role(Role::Group).aria_label(t!("settings.providers.start_from")).flex().gap(px(6.));
                for (i, (name, _)) in PRESETS.iter().enumerate() {
                    presets = presets.child(Chip::new(("preset", i), *name, false).on_click(cx.listener(move |this, _, w, cx| this.apply_preset(i, w, cx))));
                }
                let url = add.url.read(cx).value().to_string();
                let cloud = !url.trim().is_empty() && !is_local_url(&url);
                body = body
                    .child(pkit::field(&t!("settings.providers.start_from"), presets, None, cx))
                    .child(
                        div()
                            .flex()
                            .gap(px(12.))
                            .child(pkit::field(&t!("settings.providers.name"), pkit::input_well_named(&add.name, Some(t!("settings.providers.name").into()), cx), None, cx).w(px(220.)))
                            .child(pkit::field(&t!("settings.providers.base_url"), pkit::input_well_named(&add.url, Some(t!("settings.providers.base_url").into()), cx), None, cx).flex_1()),
                    )
                    .child(pkit::field(
                        &t!("settings.providers.api_key"),
                        pkit::secret_well_named(&add.key, t!("settings.providers.api_key").into(), cx),
                        Some(&t!("settings.providers.api_key_hint")),
                        cx,
                    ))
                    .when(cloud, |d| {
                        d.child(Banner::new(
                            BannerKind::Info,
                            t!("settings.providers.cloud_notice"),
                        ))
                    });
            }
            AddKind::Claude | AddKind::Codex => {
                let (path, name) = if add.kind == AddKind::Claude { (claude, "Claude CLI") } else { (codex, "Codex CLI") };
                let status = match path {
                    Some(p) => t!("settings.providers.cli_found", path = p.display()),
                    None => t!("settings.providers.cli_missing", tool = name),
                };
                body = body.child(ui(status, 13., 18., FontWeight::NORMAL, c.graphite)).child(Banner::new(
                    BannerKind::Info,
                    if add.kind == AddKind::Claude {
                        t!("settings.providers.claude_cli_notice")
                    } else {
                        t!("settings.providers.codex_cli_notice")
                    },
                ));
            }
        }
        let panel = div()
            // A form that opens in place. Escape closes it.
            .id("add-provider-form")
            .role(Role::Form)
            .aria_label(t!("settings.providers.add_title"))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.add = None;
                    cx.notify();
                }
            }))
            .flex()
            .flex_col()
            .gap(px(18.))
            .mt(px(14.))
            .p(px(22.))
            .rounded(px(14.))
            .raised(&c)
            .child(div().flex().items_center().justify_between().child(text::title(t!("settings.providers.add_title"), 22., &c)).child(seg))
            .child(body)
            .when_some(add.error, |d, e| d.child(Banner::new(BannerKind::Warning, e.text())))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(Button::new("save-provider", t!("settings.providers.add_provider")).primary().on_click(cx.listener(|this, _, window, cx| this.save_add(window, cx))))
                    .child(Button::new("cancel-provider", t!("settings.providers.cancel")).ghost().on_click(cx.listener(|this, _, _, cx| {
                        this.add = None;
                        cx.notify();
                    }))),
            );
        Some(panel.into_any_element())
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
                add.name.update(cx, |s, cx| s.set_placeholder(t!("settings.providers.name_placeholder"), window, cx));
                add.key.update(
                    cx,
                    |s, cx| s.set_placeholder(t!(crate::shell::os_text!("settings.providers.key_placeholder_mac", "settings.providers.key_placeholder_windows", "settings.providers.key_placeholder_other")), window, cx),
                );
            }
        }
        let c = cx.paper().colors;
        let (providers, default, enabled, searching) = {
            let m = self.model.read(cx);
            (m.config.ai.providers.clone(), m.config.ai.default_provider.clone(), m.config.ai.enabled, !m.detected.done)
        };
        // A provider that is gone has no row to keep open.
        if self.open.as_ref().is_some_and(|o| !providers.iter().any(|p| p.id == o.id)) {
            self.open = None;
        }
        let model = self.model.clone();
        let use_ai = kit::row(
            &t!("settings.providers.use_ai"),
            &t!("settings.providers.use_ai_detail"),
            Switch::new("ai-on", enabled).label(t!("settings.providers.use_ai")).on_toggle(move |on, _, cx| model.update(cx, |m, cx| m.set_ai_enabled(on, cx))),
            cx,
        );

        let mut list = group(&t!("settings.providers.your_providers"), cx);
        if providers.is_empty() {
            let line = if searching {
                t!("settings.providers.searching", computer = crate::shell::computer())
            } else {
                t!("settings.providers.none")
            };
            list = list.child(div().py(px(14.)).child(ui(line, 13., 16., FontWeight::NORMAL, c.graphite)));
        }
        for p in &providers {
            let is_default = default.as_deref() == Some(p.id.as_str());
            list = list.child(self.provider_row(p, is_default, cx));
        }
        list = list.children(self.suggestions(&providers, cx));
        if self.add.is_none() {
            list = list.child(
                pkit::dashed("add-provider", cx)
                    .label(t!("settings.providers.add_a_provider"))
                    .aria_description(t!("settings.providers.add_a_provider_detail"))
                    .justify_start()
                    .gap(px(12.))
                    .mt(px(14.))
                    .px(px(16.))
                    .py(px(14.))
                    .rounded(px(10.))
                    .on_click(cx.listener(|this, _, w, cx| this.open_add(w, cx)))
                    .child(icon(Icon::Plus, 14., c.graphite))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(plain(t!("settings.providers.add_a_provider"), 15., 18., FontWeight::SEMIBOLD, c.ink))
                            .child(plain(
                                t!("settings.providers.add_a_provider_detail"),
                                13.,
                                16.,
                                FontWeight::NORMAL,
                                c.graphite,
                            )),
                    ),
            );
        }
        list = list.children(self.add_panel(cx));

        kit::page(
            "providers-page",
            &t!("settings.providers.title"),
            &t!("settings.providers.subtitle"),
            kit::body().child(div().flex().flex_col().w_full().min_w_0().child(use_ai)).child(list),
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    // No glob import: `gpui_kit::*` has its own `test` attribute.
    use super::{cleanup, cleanup_language, languages_short, list, style_names};
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
}
