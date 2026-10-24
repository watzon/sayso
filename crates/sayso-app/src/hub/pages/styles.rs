//! The styles page. See the Paper page "Styles · Compact".
//!
//! A panel that shows the featured style with an example, one row for each
//! style (click to make it the default, Edit to change it), the style editor
//! panel, and a line that names the default provider. The providers are in
//! Settings.

use super::ext::{self, StyleRoute};
use super::kit::{self, ui};
use crate::model::AppModel;
use crate::model_picker::{self, Extra};
use gpui_kit::component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::cleanup::{CleanupModelInfo, CleanupSettings, Format, Layout, Settings, Tone};
use sayso_core::config::{Provider, ProviderKind};
use sayso_core::ink::{NamedInk, NamedInkLabel as _};
use sayso_core::style::{CleanupField, RunPlan, Style, StyleOrigin};
use sayso_core::style_rules::{self, TypedTarget};
use sayso_i18n::t;
use sayso_platform::AppInfo;
use sayso_ui::a11y::{A11yExt as _, Pressable, Role, Toggled};
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::fonts::{DISPLAY, Face, LocaleFonts as _, UI};
use sayso_ui::{ActivePaper, Colors, text};

/// The style that the page shows with an example.
const FEATURED_STYLE_ID: &str = "rambling";

/// The styles to feature when the default provider cannot run the usual one,
/// first choice first.
const FEATURED_FALLBACK: [&str; 6] = ["rambling", "email", "notes", "message", "polished", "clean"];

/// The style that the page shows with an example. It is a style that the
/// default provider can run: a cleanup model reads no prompt, so it cannot run
/// every style. With any other default provider this is the usual style.
fn featured_style<'a>(styles: &'a [Style], default: Option<&Provider>) -> Option<&'a Style> {
    let usual = styles.iter().find(|s| s.id == FEATURED_STYLE_ID);
    let Some(default) = default.filter(|p| matches!(p.kind, ProviderKind::CleanupModel { .. })) else { return usual };
    let runs = |s: &Style| matches!(s.run_plan(default), RunPlan::Prompt | RunPlan::Cleanup(_));
    if usual.is_some_and(runs) {
        return usual;
    }
    FEATURED_FALLBACK.iter().filter_map(|id| styles.iter().find(|s| s.id == *id)).find(|s| runs(s)).or(usual)
}

/// What the cleanup controls of the editor say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CleanupEdit {
    settings: Settings,
    /// True when these settings replace what the style has: the user changed
    /// a control, or the style is new.
    own: bool,
}

impl CleanupEdit {
    fn new(style: &Style, is_new: bool) -> Self {
        CleanupEdit { settings: style.cleanup_settings().and_then(|s| s.known()).unwrap_or_default(), own: is_new }
    }

    fn change(&mut self, f: impl FnOnce(&mut Settings)) {
        f(&mut self.settings);
        self.own = true;
    }

    fn table(&self) -> CleanupField {
        CleanupField::Settings(CleanupSettings::new(self.settings.tone, self.settings.layout, self.settings.format))
    }
}

/// The style as the editor has it now, to ask `Style::run_plan` what a provider
/// does with it.
fn style_as_edited(base: &Style, prompt: &str, edit: &CleanupEdit) -> Style {
    Style { prompt: prompt.trim().to_string(), cleanup: if edit.own { edit.table() } else { base.cleanup.clone() }, ..base.clone() }
}

/// The `cleanup` key to save for a style whose provider is a cleanup model.
/// None keeps the key as it is: a style with no cleanup settings that the user
/// did not give any, and a style with a value that this version does not know.
fn cleanup_to_save(base: &Style, edit: &CleanupEdit) -> Option<CleanupField> {
    let known = base.cleanup_settings().is_some_and(|s| s.known().is_some());
    (edit.own || known).then(|| edit.table())
}

/// The cleanup model that cannot run `style`, when `provider` is one.
fn cannot_run(style: &Style, provider: Option<&Provider>) -> Option<&'static CleanupModelInfo> {
    let provider = provider?;
    let info = ext::cleanup_model_info(provider)?;
    (style.run_plan(provider) == RunPlan::CleanupOnly).then_some(info)
}

/// Why the editor cannot save. It keeps the reason, not the sentence, so it
/// changes with the language.
enum EditError {
    NeedsName,
    BadTimeout,
    SaveFailed(String),
    BaseSaveFailed(String),
}

impl EditError {
    fn text(&self) -> String {
        match self {
            EditError::NeedsName => t!("styles.error.needs_name"),
            EditError::BadTimeout => t!("styles.error.bad_timeout"),
            EditError::SaveFailed(e) => t!("styles.error.save_failed", error = e),
            EditError::BaseSaveFailed(e) => t!("styles.error.base_save_failed", error = e),
        }
    }
}

/// The placeholder of the "add" field. Only some systems read the address of a page.
fn target_placeholder(sites_ok: bool) -> String {
    match (sites_ok, cfg!(target_os = "macos")) {
        (true, true) => t!("styles.targets.placeholder_site_bundle"),
        (true, false) => t!("styles.targets.placeholder_site_app"),
        (false, true) => t!("styles.targets.placeholder_bundle"),
        (false, false) => t!("styles.targets.placeholder_app"),
    }
}

struct Editor {
    id: String,
    is_new: bool,
    origin: Option<StyleOrigin>,
    base: Style,
    name: Entity<InputState>,
    description: Entity<InputState>,
    prompt: Entity<TextareaState>,
    ink: String,
    provider: Option<String>,
    provider_menu: bool,
    /// The style's model. None uses the provider's model.
    model: Option<String>,
    timeout: Entity<InputState>,
    /// False for a standalone style.
    use_base: bool,
    /// The controls of a cleanup model. Used only when the provider is one.
    cleanup: CleanupEdit,
    /// With a cleanup model, the prompt and the apps are folded away.
    prompt_open: bool,
    targets_open: bool,
    error: Option<EditError>,
    confirm_delete: bool,
    /// The apps and sites that use this style (see `style_rules`).
    apps: Vec<String>,
    sites: Vec<String>,
    /// The text of the "add" field.
    target: Entity<InputState>,
    /// The "add" field shows all apps, also with no text.
    target_open: bool,
    _target_sub: Subscription,
}

/// A row of the list under the "add" field.
#[derive(Clone)]
enum Choice {
    /// An app on this computer.
    App(AppInfo),
    /// A typed app id that no app on this computer has.
    AppId(String),
    Site(String),
}

/// The most rows that the list under the "add" field draws.
const MAX_CHOICES: usize = 40;

/// The editor of the base prompt.
struct BaseEditor {
    prompt: Entity<TextareaState>,
    error: Option<EditError>,
}

pub struct StylesPage {
    model: Entity<AppModel>,
    hovered: Option<String>,
    editor: Option<Editor>,
    base_editor: Option<BaseEditor>,
    /// The apps on this computer, for the names and the picker of the style rules.
    apps: Vec<AppInfo>,
    /// The locale of the text in the placeholders of the editors.
    locale: String,
    _subs: Vec<Subscription>,
}

impl StylesPage {
    pub fn new(model: Entity<AppModel>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        // The list of apps reads many folders, so it loads off the main thread.
        let context = model.read(cx).services.platform.context.clone();
        cx.spawn(async move |this, cx| {
            let mut apps = cx.background_spawn(async move { context.installed_apps() }).await;
            apps.retain(|a| a.bundle_id != "dev.sayso.Sayso" && !a.bundle_id.is_empty() && !a.name.is_empty());
            let _ = this.update(cx, |this, cx| {
                this.apps = apps;
                cx.notify();
            });
        })
        .detach();
        Self {
            model,
            hovered: None,
            editor: None,
            base_editor: None,
            apps: Vec::new(),
            locale: sayso_i18n::locale(),
            _subs: vec![observe],
        }
    }

    // -----------------------------------------------------------------------
    // Editor
    // -----------------------------------------------------------------------

    fn open_editor(&mut self, style: Option<Style>, window: &mut Window, cx: &mut Context<Self>) {
        let is_new = style.is_none();
        let base = style.unwrap_or_else(|| Style {
            id: String::new(),
            name: String::new(),
            ink: format!("#{:06X}", NamedInk::Indigo.color().0),
            description: String::new(),
            prompt: String::new(),
            provider: None,
            model: None,
            temperature: None,
            timeout_ms: None,
            standalone: false,
            cleanup: Default::default(),
            inherited_cleanup: None,
        });
        let origin = if is_new { None } else { self.model.read(cx).style_origin(&base.id) };
        let http_s = self.model.read(cx).config.ai.http_timeout_ms as f64 / 1000.0;
        let b = base.clone();
        // A built-in style shows its name and description in the interface language.
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(t!("styles.editor.name_placeholder")).default_value(b.display_name()));
        let description = cx.new(|cx| InputState::new(window, cx).placeholder(t!("styles.editor.description_placeholder")).default_value(b.display_description()));
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(t!("styles.editor.prompt_placeholder"))
                .default_value(b.prompt.clone())
        });
        let timeout = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("styles.editor.timeout_placeholder", seconds = trim_float(http_s)))
                .default_value(b.timeout_ms.map(|ms| trim_float(ms as f64 / 1000.0)).unwrap_or_default())
        });
        name.update(cx, |s, cx| s.focus(window, cx));
        let (apps, sites, sites_ok) = {
            let m = self.model.read(cx);
            let rule = m.config.ai.style_rules.rule(&base.id).cloned().unwrap_or_default();
            (rule.apps, rule.sites, m.services.platform.context.reads_page_url())
        };
        let target = cx.new(|cx| {
            InputState::new(window, cx).placeholder(target_placeholder(sites_ok))
        });
        let _target_sub = cx.subscribe_in(&target, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. })
                && let Some(first) = this.target_choices(cx).into_iter().next()
            {
                this.add_target(first, window, cx);
            }
            cx.notify();
        });
        let style_model = base.model.clone();
        let use_base = !base.standalone;
        let cleanup = CleanupEdit::new(&base, is_new);
        self.editor = Some(Editor {
            id: base.id.clone(),
            is_new,
            origin,
            ink: base.ink.clone(),
            provider: base.provider.clone(),
            base,
            name,
            description,
            prompt,
            provider_menu: false,
            model: style_model,
            timeout,
            use_base,
            cleanup,
            prompt_open: false,
            targets_open: false,
            error: None,
            confirm_delete: false,
            apps,
            sites,
            target,
            target_open: false,
            _target_sub,
        });
        cx.notify();
    }

    /// The name of an app for the user: its name on this computer, else the last part of its id.
    fn app_name(&self, id: &str, cx: &App) -> String {
        match self.apps.iter().find(|a| a.bundle_id.eq_ignore_ascii_case(id)) {
            Some(app) => app.name.clone(),
            None => self.model.read(cx).app_name_for(id, &[]),
        }
    }

    /// The rows under the "add" field: the apps that fit the text, then what
    /// the text itself can be. With no text, all apps when the list is open.
    fn target_choices(&self, cx: &App) -> Vec<Choice> {
        let Some(ed) = self.editor.as_ref() else { return Vec::new() };
        let text = ed.target.read(cx).value().trim().to_string();
        let query = text.to_lowercase();
        if query.is_empty() && !ed.target_open {
            return Vec::new();
        }
        let added = |id: &str| ed.apps.iter().any(|a| a.eq_ignore_ascii_case(id));
        let mut apps: Vec<AppInfo> = self
            .apps
            .iter()
            .filter(|a| !added(&a.bundle_id))
            .filter(|a| a.name.to_lowercase().contains(&query) || a.bundle_id.to_lowercase().contains(&query))
            .cloned()
            .collect();
        // An app whose name fits comes before an app where only the id fits.
        apps.sort_by_key(|a| !a.name.to_lowercase().contains(&query));
        let mut choices: Vec<Choice> = apps.into_iter().map(Choice::App).collect();
        let sites_ok = self.model.read(cx).services.platform.context.reads_page_url();
        // The text as the user typed it: an app id and a path can have capital letters.
        for typed in style_rules::typed_targets(&text) {
            match typed {
                // A bundle id has a dot. A text with no dot is the start of a name.
                TypedTarget::App(id) if cfg!(target_os = "macos") && !id.contains('.') => {}
                TypedTarget::App(id) => {
                    if !added(&id) && !self.apps.iter().any(|a| a.bundle_id.eq_ignore_ascii_case(&id)) {
                        choices.push(Choice::AppId(id));
                    }
                }
                TypedTarget::Site(site) if sites_ok && !ed.sites.contains(&site) => choices.push(Choice::Site(site)),
                TypedTarget::Site(_) => {}
            }
        }
        choices
    }

    fn add_target(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.editor.as_mut() else { return };
        match choice {
            Choice::App(app) => ed.apps.push(app.bundle_id),
            Choice::AppId(id) => ed.apps.push(id),
            Choice::Site(site) => ed.sites.push(site),
        }
        ed.target_open = false;
        ed.target.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.editor.as_mut() else { return };
        let name = ed.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            ed.error = Some(EditError::NeedsName);
            cx.notify();
            return;
        }
        let timeout = ed.timeout.read(cx).value().trim().to_string();
        let timeout_ms = if timeout.is_empty() {
            None
        } else {
            match timeout.trim_end_matches('s').trim().parse::<f64>() {
                Ok(s) if s > 0.0 && s <= 120.0 => Some((s * 1000.0).round() as u64),
                _ => {
                    ed.error = Some(EditError::BadTimeout);
                    cx.notify();
                    return;
                }
            }
        };
        let id = if ed.is_new { self.model.read(cx).new_style_id(&name) } else { ed.id.clone() };
        // A name or description that the user did not change keeps the text
        // from before, so a built-in style still follows the interface language.
        let description = ed.description.read(cx).value().trim().to_string();
        let name = if name == ed.base.display_name() { ed.base.name.clone() } else { name };
        let description = if description == ed.base.display_description() { ed.base.description.clone() } else { description };
        // The settings go into the file only for a cleanup model. A file for
        // another provider stays as it was.
        let on_cleanup_model = self.model.read(cx).config.ai.provider(ed.provider.as_deref()).is_some_and(|p| ext::cleanup_model_info(p).is_some());
        let cleanup = on_cleanup_model.then(|| cleanup_to_save(&ed.base, &ed.cleanup)).flatten().unwrap_or_else(|| ed.base.cleanup.clone());
        let style = Style {
            id,
            name,
            ink: ed.ink.clone(),
            description,
            prompt: ed.prompt.read(cx).value().trim().to_string(),
            provider: ed.provider.clone(),
            model: ed.model.clone(),
            timeout_ms,
            standalone: !ed.use_base,
            cleanup,
            ..ed.base.clone()
        };
        let (apps, sites) = (ed.apps.clone(), ed.sites.clone());
        let saved = self.model.update(cx, |m, cx| {
            m.save_style(&style, cx)?;
            // The rules are in the config file, not in the style file.
            let rule = m.config.ai.style_rules.rule(&style.id);
            if rule.map(|r| (&r.apps, &r.sites)) != Some((&apps, &sites)) && (rule.is_some() || !apps.is_empty() || !sites.is_empty()) {
                m.edit_config(cx, |c| c.ai.style_rules.set_targets(&style.id, apps, sites));
            }
            Ok::<_, String>(())
        });
        match saved {
            Ok(()) => self.editor = None,
            Err(e) => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.error = Some(EditError::SaveFailed(e));
                }
            }
        }
        cx.notify();
    }

    fn editor_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = cx.paper().colors;
        let ed = self.editor.as_ref()?;
        let m = self.model.read(cx);
        let providers: Vec<(String, String)> = m.config.ai.providers.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
        let default_name = m.config.ai.provider(None).map(|p| p.name.clone());
        let styles_dir = m.styles_dir_display();
        // The provider the style runs on, for its model list.
        let target = m.config.ai.provider(ed.provider.as_deref()).cloned();
        let model_label = match (&ed.model, &target) {
            (Some(id), Some(p)) => m.model_label(&p.id, id),
            (Some(id), None) => id.clone(),
            (None, Some(p)) => match p.model() {
                Some(id) => t!("styles.editor.provider_model_with", model = m.model_label(&p.id, id)),
                None => t!("styles.editor.provider_model"),
            },
            (None, None) => t!("styles.editor.provider_model"),
        };
        let all_providers: Vec<Provider> = m.config.ai.providers.clone();
        let default_provider = m.config.ai.provider(None).cloned();
        // The controls and the menu follow what the provider does with the style as it is now.
        let cleanup_model = target.as_ref().and_then(ext::cleanup_model_info);
        let edited = style_as_edited(&ed.base, &ed.prompt.read(cx).value(), &ed.cleanup);
        let blocked = cannot_run(&edited, target.as_ref());

        // Ink swatches.
        let ink_count = NamedInk::ALL.len();
        let ink_current = NamedInk::ALL.iter().position(|n| ed.ink.eq_ignore_ascii_case(&format!("#{:06X}", n.color().0)));
        // A group of radio buttons: the arrows pick the next ink and move the focus.
        let mut radios = div()
            .id("ink-swatches")
            .role(Role::RadioGroup)
            .aria_label(t!("styles.editor.ink"))
            .flex()
            .items_center()
            .gap(px(10.))
            .when_some(ink_current, |d, current| {
                d.on_key_down(cx.listener(move |this, ev: &KeyDownEvent, w, cx| {
                    let next = match ev.keystroke.key.as_str() {
                        "right" | "down" if current + 1 < ink_count => current + 1,
                        "left" | "up" if current > 0 => current - 1,
                        _ => return,
                    };
                    cx.stop_propagation();
                    if let Some(ed) = this.editor.as_mut() {
                        ed.ink = format!("#{:06X}", NamedInk::ALL[next].color().0);
                    }
                    if next > current { w.focus_next(cx) } else { w.focus_prev(cx) }
                    cx.notify();
                }))
            });
        for (i, ink) in NamedInk::ALL.into_iter().enumerate() {
            let hex = format!("#{:06X}", ink.color().0);
            let selected = ed.ink.eq_ignore_ascii_case(&hex);
            let h2 = hex.clone();
            radios = radios.child(
                Pressable::new(SharedString::from(format!("ink-{}", ink.name())))
                    .a11y_radio(ink.label(), selected, i, ink_count)
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(30.))
                    .rounded_full()
                    .cursor_pointer()
                    .border_2()
                    .border_color(if selected { c.ink } else { transparent_black() })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(ed) = this.editor.as_mut() {
                            ed.ink = h2.clone();
                        }
                        cx.notify();
                    }))
                    .child(kit::seal_dot(Some(&hex), 22., cx)),
            );
        }
        let ink_name = NamedInk::ALL
            .iter()
            .find(|n| ed.ink.eq_ignore_ascii_case(&format!("#{:06X}", n.color().0)))
            .map(|n| n.label())
            .unwrap_or_else(|| t!("styles.editor.ink_custom", ink = ed.ink));
        let swatches = div().flex().items_center().gap(px(10.)).child(radios).child(ui(ink_name, 13., 16., FontWeight::MEDIUM, c.graphite).pl(px(4.)));

        // Provider select.
        let current = match &ed.provider {
            Some(id) => providers.iter().find(|p| &p.0 == id).map(|p| p.1.clone()).unwrap_or_else(|| t!("styles.editor.provider_not_set_up", provider = id)),
            None => match &default_name {
                Some(n) => t!("styles.editor.default_provider_named", provider = n),
                None => t!("styles.editor.default_provider"),
            },
        };
        let mut select = div().relative().child(
            Pressable::new("provider-select")
                .a11y_role(Role::ComboBox)
                .label(t!("styles.editor.provider"))
                .aria_value(current.clone())
                .aria_expanded(ed.provider_menu)
                .flex()
                .items_center()
                .justify_between()
                .h(px(36.))
                .px(px(12.))
                .rounded(px(9.))
                .raised_small(&c)
                .when(blocked.is_some(), |d| d.border_1().border_color(c.danger))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(ed) = this.editor.as_mut() {
                        ed.provider_menu = !ed.provider_menu;
                    }
                    cx.notify();
                }))
                .child(kit::plain(current.clone(), 14., 18., FontWeight::NORMAL, c.ink))
                .child(icon(Icon::ChevronDown, 12., c.graphite)),
        );
        if ed.provider_menu {
            // A cleanup model that cannot run the style is grey. When the
            // current choice is one, the first line says why.
            let default_blocked = cannot_run(&edited, default_provider.as_ref()).is_some();
            let mut rows = Vec::new();
            let note = blocked.map(|info| (current.clone(), t!("styles.editor.cannot_run", model = info.name)));
            // With the default provider as the current choice, the first line stands for its row.
            if !(ed.provider.is_none() && note.is_some()) {
                rows.push(ProviderRow { pick: None, label: t!("styles.editor.default_provider"), selected: ed.provider.is_none(), blocked: default_blocked });
            }
            for p in &all_providers {
                rows.push(ProviderRow {
                    pick: Some(p.id.clone()),
                    label: p.name.clone(),
                    selected: ed.provider.as_deref() == Some(p.id.as_str()),
                    blocked: cannot_run(&edited, Some(p)).is_some(),
                });
            }
            let picks: Vec<Option<String>> = rows.iter().map(|r| r.pick.clone()).collect();
            let e1 = cx.entity();
            let e2 = cx.entity();
            select = select.child(div().absolute().top_full().left_0().child(provider_menu(
                note,
                rows,
                move |i, _, cx| {
                    let Some(pick) = picks.get(i).cloned() else { return };
                    e1.update(cx, |this, cx| {
                        if let Some(ed) = this.editor.as_mut() {
                            // A model from one provider means nothing to another.
                            if ed.provider != pick {
                                ed.model = None;
                            }
                            ed.provider = pick;
                            ed.provider_menu = false;
                        }
                        cx.notify();
                    });
                },
                move |_, cx| {
                    e2.update(cx, |this, cx| {
                        if let Some(ed) = this.editor.as_mut() {
                            ed.provider_menu = false;
                        }
                        cx.notify();
                    })
                },
                cx,
            )));
        }
        let provider_field = if providers.is_empty() {
            ui(t!("styles.editor.no_providers"), 13., 18., FontWeight::NORMAL, c.graphite).into_any_element()
        } else {
            select.into_any_element()
        };

        let model_field = match target {
            None => ui(t!("styles.editor.add_provider_for_model"), 13., 18., FontWeight::NORMAL, c.graphite).into_any_element(),
            Some(p) => {
                let pid = p.id.clone();
                Pressable::new("style-model")
                    .a11y_role(Role::ComboBox)
                    .label(t!("styles.editor.model"))
                    .aria_value(model_label.clone())
                    .relative()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(36.))
                    .px(px(12.))
                    .rounded(px(9.))
                    .raised_small(&c)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let selected = this.editor.as_ref().and_then(|e| e.model.clone());
                        let page = cx.entity().downgrade();
                        let model = this.model.clone();
                        let refresh_id = pid.clone();
                        this.model.update(cx, |m, cx| m.load_provider_models(&pid, false, cx));
                        let request = model_picker::Request {
                            key: "style-model".into(),
                            list_key: pid.clone(),
                            extra: Some(Extra { label: t!("styles.editor.provider_model").into(), selected: selected.is_none() }),
                            thinking_for: None,
                            selected,
                            on_pick: Box::new(move |pick, cx| {
                                let _ = page.update(cx, |this, cx| {
                                    if let Some(ed) = this.editor.as_mut() {
                                        ed.model = pick;
                                    }
                                    cx.notify();
                                });
                            }),
                            on_refresh: Box::new(move |cx| model.update(cx, |m, cx| m.load_provider_models(&refresh_id, true, cx))),
                        };
                        model_picker::open(&this.model, request, cx);
                    }))
                    .child(kit::plain(model_label, 14., 18., FontWeight::NORMAL, c.ink).truncate())
                    .child(icon(Icon::ChevronDown, 12., c.graphite))
                    .child(model_picker::anchor("style-model"))
                    .into_any_element()
            }
        };

        let prompt = div()
            .p(px(4.))
            .rounded(px(10.))
            .debossed(&c)
            .text_size(px(14.))
            .child(Textarea::new(&ed.prompt).aria_label(t!("styles.editor.prompt")).appearance(false).h(px(150.)).text_size(px(14.)));

        let title = if ed.is_new { t!("styles.editor.new_title") } else { t!("styles.editor.edit_title", name = ed.base.name) };
        let file_note = if ed.is_new {
            t!("styles.editor.saved_in", dir = styles_dir)
        } else {
            match ed.origin {
                Some(StyleOrigin::BuiltIn) => t!("styles.editor.built_in_note"),
                Some(StyleOrigin::Overridden) => t!("styles.editor.overridden_note"),
                _ => format!("{}/{}.toml", styles_dir, ed.id),
            }
        };

        let mut buttons = div().flex().items_center().gap(px(8.)).child(
            Button::new("save-style", t!("styles.save")).primary().on_click(cx.listener(|this, _, _, cx| this.save_editor(cx))),
        );
        buttons = buttons.child(Button::new("cancel-style", t!("styles.cancel")).ghost().on_click(cx.listener(|this, _, _, cx| {
            this.editor = None;
            cx.notify();
        })));
        buttons = buttons.child(div().flex_1());
        if ed.origin == Some(StyleOrigin::Overridden) {
            let id = ed.id.clone();
            buttons = buttons.child(Button::new("reset-style", t!("styles.editor.reset_built_in")).icon(Icon::Undo).on_click(cx.listener(
                move |this, _, _, cx| {
                    let id = id.clone();
                    this.model.update(cx, |m, cx| m.reset_style(&id, cx));
                    this.editor = None;
                    cx.notify();
                },
            )));
        }
        if ed.origin == Some(StyleOrigin::User) {
            let id = ed.id.clone();
            if ed.confirm_delete {
                buttons = buttons.child(Button::new("delete-style-yes", t!("styles.editor.delete_style")).danger().on_click(cx.listener(move |this, _, _, cx| {
                    let id = id.clone();
                    this.model.update(cx, |m, cx| m.delete_style(&id, cx));
                    this.editor = None;
                    cx.notify();
                })));
            } else {
                buttons = buttons.child(Button::new("delete-style", t!("styles.editor.delete")).danger().icon(Icon::Trash).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(ed) = this.editor.as_mut() {
                        ed.confirm_delete = true;
                    }
                    cx.notify();
                })));
            }
        }

        let prompt_field = kit::field(&t!("styles.editor.prompt"), prompt, Some(&t!("styles.editor.prompt_hint")), cx);
        let use_base_row = div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(ui(t!("styles.editor.use_base"), 13., 16., FontWeight::SEMIBOLD, c.ink))
                            .child(ui(
                                t!("styles.editor.use_base_detail"),
                                12.,
                                16.,
                                FontWeight::NORMAL,
                                c.graphite,
                            )),
                    )
                    .child(div().flex_none().child(Switch::new("use-base", ed.use_base).label(t!("styles.editor.use_base")).on_toggle({
                        let page = cx.entity();
                        move |on, _, cx| {
                            page.update(cx, |this, cx| {
                                if let Some(ed) = this.editor.as_mut() {
                                    ed.use_base = on;
                                }
                                cx.notify();
                            })
                        }
                    })));
        let cleanup_hint = cleanup_model.map(|info| if info.context { t!("styles.editor.cleanup_hint_context") } else { t!("styles.editor.cleanup_hint") });
        let body = div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .p(px(28.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .child(div().flex().flex_col().gap(px(6.)).child(text::title(title, 28., &c)).child(ui(
                        file_note,
                        13.,
                        18.,
                        FontWeight::NORMAL,
                        c.graphite,
                    )))
                    .child(kit::icon_button("close-editor", Icon::Close, 12., cx).label(t!("styles.a11y.close")).on_click(cx.listener(|this, _, _, cx| {
                        this.editor = None;
                        cx.notify();
                    }))),
            )
            .child(kit::field(&t!("styles.editor.name"), kit::input_well_named(&ed.name, Some(t!("styles.editor.name").into()), cx), None, cx))
            .child(kit::field(&t!("styles.editor.description"), kit::input_well_named(&ed.description, Some(t!("styles.editor.description").into()), cx), None, cx))
            .child(kit::field(&t!("styles.editor.ink"), swatches, None, cx))
            .child(kit::field(&t!("styles.editor.provider"), provider_field, cleanup_hint.as_deref(), cx));
        let body = match cleanup_model {
            Some(info) => {
                let page = cx.entity();
                let mut body = body;
                if info.tones {
                    let tone = [Tone::VeryCasual, Tone::Casual, Tone::Formal].iter().position(|t| *t == ed.cleanup.settings.tone).unwrap_or(2);
                    let tone_control = Segmented::new(
                        "cleanup-tone",
                        [t!("styles.editor.tone_very_casual"), t!("styles.editor.tone_casual"), t!("styles.editor.tone_formal")],
                        tone,
                    )
                    .label(t!("styles.editor.tone"))
                    .compact()
                    .on_select({
                        let page = page.clone();
                        move |i, _, cx| {
                            page.update(cx, |this, cx| {
                                if let Some(ed) = this.editor.as_mut() {
                                    let tone = [Tone::VeryCasual, Tone::Casual, Tone::Formal][i.min(2)];
                                    ed.cleanup.change(|s| s.tone = tone);
                                }
                                cx.notify();
                            })
                        }
                    });
                    body = body.child(kit::field(&t!("styles.editor.tone"), tone_control, None, cx));
                }
                if info.layouts.contains(&Layout::List) {
                    let layout = if ed.cleanup.settings.layout == Layout::List { 1 } else { 0 };
                    let layout_control = Segmented::new("cleanup-layout", [t!("styles.editor.layout_paragraphs"), t!("styles.editor.layout_lists")], layout)
                        .label(t!("styles.editor.layout"))
                        .compact()
                        .on_select({
                            let page = page.clone();
                            move |i, _, cx| {
                                page.update(cx, |this, cx| {
                                    if let Some(ed) = this.editor.as_mut() {
                                        let layout = if i == 1 { Layout::List } else { Layout::Paragraphs };
                                        ed.cleanup.change(|s| s.layout = layout);
                                    }
                                    cx.notify();
                                })
                            }
                        });
                    body = body.child(kit::field(&t!("styles.editor.layout"), layout_control, None, cx));
                }
                if info.formats.contains(&Format::Email) {
                    let page = page.clone();
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(16.))
                            .child(ui(t!("styles.editor.format_email"), 13., 16., FontWeight::SEMIBOLD, c.ink).flex_1().min_w_0())
                            .child(div().flex_none().child(Switch::new("format-email", ed.cleanup.settings.format == Format::Email).label(t!("styles.editor.format_email")).on_toggle(
                                move |on, _, cx| {
                                    page.update(cx, |this, cx| {
                                        if let Some(ed) = this.editor.as_mut() {
                                            ed.cleanup.change(|s| s.format = if on { Format::Email } else { Format::Plain });
                                        }
                                        cx.notify();
                                    })
                                },
                            ))),
                    );
                }
                let prompt_body = div().flex().flex_col().gap(px(18.)).child(prompt_field).child(use_base_row);
                let targets_summary = names_line(&self.target_names(&ed.apps, &ed.sites, cx)).unwrap_or_else(|| t!("styles.targets.summary_none"));
                body.child(fold_row(
                    "fold-prompt",
                    t!("styles.editor.prompt"),
                    t!("styles.editor.prompt_not_used", model = info.name),
                    ed.prompt_open,
                    {
                        let page = page.clone();
                        move |_, cx| {
                            page.update(cx, |this, cx| {
                                if let Some(ed) = this.editor.as_mut() {
                                    ed.prompt_open = !ed.prompt_open;
                                }
                                cx.notify();
                            })
                        }
                    },
                    prompt_body,
                    &c,
                ))
                .child(fold_row(
                    "fold-targets",
                    t!("styles.targets.title"),
                    targets_summary,
                    ed.targets_open,
                    move |_, cx| {
                        page.update(cx, |this, cx| {
                            if let Some(ed) = this.editor.as_mut() {
                                ed.targets_open = !ed.targets_open;
                            }
                            cx.notify();
                        })
                    },
                    self.targets_group(ed, false, cx),
                    &c,
                ))
                .child(kit::field(&t!("styles.editor.timeout"), kit::input_well_named(&ed.timeout, Some(t!("styles.editor.timeout").into()), cx), None, cx).w(px(150.)))
            }
            None => body
                .child(prompt_field)
                .child(use_base_row)
                .child(
                    div()
                        .flex()
                        .gap(px(12.))
                        .child(kit::field(&t!("styles.editor.model"), model_field, None, cx).flex_1())
                        .child(kit::field(&t!("styles.editor.timeout"), kit::input_well_named(&ed.timeout, Some(t!("styles.editor.timeout").into()), cx), None, cx).w(px(150.))),
                )
                .child(self.targets_group(ed, true, cx)),
        };
        let body = body
            .when_some(ed.error.as_ref(), |d, e| d.child(Banner::new(BannerKind::Warning, e.text())))
            .child(div().pt(px(6.)).child(buttons));

        let panel_title = if ed.is_new { t!("styles.editor.new_title") } else { t!("styles.editor.edit_title", name = ed.base.name) };
        Some(self.side_panel("style-editor", panel_title, body, cx))
    }

    /// The line on a style card that names the apps and sites of the style.
    /// None when the style rules are off or the style has none.
    fn target_line(&self, style: &str, cx: &App) -> Option<String> {
        let rules = &self.model.read(cx).config.ai.style_rules;
        let rule = rules.rule(style).filter(|_| rules.enabled)?;
        // The apps on this computer first: the user knows their names.
        names_line(&self.target_names(&rule.apps, &rule.sites, cx))
    }

    /// The names of apps and sites: the apps on this computer first, as the
    /// user knows their names.
    fn target_names(&self, apps: &[String], sites: &[String], cx: &App) -> Vec<String> {
        let (here, away): (Vec<&String>, Vec<&String>) = apps.iter().partition(|id| self.apps.iter().any(|a| a.bundle_id.eq_ignore_ascii_case(id)));
        here.into_iter().chain(away).map(|id| self.app_name(id, cx)).chain(sites.iter().cloned()).collect()
    }

    /// The "Use automatically" part of the style editor: the apps and sites
    /// of the style, and the field that adds one.
    ///
    /// With `framed` false the group has no title and no rule above it, for a
    /// fold row that has them.
    fn targets_group(&self, ed: &Editor, framed: bool, cx: &Context<Self>) -> Div {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let rules = &m.config.ai.style_rules;
        let sites_ok = m.services.platform.context.reads_page_url();
        // "Moves from Message": the app or site leaves that style on Save.
        let moves_from = |owner: Option<&str>| owner.filter(|id| *id != ed.id).map(|id| t!("styles.targets.moves_from", style = m.style_name(id)));
        let note = |s: String| ui(s, 12., 16., FontWeight::NORMAL, c.graphite).flex_none();

        let count = ed.apps.len() + ed.sites.len();
        let mut list = div()
            .id("target-list")
            .role(Role::List)
            .aria_label(t!("styles.targets.title"))
            .flex()
            .flex_col()
            .rounded(px(10.))
            .bg(c.sheet_raised)
            .raised_small(&c);
        // The row id comes from the app or site, so a removed row does not pass its id on.
        let row = |key: String, name: String, i: usize| {
            div()
                .id(SharedString::from(key))
                .role(Role::ListItem)
                .aria_label(name)
                .aria_position_in_set(i + 1)
                .aria_size_of_set(count)
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(40.))
                .px(px(10.))
                .when(i + 1 < count, |d| d.border_b_1().border_color(c.rule))
        };
        for (i, id) in ed.apps.iter().enumerate() {
            let known = self.apps.iter().find(|a| a.bundle_id.eq_ignore_ascii_case(id));
            let name = self.app_name(id, cx);
            let remove = id.clone();
            let remove_label = t!("styles.a11y.remove_app", name = name);
            list = list.child(
                row(format!("app-{id}"), name.clone(), i)
                    .child(AppBadge::new(name.clone()).icon(m.icons.get(id)).size(20.))
                    .when(known.is_some(), |d| d.child(ui(name, 14., 18., FontWeight::MEDIUM, c.ink).flex_none()))
                    .child(div().flex_1().min_w_0().child(text::mono(id.clone(), 11., if known.is_some() { c.graphite } else { c.ink }).truncate()))
                    // Before the list of apps loads, each app would get this note.
                    .when(known.is_none() && !self.apps.is_empty(), |d| d.child(note(t!("styles.targets.not_here"))))
                    .when_some(moves_from(rules.style_of_app(id)), |d, s| d.child(note(s)))
                    .child(kit::icon_button("remove-app", Icon::Close, 10., cx).label(remove_label).on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(ed) = this.editor.as_mut() {
                            ed.apps.retain(|a| *a != remove);
                        }
                        cx.notify();
                    }))),
            );
        }
        for (i, site) in ed.sites.iter().enumerate() {
            let remove = site.clone();
            list = list.child(
                row(format!("site-{site}"), site.clone(), ed.apps.len() + i)
                    .child(div().flex().flex_none().items_center().justify_center().size(px(20.)).child(icon(Icon::Globe, 16., c.graphite)))
                    .child(div().flex_1().min_w_0().child(text::mono(site.clone(), 12., c.ink).truncate()))
                    .when_some(moves_from(rules.style_of_site(site)), |d, s| d.child(note(s)))
                    .child(kit::icon_button("remove-site", Icon::Close, 10., cx).label(t!("styles.a11y.remove_site", name = site)).on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(ed) = this.editor.as_mut() {
                            ed.sites.retain(|s| *s != remove);
                        }
                        cx.notify();
                    }))),
            );
        }

        let open = kit::icon_button("target-open", Icon::ChevronDown, 12., cx).label(t!("styles.a11y.show_apps")).aria_expanded(ed.target_open).on_click(cx.listener(|this, _, _, cx| {
            if let Some(ed) = this.editor.as_mut() {
                ed.target_open = !ed.target_open;
            }
            cx.notify();
        }));
        let typed = !ed.target.read(cx).value().trim().is_empty();
        let choices = self.target_choices(cx);
        // An address is not the name of an app, so it gets no such line.
        let no_app = typed && !ed.target.read(cx).value().contains('/') && !choices.iter().any(|ch| matches!(ch, Choice::App(_)));
        let mut panel = div().id("target-choices").role(Role::ListBox).aria_label(t!("styles.a11y.choices")).flex().flex_col().gap(px(2.)).p(px(6.)).max_h(px(236.)).overflow_y_scroll().rounded(px(10.)).bg(c.sheet_raised).raised_small(&c);
        if no_app {
            panel = panel.child(div().px(px(6.)).py(px(4.)).child(note(t!("styles.targets.no_app"))));
        }
        let plain_note = |s: String| kit::plain(s, 12., 16., FontWeight::NORMAL, c.graphite).flex_none();
        for (i, choice) in choices.into_iter().take(MAX_CHOICES).enumerate() {
            // The name of the row says what a pick does: "Add site, example.com".
            let (key, name) = match &choice {
                Choice::App(app) => (format!("choice-app-{}", app.bundle_id), app.name.clone()),
                Choice::AppId(id) => {
                    (format!("choice-id-{id}"), format!("{}, {id}", t!(crate::shell::os_text!("styles.targets.add_bundle_id", "styles.targets.add_app_id"))))
                }
                Choice::Site(site) => (format!("choice-site-{site}"), format!("{}, {site}", t!("styles.targets.add_site"))),
            };
            let line = Pressable::new(SharedString::from(key))
                .a11y_role(Role::ListBoxOption)
                .label(name)
                .flex()
                .flex_none()
                .items_center()
                .gap(px(10.))
                .h(px(36.))
                .px(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                // Return adds the first row.
                .when(i == 0 && typed, |d| d.bg(c.accent_wash))
                .hover(|s| s.bg(c.accent_wash));
            let slot = |ic: Icon| div().flex().flex_none().items_center().justify_center().size(px(20.)).child(icon(ic, 14., c.graphite));
            let label = |s: &str| kit::plain(s.to_string(), 14., 18., FontWeight::MEDIUM, c.ink).flex_none();
            let id_text = |s: &str, color: Hsla| div().flex_1().min_w_0().child(kit::plain_mono(s.to_string(), 11., color).truncate());
            let line = match &choice {
                Choice::App(app) => line
                    .child(AppBadge::new(app.name.clone()).icon(m.icons.get(&app.bundle_id)).size(20.))
                    .child(label(&app.name))
                    .child(id_text(&app.bundle_id, c.graphite))
                    .when_some(moves_from(rules.style_of_app(&app.bundle_id)), |d, s| d.child(plain_note(s))),
                Choice::AppId(id) => line
                    .child(slot(Icon::Plus))
                    .child(label(&t!(crate::shell::os_text!("styles.targets.add_bundle_id", "styles.targets.add_app_id"))))
                    .child(id_text(id, c.ink)),
                Choice::Site(site) => line
                    .child(slot(Icon::Globe))
                    .child(label(&t!("styles.targets.add_site")))
                    .child(id_text(site, c.ink))
                    .when_some(moves_from(rules.style_of_site(site)), |d, s| d.child(plain_note(s))),
            };
            panel = panel.child(line.on_click(cx.listener(move |this, _, window, cx| this.add_target(choice.clone(), window, cx))));
        }

        let intro = match (sites_ok, ed.base.name.is_empty()) {
            (true, true) => t!("styles.targets.intro_sites_unnamed"),
            (true, false) => t!("styles.targets.intro_sites", style = ed.base.name),
            (false, true) => t!("styles.targets.intro_apps_unnamed"),
            (false, false) => t!("styles.targets.intro_apps", style = ed.base.name),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .when(framed, |d| d.pt(px(18.)).border_t_1().border_color(c.rule))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .when(framed, |d| d.child(text::title(t!("styles.targets.title"), 18., &c).line_height(px(24.))))
                    .child(ui(intro, 13., 18., FontWeight::NORMAL, c.graphite)),
            )
            .when(!rules.enabled, |d| d.child(Banner::new(BannerKind::Info, t!("styles.targets.off"))))
            .when(count > 0, |d| d.child(list))
            .child(kit::search_well_named(&ed.target, Some(t!("styles.targets.title").into()), 36., Some(open.into_any_element()), cx))
            .when(typed || ed.target_open, |d| d.child(panel))
            .when(sites_ok, |d| {
                d.child(ui(
                    t!("styles.targets.site_hint"),
                    12.,
                    16.,
                    FontWeight::NORMAL,
                    c.graphite,
                ))
            })
    }

    /// A panel on the right of the page, over a scrim. A click on the scrim closes it.
    fn side_panel(&self, id: &'static str, title: String, body: Div, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let panel = div()
            .id(id)
            .role(Role::Dialog)
            .aria_label(title)
            // Escape closes the panel when the focus is in it.
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    this.editor = None;
                    this.base_editor = None;
                    cx.notify();
                }
            }))
            .absolute()
            .top(px(12.))
            .right(px(12.))
            .bottom(px(12.))
            .w(px(500.))
            .rounded(px(14.))
            .floating(&c)
            .overflow_y_scroll()
            .occlude()
            .child(body);
        let scrim = Pressable::new("scrim")
            .label(t!("styles.a11y.close"))
            .tab_stop(false)
            .absolute()
            .inset_0()
            .bg(c.ground.opacity(0.45))
            .occlude()
            .on_click(cx.listener(|this, _, _, cx| {
                this.editor = None;
                this.base_editor = None;
                cx.notify();
            }));
        div().absolute().inset_0().child(scrim).child(panel).into_any_element()
    }

    // -----------------------------------------------------------------------
    // Base prompt
    // -----------------------------------------------------------------------

    fn open_base_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.model.read(cx).styles.base_prompt().to_string();
        let prompt = cx.new(|cx| TextareaState::new(window, cx).placeholder(t!("styles.base.placeholder")).default_value(text));
        prompt.update(cx, |s, cx| s.focus(window, cx));
        self.base_editor = Some(BaseEditor { prompt, error: None });
        cx.notify();
    }

    fn save_base_editor(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.base_editor.as_ref() else { return };
        let text = ed.prompt.read(cx).value().to_string();
        // The default text saved again is not a change.
        let result = if text.trim() == sayso_core::style::DEFAULT_BASE_PROMPT {
            self.model.update(cx, |m, cx| m.reset_base_prompt(cx));
            Ok(())
        } else {
            self.model.update(cx, |m, cx| m.save_base_prompt(&text, cx))
        };
        match result {
            Ok(()) => self.base_editor = None,
            Err(e) => {
                if let Some(ed) = self.base_editor.as_mut() {
                    ed.error = Some(EditError::BaseSaveFailed(e));
                }
            }
        }
        cx.notify();
    }

    fn base_editor_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = cx.paper().colors;
        let ed = self.base_editor.as_ref()?;
        let m = self.model.read(cx);
        let changed = m.styles.user_base_prompt.is_some();
        let file_note = if changed {
            t!("styles.base.changed_note", file = format!("{}/{}", m.styles_dir_display(), sayso_core::style::BASE_PROMPT_FILE))
        } else {
            t!("styles.base.default_note")
        };
        let prompt = div()
            .p(px(4.))
            .rounded(px(10.))
            .debossed(&c)
            .text_size(px(14.))
            .child(Textarea::new(&ed.prompt).aria_label(t!("styles.base.rules")).appearance(false).h(px(380.)).text_size(px(14.)));
        let mut buttons = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(Button::new("save-base", t!("styles.save")).primary().on_click(cx.listener(|this, _, _, cx| this.save_base_editor(cx))))
            .child(Button::new("cancel-base", t!("styles.cancel")).ghost().on_click(cx.listener(|this, _, _, cx| {
                this.base_editor = None;
                cx.notify();
            })))
            .child(div().flex_1());
        if changed {
            buttons = buttons.child(Button::new("reset-base", t!("styles.base.reset")).icon(Icon::Undo).on_click(cx.listener(|this, _, _, cx| {
                this.model.update(cx, |m, cx| m.reset_base_prompt(cx));
                this.base_editor = None;
                cx.notify();
            })));
        }
        let body = div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .p(px(28.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(12.))
                    .child(div().flex().flex_col().flex_1().min_w_0().gap(px(6.)).child(text::title(t!("styles.base.title"), 28., &c)).child(ui(
                        file_note,
                        13.,
                        18.,
                        FontWeight::NORMAL,
                        c.graphite,
                    )))
                    .child(kit::icon_button("close-base", Icon::Close, 12., cx).label(t!("styles.a11y.close")).on_click(cx.listener(|this, _, _, cx| {
                        this.base_editor = None;
                        cx.notify();
                    }))),
            )
            .child(kit::field(
                &t!("styles.base.rules"),
                prompt,
                Some(&t!("styles.base.rules_hint")),
                cx,
            ))
            .when_some(ed.error.as_ref(), |d, e| d.child(Banner::new(BannerKind::Warning, e.text())))
            .child(div().pt(px(6.)).child(buttons));
        Some(self.side_panel("base-editor", t!("styles.base.title"), body, cx))
    }

    // -----------------------------------------------------------------------
    // Featured style and rows
    // -----------------------------------------------------------------------

    fn edit_button(&self, id: &str, style: &Style, cx: &mut Context<Self>) -> Button {
        let st = style.clone();
        Button::new(SharedString::from(format!("{id}-{}", style.id)), t!("styles.edit")).a11y_label(t!("styles.a11y.edit", name = style.display_name())).small().ghost().on_click(cx.listener(move |this, _, w, cx| {
            cx.stop_propagation();
            this.open_editor(Some(st.clone()), w, cx);
        }))
    }

    /// The featured style: what it does, and one example of speech and the
    /// text that the style makes from it.
    fn featured(&self, style: &Style, active: bool, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let id = style.id.clone();
        let style_name = style.display_name();
        let actions = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .pt(px(6.))
            .map(|d| {
                if active {
                    d.child(Badge::new(t!("styles.default"), BadgeTone::Ink))
                } else {
                    d.child(Button::new("featured-default", t!("styles.make_default")).a11y_label(t!("styles.a11y.make_default", name = style_name)).primary().small().on_click(cx.listener(move |this, _, _, cx| {
                        let id = id.clone();
                        this.model.update(cx, |m, cx| m.set_active_style(&id, cx));
                    })))
                }
            })
            .child(self.edit_button("featured-edit", style, cx));
        let about = div()
            .flex()
            .flex_col()
            .flex_none()
            .when(!narrow, |d| d.w(px(252.)))
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(20.))
                    .child(kit::seal_dot(Some(style.ink.as_str()), 14., cx))
                    .child(kit::caps(t!("styles.recommended"), 11., c.graphite)),
            )
            .child(text::title(style.display_name(), 26., &c).line_height(px(32.)))
            .child(ui(style.display_description(), 14., 20., FontWeight::NORMAL, c.graphite))
            .child(actions);
        let side = |label: String, body: Div| div().flex().flex_col().flex_1().min_w_0().gap(px(6.)).py(px(14.)).px(px(16.)).child(kit::caps(label, 11., c.graphite)).child(body);
        let example = div()
            .flex()
            .flex_1()
            .min_w_0()
            .rounded(px(10.))
            .debossed(&c)
            .child(side(t!("styles.featured.you_say"), kit::mono(t!("styles.featured.said"), 12., c.graphite).line_height(px(18.))))
            .child(side(t!("styles.featured.sayso_writes"), ui(t!("styles.featured.written"), 14., 20., FontWeight::MEDIUM, c.ink)).border_l_1().border_color(c.deboss_shade));
        div()
            .flex()
            .when(narrow, |d| d.flex_col())
            .gap(px(if narrow { 16. } else { 24. }))
            .py(px(20.))
            .px(px(22.))
            .rounded(px(14.))
            .bg(c.sheet_raised)
            .shadow(vec![
                BoxShadow::new(px(0.), px(1.), c.highlight(1.0)).inset(),
                BoxShadow::new(px(0.), px(1.), c.shadow(0.14)).blur_radius(px(2.)),
                BoxShadow::new(px(0.), px(8.), c.shadow(0.08)).blur_radius(px(22.)),
            ])
            .child(about)
            .child(example)
            .into_any_element()
    }

    /// The detail of a style row: the apps and sites of the style, or its
    /// provider when that is not the default provider.
    ///
    /// The last value is true for the warning of a cleanup model that cannot
    /// run the style. It comes before the apps and sites: the style does not
    /// run as the user wants, whatever its rules say.
    fn row_detail(&self, style: &Style, cx: &App) -> Option<(String, Hsla, bool)> {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let route = m.style_route(style);
        if matches!(route, StyleRoute::CleanupCannotRun { .. }) {
            return Some((route.line(), c.danger, true));
        }
        if let Some(line) = self.target_line(&style.id, cx) {
            return Some((line, c.ink, false));
        }
        let color = match &route {
            StyleRoute::NoAi => c.graphite,
            StyleRoute::NeedsModel { .. } => c.danger,
            // The line below the rows names the default provider.
            _ if route == m.style_route(&Style { provider: None, model: None, ..style.clone() }) => return None,
            StyleRoute::Cloud { .. } => c.accent,
            StyleRoute::Local { .. } => c.success,
            StyleRoute::NeedsProvider => c.pencil,
            StyleRoute::CleanupCannotRun { .. } => c.danger,
        };
        Some((route.line(), color, false))
    }

    /// A style row is a radio button of the default style: `place` is its
    /// index and the count of the group.
    fn row(&self, style: &Style, active: bool, place: (usize, usize), narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let last = place.0 + 1 == place.1;
        let hovered = self.hovered.as_deref() == Some(style.id.as_str());
        let detail = self.row_detail(style, cx);
        let has_rule = detail.as_ref().is_some_and(|d| !d.2) && self.target_line(&style.id, cx).is_some();
        let id2 = style.id.clone();
        let id3 = style.id.clone();
        let st = style.clone();
        let name = style.display_name();
        // The row says what it shows: the style, what it does, and where it goes.
        let mut said = vec![style.display_description()];
        said.extend(detail.as_ref().map(|d| d.0.clone()));
        said.retain(|s| !s.is_empty());
        // The Edit button is in the tab order, also when the row is not hovered:
        // it is clear until the row has the pointer or the focus.
        let edit = Pressable::new("edit")
            .label(t!("styles.a11y.edit", name = name))
            .absolute()
            .right_0()
            .flex()
            .items_center()
            .justify_center()
            .h(px(28.))
            .px(px(12.))
            .rounded(px(8.))
            .font_family(UI)
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(c.graphite)
            .cursor_pointer()
            .hover(|s| s.bg(c.deboss.opacity(0.6)))
            .when(!hovered, |d| d.opacity(0.).focus(|s| s.opacity(1.)))
            .child(t!("styles.edit"))
            .on_click(cx.listener(move |this, _, w, cx| {
                cx.stop_propagation();
                this.open_editor(Some(st.clone()), w, cx);
            }));
        Pressable::new(SharedString::from(format!("style-{}", style.id)))
            .a11y_radio(name.clone(), active, place.0, place.1)
            .aria_description(said.join(", "))
            .flex()
            .items_center()
            .gap(px(14.))
            .h(px(48.))
            .px(px(4.))
            .when(!last, |d| d.border_b_1().border_color(c.rule))
            .when(hovered, |d| d.bg(c.deboss.opacity(0.55)))
            .cursor_pointer()
            .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                if *h {
                    this.hovered = Some(id2.clone());
                } else if this.hovered.as_deref() == Some(id2.as_str()) {
                    this.hovered = None;
                }
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                let id = id3.clone();
                this.model.update(cx, |m, cx| m.set_active_style(&id, cx));
            }))
            .child(kit::seal_dot((!style.is_raw()).then_some(style.ink.as_str()), 14., cx))
            .child(
                div()
                    .font_family(DISPLAY)
                    .locale_fonts(Face::Serif)
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c.ink)
                    .line_height(px(24.))
                    .flex_none()
                    .w(px(112.))
                    .truncate()
                    .child(name),
            )
            .child(kit::plain(style.display_description(), 14., 20., FontWeight::NORMAL, c.graphite).flex_1().min_w_0().truncate())
            .child(div().flex().flex_none().items_center().gap(px(6.)).w(px(if narrow { 150. } else { 236. })).when_some(detail, |d, (line, color, warning)| {
                d.when(has_rule, |d| d.child(icon(Icon::ArrowRight, 11., c.graphite)))
                    .when(warning, |d| d.child(icon(Icon::Warning, 12., color)))
                    .child(kit::plain(line, 12., 16., FontWeight::MEDIUM, color).min_w_0().truncate())
            }))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_end()
                    .w(px(80.))
                    // Edit takes the place of the badge, so the row keeps its columns.
                    .child(edit)
                    .when(active && !hovered, |d| d.child(Badge::new(t!("styles.default"), BadgeTone::Ink))),
            )
            .into_any_element()
    }

    /// The default provider in one line, with the way to the providers.
    fn provider_line(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let default = m.config.ai.provider(None).filter(|_| m.config.ai.enabled);
        let (dot, line) = match default {
            Some(p) => {
                // A cleanup model has its own name and no model to choose.
                let model = p.model().filter(|_| !matches!(p.kind, ProviderKind::CleanupModel { .. }));
                let line = match model {
                    Some(id) => t!("styles.provider_line.with_model", provider = p.name, model = m.model_label(&p.id, id)),
                    None => t!("styles.provider_line.no_model", provider = p.name),
                };
                (c.success, line)
            }
            None if m.config.ai.providers.is_empty() => (c.pencil, t!("styles.provider_line.no_provider")),
            None => (c.pencil, t!("styles.provider_line.ai_off")),
        };
        let label = if m.config.ai.providers.is_empty() { t!("styles.provider_line.set_up") } else { t!("styles.provider_line.manage") };
        let model = self.model.clone();
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .pt(px(16.))
            .border_t_1()
            .border_color(c.rule)
            .child(div().flex_none().size(px(8.)).rounded_full().bg(dot))
            .child(ui(line, 13., 18., FontWeight::NORMAL, c.graphite).flex_1().min_w_0())
            .child(
                Pressable::new("manage-providers")
                    .a11y_role(Role::Link)
                    .label(label.clone())
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.75))
                    .on_click(move |_, _, cx| model.update(cx, |m, cx| m.navigate(crate::hub::Route::Settings(crate::hub::SettingsPage::Providers), cx)))
                    .child(kit::plain(label, 13., 18., FontWeight::SEMIBOLD, c.accent))
                    .child(icon(Icon::ChevronRight, 12., c.accent)),
            )
            .into_any_element()
    }
}

/// The names of apps and sites in one line: "Mail", "Mail and Notes", "Mail, Notes, and 3 more".
fn names_line(names: &[String]) -> Option<String> {
    Some(match names {
        [] => return None,
        [one] => one.clone(),
        [one, two] => t!("styles.targets.names_two", one = one, two = two),
        [one, two, rest @ ..] => t!("styles.targets.names_many", one = one, two = two, more = rest.len()),
    })
}

/// A part of the editor that is folded to one row: a title, a line of detail,
/// and the body when it is open.
fn fold_row(
    id: &'static str,
    title: String,
    detail: String,
    open: bool,
    on_toggle: impl Fn(&mut Window, &mut App) + 'static,
    body: Div,
    c: &Colors,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .pt(px(14.))
        .border_t_1()
        .border_color(c.rule)
        .child(
            Pressable::new(id)
                .label(title.clone())
                .aria_description(detail.clone())
                .aria_expanded(open)
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.))
                .cursor_pointer()
                .on_click(move |_, w, cx| on_toggle(w, cx))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(ui(title, 13., 16., FontWeight::SEMIBOLD, c.ink))
                        .child(ui(detail, 12., 16., FontWeight::NORMAL, c.graphite)),
                )
                .child(icon(Icon::ChevronDown, 12., c.graphite)),
        )
        .when(open, |d| d.child(body))
}

/// A line of the provider menu of the editor.
struct ProviderRow {
    /// The provider id. None is the default provider.
    pick: Option<String>,
    label: String,
    selected: bool,
    /// A cleanup model that cannot run the style. It is grey and no pick.
    blocked: bool,
}

/// The provider menu of the editor. `note` is the first line when the current
/// choice cannot run the style: its name, and why. Place it inside a
/// `relative()` parent.
fn provider_menu(
    note: Option<(String, String)>,
    rows: Vec<ProviderRow>,
    on_pick: impl Fn(usize, &mut Window, &mut App) + 'static,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let c = cx.paper().colors;
    let on_pick = std::rc::Rc::new(on_pick);
    let on_close = std::rc::Rc::new(on_close);
    let close = on_close.clone();
    let radio = rows.iter().any(|r| r.selected);
    let mut list = div()
        .id("provider-menu")
        .role(Role::Menu)
        .flex()
        .flex_col()
        .min_w(px(280.))
        .max_h(px(320.))
        .overflow_y_scroll()
        .p(px(4.))
        .rounded(px(10.))
        .bg(c.sheet_raised)
        .shadow(sayso_ui::paper::floating(&c))
        .locale_fonts(Face::Sans)
        .occlude()
        .on_mouse_down_out(move |_, w, cx| close(w, cx))
        .on_key_down(move |e, w, cx| {
            match e.keystroke.key.as_str() {
                "escape" => on_close(w, cx),
                "down" => w.focus_next(cx),
                "up" => w.focus_prev(cx),
                _ => return,
            }
            cx.stop_propagation();
        });
    if let Some((label, why)) = note {
        list = list
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(10.))
                    .py(px(6.))
                    .child(kit::plain(label, 13., 16., FontWeight::NORMAL, c.pencil))
                    .child(kit::plain(why, 12., 16., FontWeight::NORMAL, c.danger)),
            )
            .child(div().h(px(1.)).mx(px(10.)).my(px(4.)).bg(c.rule));
    }
    for (i, row) in rows.into_iter().enumerate() {
        let f = on_pick.clone();
        let weight = if row.selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL };
        let detail = row.blocked.then(|| t!("styles.editor.no_prompts"));
        let name = match &detail {
            Some(d) => format!("{}, {d}", row.label),
            None => row.label.clone(),
        };
        let item = Pressable::new(i).label(name).disabled(row.blocked);
        let item = if radio {
            item.a11y_role(Role::MenuItemRadio).aria_toggled(if row.selected { Toggled::True } else { Toggled::False })
        } else {
            item.a11y_role(Role::MenuItem)
        };
        list = list.child(
            item.flex()
                .items_center()
                .gap(px(8.))
                .h(px(30.))
                .px(px(10.))
                .rounded(px(7.))
                .when(!row.blocked, |d| d.cursor_pointer().hover(|s| s.bg(c.deboss)))
                .on_click(move |_, w, cx| f(i, w, cx))
                .child(div().w(px(14.)).flex_none().when(row.selected, |d| d.child(icon(Icon::Check, 12., c.ink))))
                .child(kit::plain(row.label, 13., 16., weight, if row.blocked { c.pencil } else { c.ink }).flex_1().min_w_0())
                .when_some(detail, |d, detail| d.child(kit::plain(detail, 13., 16., FontWeight::NORMAL, c.pencil).flex_none().ml(px(8.)))),
        );
    }
    deferred(anchored().snap_to_window_with_margin(px(8.)).child(div().mt(px(4.)).child(list))).with_priority(2)
}

fn trim_float(v: f64) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

impl Render for StylesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c: Colors = cx.paper().colors;
        // The inputs of the editors keep their placeholders, so they change with the language.
        if self.locale != sayso_i18n::locale() {
            self.locale = sayso_i18n::locale();
            let http_s = self.model.read(cx).config.ai.http_timeout_ms as f64 / 1000.0;
            let sites_ok = self.model.read(cx).services.platform.context.reads_page_url();
            if let Some(ed) = &self.editor {
                ed.name.update(cx, |s, cx| s.set_placeholder(t!("styles.editor.name_placeholder"), window, cx));
                ed.description.update(cx, |s, cx| s.set_placeholder(t!("styles.editor.description_placeholder"), window, cx));
                ed.prompt.update(cx, |s, cx| s.set_placeholder(t!("styles.editor.prompt_placeholder"), window, cx));
                ed.timeout.update(cx, |s, cx| s.set_placeholder(t!("styles.editor.timeout_placeholder", seconds = trim_float(http_s)), window, cx));
                ed.target.update(cx, |s, cx| s.set_placeholder(target_placeholder(sites_ok), window, cx));
            }
            if let Some(ed) = &self.base_editor {
                ed.prompt.update(cx, |s, cx| s.set_placeholder(t!("styles.base.placeholder"), window, cx));
            }
        }
        let (styles, active, dir) = {
            let m = self.model.read(cx);
            (m.styles.styles().cloned().collect::<Vec<_>>(), m.active_style().id, m.styles_dir_display())
        };
        let actions = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(Button::new("edit-base", t!("styles.base.button")).large().icon(Icon::File).on_click(cx.listener(|this, _, w, cx| this.open_base_editor(w, cx))))
            .child(
                Button::new("new-style", t!("styles.new_style"))
                    .primary()
                    .large()
                    .icon(Icon::Plus)
                    .on_click(cx.listener(|this, _, w, cx| this.open_editor(None, w, cx))),
            )
            .into_any_element();
        let head = kit::page_head(
            &t!("styles.title"),
            Some(kit::intro(
                t!("styles.intro", dir = dir),
                560.,
                cx,
            )),
            Some(actions),
            cx,
        );
        let pad = crate::layout::page_pad(window, &self.model.read(cx).config);
        let narrow = crate::layout::page_narrow(window, &self.model.read(cx).config);
        let default_provider = {
            let m = self.model.read(cx);
            m.config.ai.provider(None).filter(|_| m.config.ai.enabled).cloned()
        };
        let featured = featured_style(&styles, default_provider.as_ref()).map(|s| self.featured(s, s.id == active, narrow, cx));
        // The default style is a choice of one: the rows are radio buttons, and
        // the arrows pick the next style and move the focus.
        let count = styles.len();
        let current = styles.iter().position(|s| s.id == active);
        let ids: Vec<String> = styles.iter().map(|s| s.id.clone()).collect();
        let mut rows = div()
            .id("style-rows")
            .role(Role::RadioGroup)
            .aria_label(t!("styles.a11y.default_style"))
            .flex()
            .flex_col()
            .when_some(current, |d, current| {
                d.on_key_down(cx.listener(move |this, ev: &KeyDownEvent, w, cx| {
                    let next = match ev.keystroke.key.as_str() {
                        "right" | "down" if current + 1 < count => current + 1,
                        "left" | "up" if current > 0 => current - 1,
                        _ => return,
                    };
                    cx.stop_propagation();
                    let id = ids[next].clone();
                    this.model.update(cx, |m, cx| m.set_active_style(&id, cx));
                    // The Edit button of a row is a tab stop between the rows.
                    for _ in 0..2 {
                        if next > current { w.focus_next(cx) } else { w.focus_prev(cx) }
                    }
                }))
            });
        for (i, s) in styles.iter().enumerate() {
            rows = rows.child(self.row(s, s.id == active, (i, count), narrow, cx));
        }
        let errors = self.model.read(cx).styles.errors.clone();
        let providers = self.provider_line(cx);
        let editor = self.editor_panel(cx).or_else(|| self.base_editor_panel(cx));
        div()
            .absolute()
            .inset_0()
            .text_color(c.ink)
            .child(
                div().id("styles").absolute().inset_0().overflow_y_scroll().child(
                    div()
                        .flex()
                        .flex_col()
                        .w_full()
                        .gap(px(24.))
                        .py(px(pad.min(44.)))
                        .px(px(pad))
                        .child(head)
                        .children(errors.into_iter().map(|e| Banner::new(BannerKind::Warning, t!("styles.file_error", error = e))))
                        .children(featured)
                        .child(rows)
                        .child(providers),
                ),
            )
            .children(editor)
    }
}

#[cfg(test)]
mod tests {
    use super::{CleanupEdit, CleanupField, CleanupSettings, Format, Layout, Provider, ProviderKind, Settings, Style, Tone};
    use super::{cannot_run, cleanup_to_save, featured_style, style_as_edited};
    use sayso_core::style::builtin_styles;

    fn cleanup_model(model: &str) -> Provider {
        Provider { id: "c".into(), name: "C".into(), kind: ProviderKind::CleanupModel { model: model.into() } }
    }

    fn apple() -> Provider {
        Provider { id: "a".into(), name: "A".into(), kind: ProviderKind::AppleIntelligence }
    }

    fn built_in(id: &str) -> Style {
        builtin_styles().into_iter().find(|s| s.id == id).unwrap()
    }

    fn featured_id(default: Option<&Provider>) -> Option<String> {
        featured_style(&builtin_styles(), default).map(|s| s.id.clone())
    }

    #[test]
    fn the_featured_style_stays_without_a_cleanup_model() {
        assert_eq!(featured_id(None).as_deref(), Some("rambling"));
        assert_eq!(featured_id(Some(&apple())).as_deref(), Some("rambling"));
    }

    #[test]
    fn the_featured_style_is_one_that_the_cleanup_model_runs() {
        // S1-mini has no prompts, so Rambling is out. Email is the first that it runs.
        assert_eq!(featured_id(Some(&cleanup_model("s1-mini"))).as_deref(), Some("email"));
        // Cleanup 1 has no format email and no lists.
        assert_eq!(featured_id(Some(&cleanup_model("spokenly-cleanup-1"))).as_deref(), Some("message"));
    }

    #[test]
    fn a_model_that_runs_nothing_keeps_the_usual_featured_style() {
        assert_eq!(featured_id(Some(&cleanup_model("no-such-model"))).as_deref(), Some("rambling"));
    }

    #[test]
    fn a_new_style_has_its_own_default_cleanup_settings() {
        let mut style = built_in("raw");
        style.id = String::new();
        let edit = CleanupEdit::new(&style, true);
        assert!(edit.own);
        assert_eq!(edit.settings, Settings::default());
        // With a prompt, the new style still runs on a cleanup model.
        let edited = style_as_edited(&style, "Make it short.", &edit);
        assert_eq!(cannot_run(&edited, Some(&cleanup_model("s1-mini"))), None);
    }

    #[test]
    fn a_style_with_no_cleanup_settings_is_blocked_until_the_user_gives_it_some() {
        let polished = built_in("polished");
        let mut edit = CleanupEdit::new(&polished, false);
        let model = cleanup_model("s1-mini");
        let edited = style_as_edited(&polished, &polished.prompt, &edit);
        assert_eq!(cannot_run(&edited, Some(&model)).map(|m| m.name), Some("S1-mini"));
        edit.change(|s| s.tone = Tone::Casual);
        let edited = style_as_edited(&polished, &polished.prompt, &edit);
        assert_eq!(cannot_run(&edited, Some(&model)), None);
        // A provider that reads prompts is never blocked.
        assert_eq!(cannot_run(&edited, Some(&apple())), None);
        assert_eq!(cannot_run(&edited, None), None);
    }

    #[test]
    fn the_settings_that_the_editor_has_decide_what_a_model_runs() {
        let email = built_in("email");
        let mut edit = CleanupEdit::new(&email, false);
        assert_eq!(edit.settings.format, Format::Email);
        let cleanup_1 = cleanup_model("spokenly-cleanup-1");
        let edited = style_as_edited(&email, &email.prompt, &edit);
        assert_eq!(cannot_run(&edited, Some(&cleanup_1)).map(|m| m.name), Some("Cleanup 1"));
        edit.change(|s| s.format = Format::Plain);
        let edited = style_as_edited(&email, &email.prompt, &edit);
        assert_eq!(cannot_run(&edited, Some(&cleanup_1)), None);
    }

    #[test]
    fn save_writes_the_settings_only_when_there_are_some() {
        // A style that has known settings gets them back as a table.
        let clean = built_in("clean");
        let edit = CleanupEdit::new(&clean, false);
        assert_eq!(cleanup_to_save(&clean, &edit), Some(CleanupField::Settings(CleanupSettings::default())));
        // A style that has none and was not changed stays as it is.
        let polished = built_in("polished");
        assert_eq!(cleanup_to_save(&polished, &CleanupEdit::new(&polished, false)), None);
        // A new style, and a style that the user changed, get a table.
        let mut changed = CleanupEdit::new(&polished, false);
        changed.change(|s| s.tone = Tone::VeryCasual);
        let saved = cleanup_to_save(&polished, &changed);
        assert_eq!(saved, Some(CleanupField::Settings(CleanupSettings::new(Tone::VeryCasual, Layout::Paragraphs, Format::Plain))));
        assert!(cleanup_to_save(&polished, &CleanupEdit::new(&polished, true)).is_some());
    }

    #[test]
    fn save_keeps_a_value_that_this_version_does_not_know() {
        let mut style = built_in("clean");
        style.cleanup = CleanupField::Settings(CleanupSettings { layout: Some("table".into()), ..Default::default() });
        assert_eq!(cleanup_to_save(&style, &CleanupEdit::new(&style, false)), None);
    }
}
