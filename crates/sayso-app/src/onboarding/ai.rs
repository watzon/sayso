//! Step 3: optional AI clean-up. Continue saves the provider. Skip turns AI off.

use super::{OnboardingView, arrow_target, heading, live_ui, plain, size_text};
use crate::model::{AppModel, ModelList};
use crate::model_picker;
use gpui_kit::component::input::{Input, InputContentType, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_i18n::t;
use sayso_core::cleanup::{self, CleanupModelInfo};
use sayso_core::config::{Provider, ProviderKind, is_local_url};
use sayso_core::languages;
use sayso_ui::a11y::{A11yExt as _, Pressable, Role};
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::{self, PaperStyled};
use sayso_ui::{ActivePaper, Colors, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AiChoice {
    Apple,
    Local,
    Cloud,
    Claude,
    /// A cleanup model: its place in the catalog.
    Cleanup(usize),
}

/// The choices before the cleanup models.
const FIXED_CHOICES: usize = 4;

impl AiChoice {
    /// The place of the choice in `AiForm::rows`.
    fn slot(self) -> usize {
        match self {
            AiChoice::Apple => 0,
            AiChoice::Local => 1,
            AiChoice::Cloud => 2,
            AiChoice::Claude => 3,
            AiChoice::Cleanup(i) => FIXED_CHOICES + i,
        }
    }
}

/// What is wrong with the form. The form keeps the kind, not the text, so the
/// text follows the interface language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AiError {
    Choose,
    /// With the reason code of the engine, when it sent one.
    AppleUnavailable(Option<sayso_core::stt::EngineReason>),
    OllamaNoModels,
    BaseUrl,
    ApiKey,
    ChooseModel,
    NoClaude,
}

impl AiError {
    fn text(self) -> String {
        match self {
            AiError::Choose => t!("onboarding.ai.error_choose"),
            AiError::AppleUnavailable(Some(reason)) => t!("onboarding.ai.error_apple_reason", reason = crate::engine_text::describe(Some(reason), "")),
            AiError::AppleUnavailable(None) => t!("onboarding.ai.error_apple"),
            AiError::OllamaNoModels => t!("onboarding.ai.error_ollama"),
            AiError::BaseUrl => t!("onboarding.ai.error_base_url"),
            AiError::ApiKey => t!("onboarding.ai.error_api_key", store = crate::shell::secret_store()),
            AiError::ChooseModel => t!("onboarding.ai.error_model"),
            AiError::NoClaude => t!("onboarding.ai.error_claude"),
        }
    }
}

/// Base URL presets for the cloud option: (label, id, url).
const PRESETS: [(&str, &str, &str); 3] = [
    ("OpenRouter", "openrouter", "https://openrouter.ai/api/v1"),
    ("OpenAI", "openai", "https://api.openai.com/v1"),
    ("Other", "custom", ""),
];

/// The model list key for the cloud form, before the provider is saved.
const DRAFT: &str = "onboarding-cloud";
/// The Keychain account name the draft provider uses for its key in memory.
const DRAFT_ACCOUNT: &str = "draft";

pub(super) struct AiForm {
    choice: Option<AiChoice>,
    local_model: Option<String>,
    local_open: bool,
    preset: usize,
    base_url: Entity<InputState>,
    /// Chosen from the provider's list. None until the user picks one.
    model: Option<String>,
    /// The (URL, key) the draft list was fetched with, so a change fetches again.
    fetched: Option<(String, String)>,
    pub(super) key: Entity<InputState>,
    error: Option<AiError>,
    test: Option<Result<u64, String>>,
    testing: bool,
    /// One handle for each choice, around its radio button. The arrow keys
    /// use them to find out where the focus is and to move it.
    rows: Vec<FocusHandle>,
}

impl AiForm {
    pub(super) fn new(model: &Entity<AppModel>, window: &mut Window, cx: &mut App) -> Self {
        let config = model.read(cx).config.ai.clone();
        // Start from the saved provider, if onboarding resumes here.
        let saved = config.enabled.then(|| config.provider(None).cloned()).flatten();
        let (choice, preset, url, model_name, local) = match saved.as_ref().map(|p| &p.kind) {
            Some(ProviderKind::OpenAiCompatible { base_url, model, .. }) if is_local_url(base_url) => {
                (Some(AiChoice::Local), 0, PRESETS[0].2.to_string(), String::new(), Some(model.clone()))
            }
            Some(ProviderKind::OpenAiCompatible { base_url, model, .. }) => {
                let p = PRESETS.iter().position(|p| p.2 == base_url).unwrap_or(2);
                (Some(AiChoice::Cloud), p, base_url.clone(), model.clone(), None)
            }
            Some(ProviderKind::ClaudeCli { .. }) => (Some(AiChoice::Claude), 0, PRESETS[0].2.to_string(), String::new(), None),
            Some(ProviderKind::AppleIntelligence) => (Some(AiChoice::Apple), 0, PRESETS[0].2.to_string(), String::new(), None),
            Some(ProviderKind::CleanupModel { model }) => {
                let place = cleanup::catalog().iter().position(|info| info.id == model);
                (place.map(AiChoice::Cleanup), 0, PRESETS[0].2.to_string(), String::new(), None)
            }
            _ => (None, 0, PRESETS[0].2.to_string(), String::new(), None),
        };
        let base_url = crate::widgets::input_state("https://…/v1", &url, window, cx);
        let key = cx.new(|cx| InputState::new(window, cx).placeholder(t!("onboarding.ai.key_placeholder")).masked(true));
        Self {
            choice,
            local_model: local,
            local_open: false,
            preset,
            base_url,
            model: (!model_name.is_empty()).then_some(model_name),
            fetched: None,
            key,
            error: None,
            test: None,
            testing: false,
            rows: (0..FIXED_CHOICES + cleanup::catalog().len()).map(|_| cx.focus_handle()).collect(),
        }
    }
}

impl OnboardingView {
    /// The cloud form's model row: a picker over the list the endpoint reports.
    fn cloud_model(&mut self, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let list = m.model_lists.get(DRAFT).cloned();
        let label = match (&self.ai.model, &list) {
            (Some(id), Some(ModelList::Ready(models))) => models.iter().find(|x| &x.id == id).map(|x| x.name.clone()).unwrap_or_else(|| id.clone()),
            (Some(id), _) => id.clone(),
            (None, _) => t!("onboarding.ai.choose_model"),
        };
        let chosen = self.ai.model.is_some();
        let trigger = Pressable::new("cloud-model")
            .label(t!("onboarding.ai.model_button", name = t!("onboarding.ai.model"), value = label.clone()))
            .relative()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .justify_between()
            .h(px(34.))
            .px(px(12.))
            .rounded(px(9.))
            .bg(c.sheet_raised)
            .shadows(paper::raised_small(c))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.load_cloud_models(false, cx);
                let view = cx.entity().downgrade();
                let refresh = cx.entity().downgrade();
                let request = model_picker::Request {
                    key: "onboarding-cloud".into(),
                    list_key: DRAFT.into(),
                    selected: this.ai.model.clone(),
                    extra: None,
                    thinking_for: None,
                    on_pick: Box::new(move |pick, cx| {
                        let _ = view.update(cx, |v, cx| {
                            if pick.is_some() {
                                v.ai.model = pick;
                                v.ai.test = None;
                                v.ai.error = None;
                            }
                            cx.notify();
                        });
                    }),
                    on_refresh: Box::new(move |cx| {
                        let _ = refresh.update(cx, |v, cx| v.load_cloud_models(true, cx));
                    }),
                };
                model_picker::open(&this.model, request, cx);
            }))
            .child(plain(label, 13., FontWeight::NORMAL, if chosen { c.ink } else { c.accent }).truncate())
            .child(icon(Icon::ChevronDown, 12., c.graphite))
            .child(model_picker::anchor("onboarding-cloud"));
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(text::ui(t!("onboarding.ai.model"), 13., FontWeight::MEDIUM, c.graphite).w(px(70.)).flex_none())
            .child(trigger)
            .into_any_element()
    }

    /// Fetch the model list for the URL and key in the form. A changed URL or
    /// key fetches again even without `force`.
    fn load_cloud_models(&mut self, force: bool, cx: &mut Context<Self>) {
        let url = self.ai.base_url.read(cx).value().trim().trim_end_matches('/').to_string();
        let key = self.ai.key.read(cx).value().trim().to_string();
        let signature = (url.clone(), key.clone());
        let changed = self.ai.fetched.as_ref() != Some(&signature);
        self.ai.fetched = Some(signature);
        let provider = Provider {
            id: DRAFT.into(),
            name: "Draft".into(),
            kind: ProviderKind::OpenAiCompatible {
                base_url: url,
                model: String::new(),
                api_key_account: (!key.is_empty()).then(|| DRAFT_ACCOUNT.to_string()),
                zero_data_retention: false,
                thinking: None,
            },
        };
        let api_key = (!key.is_empty()).then_some(key);
        self.model.update(cx, |m, cx| m.load_draft_models(DRAFT, provider, api_key, force || changed, cx));
    }

    /// The selected option. None when the user has not chosen one: Continue
    /// is then off, and Skip is the way on.
    pub(super) fn ai_choice(&self, cx: &App) -> Option<AiChoice> {
        let m = self.model.read(cx);
        self.ai.choice.or_else(|| self.suggested_cleanup(cx)).or_else(|| m.detected.ollama.as_ref().is_some_and(|l| !l.is_empty()).then_some(AiChoice::Local))
    }

    /// The first cleanup model that knows the dictation language. The speech
    /// model sets the language: an English-only model hears English, and
    /// "auto" with a multilingual model names no language.
    fn suggested_cleanup(&self, cx: &App) -> Option<AiChoice> {
        let m = self.model.read(cx);
        let language = m.active_model().language_for(&m.config.dictation.language);
        cleanup::catalog().iter().position(|info| info.languages.contains(&language.as_str())).map(AiChoice::Cleanup)
    }

    /// True when Continue downloads a cleanup model.
    pub(super) fn ai_downloads(&self, cx: &App) -> bool {
        matches!(self.ai_choice(cx), Some(AiChoice::Cleanup(_)))
    }

    /// The provider for the current form, or a message that says what to fix.
    fn ai_provider(&self, cx: &App) -> Result<(Provider, Option<String>), AiError> {
        let m = self.model.read(cx);
        let choice = self.ai_choice(cx).ok_or(AiError::Choose)?;
        match choice {
            AiChoice::Apple => {
                if m.detected.apple_intelligence.is_none() {
                    return Err(AiError::AppleUnavailable(m.detected.apple_unavailable.as_ref().and_then(|u| u.reason)));
                }
                Ok((
                    Provider { id: "apple-intelligence".into(), name: "Apple Intelligence".into(), kind: ProviderKind::AppleIntelligence },
                    None,
                ))
            }
            AiChoice::Local => {
                let models = m.detected.ollama.clone().unwrap_or_default();
                let model = self.ai.local_model.clone().or_else(|| models.first().cloned()).ok_or(AiError::OllamaNoModels)?;
                Ok((
                    Provider {
                        id: "ollama".into(),
                        name: "Ollama".into(),
                        kind: ProviderKind::OpenAiCompatible {
                            base_url: "http://localhost:11434/v1".into(),
                            model,
                            api_key_account: None,
                            zero_data_retention: false,
                            thinking: None,
                        },
                    },
                    None,
                ))
            }
            AiChoice::Cloud => {
                let url = self.ai.base_url.read(cx).value().trim().trim_end_matches('/').to_string();
                let key = self.ai.key.read(cx).value().trim().to_string();
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return Err(AiError::BaseUrl);
                }
                if key.is_empty() && !is_local_url(&url) {
                    return Err(AiError::ApiKey);
                }
                let model = self.ai.model.clone().ok_or(AiError::ChooseModel)?;
                let (label, id, preset_url) = PRESETS[self.ai.preset];
                let (id, name) = if preset_url == url { (id.to_string(), label.to_string()) } else { ("custom".to_string(), "Custom endpoint".to_string()) };
                Ok((
                    Provider {
                        id: id.clone(),
                        name,
                        kind: ProviderKind::OpenAiCompatible {
                            base_url: url,
                            model,
                            api_key_account: (!key.is_empty()).then_some(id),
                            zero_data_retention: false,
                            thinking: None,
                        },
                    },
                    (!key.is_empty()).then_some(key),
                ))
            }
            AiChoice::Cleanup(i) => {
                let info = &cleanup::catalog()[i];
                Ok((Provider { id: info.id.into(), name: info.name.into(), kind: ProviderKind::CleanupModel { model: info.id.into() } }, None))
            }
            AiChoice::Claude => {
                let path = m.detected.claude_cli.clone().ok_or(AiError::NoClaude)?;
                Ok((
                    Provider {
                        id: "claude-cli".into(),
                        name: "Claude CLI".into(),
                        kind: ProviderKind::ClaudeCli { path: Some(path.display().to_string()), model: "haiku".into(), config_dir: None, thinking: Default::default() },
                    },
                    None,
                ))
            }
        }
    }

    /// Save the choice. Returns false when the form needs a fix.
    pub(super) fn save_ai(&mut self, cx: &mut Context<Self>) -> bool {
        // A cleanup model downloads in the background: the step does not wait for it.
        if let Some(AiChoice::Cleanup(i)) = self.ai_choice(cx) {
            self.ai.error = None;
            self.model.update(cx, |m, cx| m.add_cleanup_model(cleanup::catalog()[i].id, true, cx));
            return true;
        }
        match self.ai_provider(cx) {
            Err(e) => {
                self.ai.error = Some(e);
                false
            }
            Ok((provider, key)) => {
                self.ai.error = None;
                self.model.update(cx, |m, cx| {
                    let id = provider.id.clone();
                    m.save_provider(provider, key, true, cx);
                    // Keep the list the user picked from, so Styles shows its names.
                    if let Some(list @ ModelList::Ready(_)) = m.model_lists.get(DRAFT).cloned() {
                        m.model_lists.insert(id, list);
                    }
                });
                true
            }
        }
    }

    /// Leave the step without a provider. A provider that "Test connection"
    /// saved stays in Settings, but AI is off.
    pub(super) fn skip_ai(&mut self, cx: &mut Context<Self>) {
        self.ai.error = None;
        if self.model.read(cx).config.ai.enabled {
            self.model.update(cx, |m, cx| m.set_ai_enabled(false, cx));
        }
    }

    fn test_ai(&mut self, cx: &mut Context<Self>) {
        let provider = match self.ai_provider(cx) {
            Ok(provider) => provider,
            Err(e) => {
                self.ai.error = Some(e);
                cx.notify();
                return;
            }
        };
        self.ai.error = None;
        self.ai.testing = true;
        self.ai.test = None;
        let id = provider.0.id.clone();
        let view = cx.entity().downgrade();
        self.model.update(cx, |m, cx| {
            m.save_provider(provider.0, provider.1, true, cx);
            m.test_provider(&id, cx, move |_, result, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.ai.testing = false;
                    this.ai.test = Some(result);
                    cx.notify();
                });
            });
        });
        cx.notify();
    }

    fn select_ai(&mut self, choice: AiChoice, cx: &mut Context<Self>) {
        self.ai.choice = Some(choice);
        self.ai.error = None;
        self.ai.test = None;
        // Ask Ollama again: the user can pull a model while this step is open.
        if choice == AiChoice::Local {
            self.model.update(cx, |m, cx| m.detect_providers(cx));
        }
        cx.notify();
    }

    pub(super) fn ai_step(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let choice = self.ai_choice(cx);
        let d = self.model.read(cx).detected.clone();
        let home = sayso_core::paths::PathEnv::home(&sayso_core::paths::SystemEnv);

        // The choices that this computer has, in the order of the list.
        let mut order: Vec<AiChoice> = (0..cleanup::catalog().len()).map(AiChoice::Cleanup).collect();
        if d.apple_intelligence.is_some() {
            order.push(AiChoice::Apple);
        }
        if d.ollama.is_some() {
            order.push(AiChoice::Local);
        }
        order.push(AiChoice::Cloud);
        if d.claude_cli.is_some() {
            order.push(AiChoice::Claude);
        }
        let selected_index = order.iter().position(|x| Some(*x) == choice);
        let keys = order.clone();
        // One choice of a set: the rows are radio buttons, and the arrow keys
        // change the choice. Esc closes the list of Ollama models.
        let mut rows = div()
            .id("ai-providers")
            .role(Role::RadioGroup)
            .aria_label(t!("onboarding.ai.title"))
            .flex()
            .flex_col()
            .gap(px(8.))
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" && this.ai.local_open {
                    cx.stop_propagation();
                    this.ai.local_open = false;
                    cx.notify();
                    return;
                }
                // The arrow keys belong to an input or a list inside a row.
                if !keys.iter().any(|ch| this.ai.rows[ch.slot()].contains_focused(window, cx)) {
                    return;
                }
                let Some((next, _)) = arrow_target(&e.keystroke.key, selected_index, keys.len()) else { return };
                cx.stop_propagation();
                this.select_ai(keys[next], cx);
                window.focus(&this.ai.rows[keys[next].slot()], cx);
            }));
        if !d.done {
            rows = rows.child(text::ui(t!("onboarding.ai.looking", computer = crate::shell::computer()), 13., FontWeight::NORMAL, c.graphite).px(px(4.)));
        }
        for (i, info) in cleanup::catalog().iter().enumerate() {
            rows = rows.child(self.ai_row(
                AiChoice::Cleanup(i),
                choice,
                &order,
                info.name.into(),
                Some(t!("onboarding.ai.by", publisher = info.publisher)),
                cleanup_desc(info),
                Some(size_badge(info, &c)),
                None,
                None,
                &c,
                cx,
            ));
        }
        if let Some(model) = &d.apple_intelligence {
            rows = rows.child(self.ai_row(
                AiChoice::Apple,
                choice,
                &order,
                "Apple Intelligence".into(),
                None,
                t!("onboarding.ai.apple_desc", model = model),
                None,
                None,
                None,
                &c,
                cx,
            ));
        }
        if d.ollama.as_ref().is_some_and(|l| l.is_empty()) {
            rows = rows.child(self.ai_row(
                AiChoice::Local,
                choice,
                &order,
                "Ollama".into(),
                None,
                t!("onboarding.ai.ollama_empty", computer = crate::shell::computer()),
                None,
                None,
                None,
                &c,
                cx,
            ));
        }
        if let Some(models) = d.ollama.clone().filter(|l| !l.is_empty()) {
            let current = self.ai.local_model.clone().unwrap_or_else(|| models[0].clone());
            let open = self.ai.local_open;
            let picker = Pressable::new("ollama-model")
                .label(t!("onboarding.ai.model_button", name = t!("onboarding.ai.ollama_model"), value = current.clone()))
                .aria_expanded(open)
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(30.))
                .px(px(10.))
                .rounded(px(8.))
                .bg(c.deboss)
                .shadows(vec![BoxShadow::new(px(0.), px(1.), c.shadow(0.2)).blur_radius(px(2.)).inset()])
                .cursor_pointer()
                .child(plain(current.clone(), 13., FontWeight::NORMAL, c.ink))
                .child(icon(Icon::ChevronDown, 10., c.graphite))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.ai.choice = Some(AiChoice::Local);
                    this.ai.local_open = !this.ai.local_open;
                    cx.notify();
                }));
            let mut extra = None;
            if open && choice == Some(AiChoice::Local) {
                let count = models.len();
                let selected_model = models.iter().position(|m| *m == current);
                let names = models.clone();
                let mut list = div()
                    .id("ollama-models")
                    .role(Role::RadioGroup)
                    .aria_label(t!("onboarding.ai.ollama_model"))
                    .flex()
                    .flex_col()
                    .p(px(4.))
                    .rounded(px(10.))
                    .debossed(&c)
                    .on_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| {
                        let Some((next, forward)) = arrow_target(&e.keystroke.key, selected_model, count) else { return };
                        cx.stop_propagation();
                        this.ai.local_model = Some(names[next].clone());
                        this.ai.test = None;
                        cx.notify();
                        if forward { window.focus_next(cx) } else { window.focus_prev(cx) }
                    }));
                for (i, name) in models.into_iter().enumerate() {
                    let selected = name == current;
                    let n = name.clone();
                    list = list.child(
                        Pressable::new(SharedString::from(format!("ollama-pick-{name}")))
                            .a11y_radio(name.clone(), selected, i, count)
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(30.))
                            .px(px(10.))
                            .rounded(px(7.))
                            .cursor_pointer()
                            .hover(|s| s.bg(c.sheet_raised))
                            .child(div().w(px(14.)).when(selected, |d| d.child(icon(Icon::Check, 12., c.ink))))
                            .child(plain(name, 13., if selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }, c.ink))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.ai.local_model = Some(n.clone());
                                this.ai.local_open = false;
                                this.ai.test = None;
                                cx.notify();
                            })),
                    );
                }
                extra = Some(list.into_any_element());
            }
            rows = rows.child(self.ai_row(
                AiChoice::Local,
                choice,
                &order,
                "Ollama".into(),
                None,
                t!("onboarding.ai.ollama_running", computer = crate::shell::computer()),
                None,
                Some(picker.into_any_element()),
                extra,
                &c,
                cx,
            ));
        }
        let cloud_extra = (choice == Some(AiChoice::Cloud)).then(|| self.cloud_form(&c, cx));
        rows = rows.child(self.ai_row(
            AiChoice::Cloud,
            choice,
            &order,
            t!("onboarding.ai.cloud_api"),
            None,
            t!("onboarding.ai.cloud_desc", store = crate::shell::secret_store()),
            Some(Badge::new(t!("onboarding.ai.cloud"), BadgeTone::Accent).into_any_element()),
            None,
            cloud_extra,
            &c,
            cx,
        ));
        if let Some(path) = &d.claude_cli {
            let shown = sayso_core::paths::Paths::display(path, &home);
            rows = rows.child(self.ai_row(
                AiChoice::Claude,
                choice,
                &order,
                "Claude CLI".into(),
                None,
                t!("onboarding.ai.claude_desc", path = shown),
                Some(Badge::new(t!("onboarding.ai.cloud"), BadgeTone::Accent).into_any_element()),
                None,
                None,
                &c,
                cx,
            ));
        }

        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(22.))
            .pt(px(28.))
            .px(px(self.side()))
            .pb(px(24.))
            .child(heading(&t!("onboarding.ai.title"), &t!("onboarding.ai.lead"), &c))
            .child(rows);
        if let Some(e) = self.ai.error.filter(|_| choice != Some(AiChoice::Cloud)) {
            col = col.child(crate::hub::settings::kit::notice(BannerKind::Warning, e.text(), cx));
        }
        col.into_any_element()
    }

    /// One choice of the list. The radio button holds the dot, the title and
    /// the description. `trailing` (a badge) is inside it. `control` and
    /// `extra` are outside it, because a button cannot hold another control.
    #[allow(clippy::too_many_arguments)]
    fn ai_row(
        &self,
        me: AiChoice,
        choice: Option<AiChoice>,
        order: &[AiChoice],
        title: String,
        publisher: Option<String>,
        desc: String,
        trailing: Option<AnyElement>,
        control: Option<AnyElement>,
        extra: Option<AnyElement>,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = Some(me) == choice;
        let index = order.iter().position(|x| *x == me).unwrap_or(0);
        // The name for a screen reader has the publisher: "S1-mini by Superwhisper".
        let name = match &publisher {
            Some(by) => format!("{title} {by}"),
            None => title.clone(),
        };
        let radio = Pressable::new(SharedString::from(format!("ai-{me:?}")))
            .track_focus_handle(&self.ai.rows[me.slot()])
            .a11y_radio(name, selected, index, order.len())
            .aria_description(desc.clone())
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(14.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.select_ai(me, cx)))
            .child(radio_dot(selected, cx))
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
                            .child(plain(title, 15., FontWeight::SEMIBOLD, c.ink).line_height(px(18.)))
                            .when_some(publisher, |d, by| d.child(plain(by, 13., FontWeight::NORMAL, c.graphite).line_height(px(18.)))),
                    )
                    .child(plain(desc, 13., FontWeight::NORMAL, c.graphite).line_height(px(16.))),
            )
            .when_some(trailing, |d, t| d.child(t));
        let mut card = div()
            .id(SharedString::from(format!("ai-card-{me:?}")))
            .flex()
            .flex_col()
            .rounded(px(12.));
        card = if selected {
            let mut s = vec![BoxShadow::new(px(0.), px(0.), c.ink).spread_radius(px(2.))];
            s.push(BoxShadow::new(px(0.), px(1.), c.shadow(0.14)).blur_radius(px(2.)));
            card.bg(c.sheet_raised).shadow(s)
        } else {
            card.bg(c.sheet_raised).shadow(paper::raised_small(c)).hover(|s| s.bg(c.sheet))
        };
        card.child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .py(px(14.))
                .px(px(16.))
                .child(div().flex().flex_1().min_w_0().child(radio))
                .when_some(control, |d, t| d.child(t)),
        )
        .when_some(extra, |d, e| d.child(div().px(px(16.)).pb(px(14.)).child(div().pl(px(32.)).child(e))))
        .into_any_element()
    }

    /// A preset of the cloud form: its URL goes in the field.
    fn select_preset(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.ai.preset = i;
        self.ai.test = None;
        self.ai.model = None;
        self.ai.base_url.update(cx, |s, cx| s.set_value(PRESETS[i].2, window, cx));
        cx.notify();
    }

    fn cloud_form(&mut self, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let preset = self.ai.preset;
        let mut presets = div()
            .id("ai-presets")
            .role(Role::RadioGroup)
            .aria_label(t!("onboarding.ai.preset_label"))
            .flex()
            .gap(px(6.))
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| {
                let Some((next, forward)) = arrow_target(&e.keystroke.key, Some(preset), PRESETS.len()) else { return };
                cx.stop_propagation();
                this.select_preset(next, window, cx);
                if forward { window.focus_next(cx) } else { window.focus_prev(cx) }
            }));
        for (i, (label, id, _)) in PRESETS.into_iter().enumerate() {
            // The last preset is any other endpoint.
            let label = if id == "custom" { t!("onboarding.ai.preset_other") } else { label.to_string() };
            presets = presets.child(
                Chip::new(("preset", i), label, self.ai.preset == i)
                    .role(ChipRole::Choice { index: i, count: PRESETS.len() })
                    .on_click(cx.listener(move |this, _, window, cx| this.select_preset(i, window, cx))),
            );
        }
        let field = |label: String, state: &Entity<InputState>, mono: bool, secret: bool| {
            let label_name = label.clone();
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(text::ui(label, 13., FontWeight::MEDIUM, c.graphite).w(px(70.)).flex_none())
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .h(px(34.))
                        .px(px(12.))
                        .rounded(px(9.))
                        .debossed(c)
                        .text_size(px(13.))
                        .when(mono, |d| d.font_family(sayso_ui::fonts::MONO))
                        .child(Input::new(state).appearance(false).w_full().aria_label(label_name).when(secret, |i| i.content_type(InputContentType::Password))),
                )
        };
        // One live node for the test: a screen reader reads the result when it arrives.
        let test_line: AnyElement = if self.ai.testing {
            live_ui("ai-test-status", t!("onboarding.ai.testing"), 13., FontWeight::NORMAL, c.graphite, false).into_any_element()
        } else {
            match &self.ai.test {
                Some(Ok(ms)) => {
                    let works = t!("onboarding.ai.test_works", seconds = sayso_i18n::numbers::decimal(*ms as f64 / 1000.0, 1));
                    div()
                        .id("ai-test-status")
                        .role(Role::Status)
                        .a11y_live(works.clone(), false)
                        .child(super::check_line(works, c))
                        .into_any_element()
                }
                Some(Err(e)) => live_ui("ai-test-status", t!("onboarding.ai.test_failed", error = e), 13., FontWeight::NORMAL, c.danger, true).into_any_element(),
                None => div().into_any_element(),
            }
        };
        let error = self.ai.error.map(|e| crate::hub::settings::kit::notice(BannerKind::Warning, e.text(), cx));
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(presets)
            .child(field(t!("onboarding.ai.base_url"), &self.ai.base_url, true, false))
            .child(field(t!("onboarding.ai.api_key"), &self.ai.key, true, true))
            .child(self.cloud_model(c, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .pl(px(82.))
                    .child(Button::new("test-ai", t!("onboarding.ai.test_connection")).small().disabled(self.ai.testing).on_click(cx.listener(|this, _, _, cx| this.test_ai(cx))))
                    .child(test_line),
            )
            .when_some(error, |d, e| d.child(e))
            .into_any_element()
    }
}

/// "English only. Formats emails and lists."
fn cleanup_desc(info: &CleanupModelInfo) -> String {
    let formats = if info.formats.len() > 1 || info.layouts.len() > 1 { t!("onboarding.ai.cleanup_formats") } else { t!("onboarding.ai.cleanup_no_formats") };
    let first = info.languages.first().map(|code| languages::display(code)).unwrap_or_default();
    match info.languages.len() {
        0 | 1 => t!("onboarding.ai.cleanup_desc_one", language = first, formats = formats),
        n => t!("onboarding.ai.cleanup_desc_many", count = n - 1, language = first, formats = formats),
    }
}

/// The download size, in the green of a local choice.
fn size_badge(info: &CleanupModelInfo, c: &Colors) -> AnyElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(22.))
        .px(px(8.))
        .rounded_full()
        .bg(c.success.opacity(0.14))
        .child(plain(size_text(info.size_bytes), 12., FontWeight::SEMIBOLD, c.success))
        .into_any_element()
}
