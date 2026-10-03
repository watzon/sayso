//! Step 3: optional AI clean-up. Continue saves the provider (or turns AI off).

use super::{OnboardingView, heading};
use crate::model::{AppModel, ModelList};
use crate::model_picker;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::config::{Provider, ProviderKind, is_local_url};
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::{self, PaperStyled};
use sayso_ui::{ActivePaper, Colors, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AiChoice {
    Local,
    Cloud,
    Claude,
    NotNow,
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
    key: Entity<InputState>,
    error: Option<String>,
    test: Option<Result<u64, String>>,
    testing: bool,
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
            _ => (None, 0, PRESETS[0].2.to_string(), String::new(), None),
        };
        let base_url = crate::widgets::input_state("https://…/v1", &url, window, cx);
        let key = cx.new(|cx| InputState::new(window, cx).placeholder("Paste your API key").masked(true));
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
            (None, _) => "Choose a model".into(),
        };
        let chosen = self.ai.model.is_some();
        let trigger = div()
            .id("cloud-model")
            .relative()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .justify_between()
            .h(px(34.))
            .px(px(12.))
            .rounded(px(9.))
            .raised_small(c)
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
            .child(text::ui(label, 13., FontWeight::NORMAL, if chosen { c.ink } else { c.accent }).truncate())
            .child(icon(Icon::ChevronDown, 12., c.graphite))
            .child(model_picker::anchor("onboarding-cloud"));
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(text::ui("Model", 13., FontWeight::MEDIUM, c.graphite).w(px(70.)).flex_none())
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
            },
        };
        let api_key = (!key.is_empty()).then_some(key);
        self.model.update(cx, |m, cx| m.load_draft_models(DRAFT, provider, api_key, force || changed, cx));
    }

    fn ai_choice(&self, cx: &App) -> AiChoice {
        let d = &self.model.read(cx).detected;
        self.ai.choice.unwrap_or(if d.ollama.as_ref().is_some_and(|l| !l.is_empty()) { AiChoice::Local } else { AiChoice::NotNow })
    }

    /// The provider for the current form, or a message that says what to fix.
    fn ai_provider(&self, cx: &App) -> Result<Option<(Provider, Option<String>)>, String> {
        let m = self.model.read(cx);
        match self.ai_choice(cx) {
            AiChoice::NotNow => Ok(None),
            AiChoice::Local => {
                let models = m.detected.ollama.clone().unwrap_or_default();
                let model = self.ai.local_model.clone().or_else(|| models.first().cloned()).ok_or("Ollama has no models. Run \"ollama pull qwen3:4b\", then try again.")?;
                Ok(Some((
                    Provider {
                        id: "ollama".into(),
                        name: "Ollama".into(),
                        kind: ProviderKind::OpenAiCompatible {
                            base_url: "http://localhost:11434/v1".into(),
                            model,
                            api_key_account: None,
                            zero_data_retention: false,
                        },
                    },
                    None,
                )))
            }
            AiChoice::Cloud => {
                let url = self.ai.base_url.read(cx).value().trim().trim_end_matches('/').to_string();
                let key = self.ai.key.read(cx).value().trim().to_string();
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return Err("Enter the base URL of the API, for example https://openrouter.ai/api/v1.".into());
                }
                if key.is_empty() && !is_local_url(&url) {
                    return Err(crate::shell::os_text!("Enter your API key. Sayso keeps it in the Keychain.", "Enter your API key. Sayso keeps it in the system keyring.").into());
                }
                let model = self.ai.model.clone().ok_or("Choose a model from the provider's list.")?;
                let (label, id, preset_url) = PRESETS[self.ai.preset];
                let (id, name) = if preset_url == url { (id.to_string(), label.to_string()) } else { ("custom".to_string(), "Custom endpoint".to_string()) };
                Ok(Some((
                    Provider {
                        id: id.clone(),
                        name,
                        kind: ProviderKind::OpenAiCompatible {
                            base_url: url,
                            model,
                            api_key_account: (!key.is_empty()).then_some(id),
                            zero_data_retention: false,
                        },
                    },
                    (!key.is_empty()).then_some(key),
                )))
            }
            AiChoice::Claude => {
                let path = m.detected.claude_cli.clone().ok_or("Sayso did not find the Claude CLI. Install it, or choose another option.")?;
                Ok(Some((
                    Provider {
                        id: "claude-cli".into(),
                        name: "Claude CLI".into(),
                        kind: ProviderKind::ClaudeCli { path: Some(path.display().to_string()), model: "haiku".into() },
                    },
                    None,
                )))
            }
        }
    }

    /// Save the choice. Returns false when the form needs a fix.
    pub(super) fn save_ai(&mut self, cx: &mut Context<Self>) -> bool {
        match self.ai_provider(cx) {
            Err(e) => {
                self.ai.error = Some(e);
                false
            }
            Ok(None) => {
                self.ai.error = None;
                if self.model.read(cx).config.ai.enabled {
                    self.model.update(cx, |m, cx| m.set_ai_enabled(false, cx));
                }
                true
            }
            Ok(Some((provider, key))) => {
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

    fn test_ai(&mut self, cx: &mut Context<Self>) {
        let provider = match self.ai_provider(cx) {
            Ok(Some((p, key))) => (p, key),
            Ok(None) => return,
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
        cx.notify();
    }

    pub(super) fn ai_step(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let choice = self.ai_choice(cx);
        let d = self.model.read(cx).detected.clone();
        let home = sayso_core::paths::PathEnv::home(&sayso_core::paths::SystemEnv);

        let mut rows = div().flex().flex_col().gap(px(8.));
        if !d.done {
            rows = rows.child(text::ui(crate::shell::os_text!("Looking for AI tools on this Mac…", "Looking for AI tools on this computer…"), 13., FontWeight::NORMAL, c.graphite).px(px(4.)));
        }
        if let Some(models) = d.ollama.clone().filter(|l| !l.is_empty()) {
            let current = self.ai.local_model.clone().unwrap_or_else(|| models[0].clone());
            let open = self.ai.local_open;
            let picker = div()
                .id("ollama-model")
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(30.))
                .px(px(10.))
                .rounded(px(8.))
                .bg(c.deboss)
                .shadow(vec![BoxShadow::new(px(0.), px(1.), c.shadow(0.2)).blur_radius(px(2.)).inset()])
                .cursor_pointer()
                .child(text::ui(current.clone(), 13., FontWeight::NORMAL, c.ink))
                .child(icon(Icon::ChevronDown, 10., c.graphite))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.ai.choice = Some(AiChoice::Local);
                    this.ai.local_open = !this.ai.local_open;
                    cx.notify();
                }));
            let mut extra = None;
            if open && choice == AiChoice::Local {
                let mut list = div().flex().flex_col().p(px(4.)).rounded(px(10.)).debossed(&c);
                for (i, name) in models.into_iter().enumerate() {
                    let selected = name == current;
                    let n = name.clone();
                    list = list.child(
                        div()
                            .id(("ollama-pick", i))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(30.))
                            .px(px(10.))
                            .rounded(px(7.))
                            .cursor_pointer()
                            .hover(|s| s.bg(c.sheet_raised))
                            .child(div().w(px(14.)).when(selected, |d| d.child(icon(Icon::Check, 12., c.ink))))
                            .child(text::ui(name, 13., if selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }, c.ink))
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
                "Local model",
                crate::shell::os_text!("Ollama is running on this Mac. Text stays on this Mac.", "Ollama is running on this computer. Text stays on this computer.").into(),
                Some(picker.into_any_element()),
                extra,
                &c,
                cx,
            ));
        }
        let cloud_extra = (choice == AiChoice::Cloud).then(|| self.cloud_form(&c, cx));
        rows = rows.child(self.ai_row(
            AiChoice::Cloud,
            choice,
            "Cloud API",
            crate::shell::os_text!("OpenRouter, OpenAI, or any OpenAI-compatible URL. Your key is stored in the Keychain.", "OpenRouter, OpenAI, or any OpenAI-compatible URL. Your key is stored in the system keyring.").into(),
            Some(Badge::new("Cloud", BadgeTone::Accent).into_any_element()),
            cloud_extra,
            &c,
            cx,
        ));
        if let Some(path) = &d.claude_cli {
            let shown = sayso_core::paths::Paths::display(path, &home);
            rows = rows.child(self.ai_row(
                AiChoice::Claude,
                choice,
                "Claude CLI",
                format!("Found at {shown}. Uses your Claude login. About 1.8 s slower."),
                Some(Badge::new("Cloud", BadgeTone::Accent).into_any_element()),
                None,
                &c,
                cx,
            ));
        }
        rows = rows.child(self.ai_row(AiChoice::NotNow, choice, "Not now", "You can set this up later in Styles.".into(), None, None, &c, cx));

        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(22.))
            .pt(px(28.))
            .px(px(72.))
            .pb(px(24.))
            .child(heading(
                "Add AI clean-up",
                "Optional. A style can remove filler words, fix grammar, or format an email. Without it, Sayso inserts exactly what you said.",
                &c,
            ))
            .child(rows);
        if let Some(e) = self.ai.error.clone().filter(|_| choice != AiChoice::Cloud) {
            col = col.child(crate::hub::settings::kit::notice(BannerKind::Warning, e, cx));
        }
        col.into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn ai_row(
        &self,
        me: AiChoice,
        choice: AiChoice,
        title: &'static str,
        desc: String,
        trailing: Option<AnyElement>,
        extra: Option<AnyElement>,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = me == choice;
        let plain = me == AiChoice::NotNow && !selected;
        let mut card = div()
            .id(SharedString::from(format!("ai-{me:?}")))
            .flex()
            .flex_col()
            .gap(px(14.))
            .py(px(14.))
            .px(px(16.))
            .rounded(px(12.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.select_ai(me, cx)));
        card = if selected {
            let mut s = vec![BoxShadow::new(px(0.), px(0.), c.ink).spread_radius(px(2.))];
            s.push(BoxShadow::new(px(0.), px(1.), c.shadow(0.14)).blur_radius(px(2.)));
            card.bg(c.sheet_raised).shadow(s)
        } else if plain {
            card.hover(|s| s.bg(c.deboss.opacity(0.35)))
        } else {
            card.bg(c.sheet_raised).shadow(paper::raised_small(c)).hover(|s| s.bg(c.sheet))
        };
        card.child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .child(radio_dot(selected, cx))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(text::ui(title, 15., FontWeight::SEMIBOLD, c.ink).line_height(px(18.)))
                        .child(text::ui(desc, 13., FontWeight::NORMAL, c.graphite).line_height(px(16.))),
                )
                .when_some(trailing, |d, t| d.child(t)),
        )
        .when_some(extra, |d, e| d.child(div().pl(px(32.)).child(e)))
        .into_any_element()
    }

    fn cloud_form(&mut self, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let mut presets = div().flex().gap(px(6.));
        for (i, (label, _, url)) in PRESETS.into_iter().enumerate() {
            presets = presets.child(Chip::new(("preset", i), label, self.ai.preset == i).on_click(cx.listener(move |this, _, window, cx| {
                this.ai.preset = i;
                this.ai.test = None;
                this.ai.model = None;
                this.ai.base_url.update(cx, |s, cx| s.set_value(url, window, cx));
                cx.notify();
            })));
        }
        let field = |label: &'static str, state: &Entity<InputState>, mono: bool| {
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
                        .child(Input::new(state).appearance(false).w_full()),
                )
        };
        let test_line: AnyElement = if self.ai.testing {
            text::ui("Testing…", 13., FontWeight::NORMAL, c.graphite).into_any_element()
        } else {
            match &self.ai.test {
                Some(Ok(ms)) => super::check_line(format!("Works. The answer came in {:.1} s.", *ms as f64 / 1000.0), c).into_any_element(),
                Some(Err(e)) => text::ui(format!("The test failed: {e}"), 13., FontWeight::NORMAL, c.danger).into_any_element(),
                None => div().into_any_element(),
            }
        };
        let error = self.ai.error.clone().map(|e| crate::hub::settings::kit::notice(BannerKind::Warning, e, cx));
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            // Clicks in the form must not re-select the row.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(presets)
            .child(field("Base URL", &self.ai.base_url, true))
            .child(field("API key", &self.ai.key, true))
            .child(self.cloud_model(c, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .pl(px(82.))
                    .child(Button::new("test-ai", "Test connection").small().disabled(self.ai.testing).on_click(cx.listener(|this, _, _, cx| this.test_ai(cx))))
                    .child(test_line),
            )
            .when_some(error, |d, e| d.child(e))
            .into_any_element()
    }
}
