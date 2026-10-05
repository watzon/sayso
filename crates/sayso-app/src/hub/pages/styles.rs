//! The styles page. See the Paper board "Hub — Styles".
//!
//! A grid of style cards (click to make one active, Edit to change it), the
//! style editor panel, and the Providers strip with its Add flow.

use super::ext::{StyleRoute, provider_detail};
use super::kit::{self, ui};
use crate::model::AppModel;
use crate::model_picker::{self, Extra};
use gpui_kit::component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::config::{Provider, ProviderKind, is_local_url};
use sayso_core::ink::NamedInk;
use sayso_core::style::{Style, StyleOrigin};
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::{ActivePaper, Colors, text};
use std::collections::HashMap;

#[derive(Clone)]
enum Test {
    Running,
    Passed(u64),
    Failed(String),
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
    error: Option<String>,
    confirm_delete: bool,
}

/// The editor of the base prompt.
struct BaseEditor {
    prompt: Entity<TextareaState>,
    error: Option<String>,
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
    error: Option<String>,
}

/// Height of the tiles in the Providers strip: name, model, and status lines.
const CARD_H: f32 = 78.;

/// (name, base URL). The model is chosen from the provider's list after it is added.
const PRESETS: [(&str, &str); 4] = [
    ("OpenRouter", "https://openrouter.ai/api/v1"),
    ("OpenAI", "https://api.openai.com/v1"),
    ("Ollama", "http://localhost:11434/v1"),
    ("LM Studio", "http://localhost:1234/v1"),
];

pub struct StylesPage {
    model: Entity<AppModel>,
    hovered: Option<String>,
    hovered_provider: Option<String>,
    /// The provider whose "⋯" menu is open.
    provider_menu: Option<String>,
    confirm_remove: Option<String>,
    editor: Option<Editor>,
    base_editor: Option<BaseEditor>,
    add: Option<AddProvider>,
    tests: HashMap<String, Test>,
    _subs: Vec<Subscription>,
}

impl StylesPage {
    pub fn new(model: Entity<AppModel>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            hovered: None,
            hovered_provider: None,
            provider_menu: None,
            confirm_remove: None,
            editor: None,
            base_editor: None,
            add: None,
            tests: HashMap::new(),
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
            apps: Vec::new(),
            standalone: false,
        });
        let origin = if is_new { None } else { self.model.read(cx).style_origin(&base.id) };
        let http_s = self.model.read(cx).config.ai.http_timeout_ms as f64 / 1000.0;
        let b = base.clone();
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("For example, Slack").default_value(b.name.clone()));
        let description = cx.new(|cx| InputState::new(window, cx).placeholder("One line about what the style does").default_value(b.description.clone()));
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Tell the AI how to rewrite the text. Leave empty for no AI.")
                .default_value(b.prompt.clone())
        });
        let timeout = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(format!("Default, {} s", trim_float(http_s)))
                .default_value(b.timeout_ms.map(|ms| trim_float(ms as f64 / 1000.0)).unwrap_or_default())
        });
        name.update(cx, |s, cx| s.focus(window, cx));
        let style_model = base.model.clone();
        let use_base = !base.standalone;
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
            error: None,
            confirm_delete: false,
        });
        cx.notify();
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.editor.as_mut() else { return };
        let name = ed.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            ed.error = Some("Enter a name for the style.".into());
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
                    ed.error = Some("Enter the timeout in seconds, from 0.5 to 120. For example, 4.".into());
                    cx.notify();
                    return;
                }
            }
        };
        let id = if ed.is_new { self.model.read(cx).new_style_id(&name) } else { ed.id.clone() };
        let style = Style {
            id,
            name,
            ink: ed.ink.clone(),
            description: ed.description.read(cx).value().trim().to_string(),
            prompt: ed.prompt.read(cx).value().trim().to_string(),
            provider: ed.provider.clone(),
            model: ed.model.clone(),
            timeout_ms,
            standalone: !ed.use_base,
            ..ed.base.clone()
        };
        match self.model.update(cx, |m, cx| m.save_style(&style, cx)) {
            Ok(()) => self.editor = None,
            Err(e) => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.error = Some(format!("Sayso could not save the style: {e}"));
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
                Some(id) => format!("Provider's model ({})", m.model_label(&p.id, id)),
                None => "Provider's model".into(),
            },
            (None, None) => "Provider's model".into(),
        };

        // Ink swatches.
        let mut swatches = div().flex().items_center().gap(px(10.));
        for ink in NamedInk::ALL {
            let hex = format!("#{:06X}", ink.color().0);
            let selected = ed.ink.eq_ignore_ascii_case(&hex);
            let h2 = hex.clone();
            swatches = swatches.child(
                div()
                    .id(SharedString::from(format!("ink-{}", ink.name())))
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
            .map(|n| n.name().to_string())
            .unwrap_or_else(|| format!("Custom {}", ed.ink));
        swatches = swatches.child(ui(ink_name, 13., 16., FontWeight::MEDIUM, c.graphite).pl(px(4.)));

        // Provider select.
        let current = match &ed.provider {
            Some(id) => providers.iter().find(|p| &p.0 == id).map(|p| p.1.clone()).unwrap_or_else(|| format!("{id} (not set up)")),
            None => match &default_name {
                Some(n) => format!("Default provider ({n})"),
                None => "Default provider".into(),
            },
        };
        let mut select = div().relative().child(
            div()
                .id("provider-select")
                .flex()
                .items_center()
                .justify_between()
                .h(px(36.))
                .px(px(12.))
                .rounded(px(9.))
                .raised_small(&c)
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(ed) = this.editor.as_mut() {
                        ed.provider_menu = !ed.provider_menu;
                    }
                    cx.notify();
                }))
                .child(ui(current, 14., 18., FontWeight::NORMAL, c.ink))
                .child(icon(Icon::ChevronDown, 12., c.graphite)),
        );
        if ed.provider_menu {
            let mut items: Vec<(SharedString, bool)> = vec![("Default provider".into(), ed.provider.is_none())];
            for (id, name) in &providers {
                items.push((name.clone().into(), ed.provider.as_deref() == Some(id)));
            }
            let ids: Vec<String> = providers.iter().map(|p| p.0.clone()).collect();
            let e1 = cx.entity();
            let e2 = cx.entity();
            select = select.child(div().absolute().top_full().left_0().child(kit::menu(
                "provider-menu",
                items,
                move |i, _, cx| {
                    let pick = if i == 0 { None } else { ids.get(i - 1).cloned() };
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
            ui("No providers yet. Add one under Providers to use AI.", 13., 18., FontWeight::NORMAL, c.graphite).into_any_element()
        } else {
            select.into_any_element()
        };

        let model_field = match target {
            None => ui("Add a provider to choose a model.", 13., 18., FontWeight::NORMAL, c.graphite).into_any_element(),
            Some(p) => {
                let pid = p.id.clone();
                div()
                    .id("style-model")
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
                            extra: Some(Extra { label: "Provider's model".into(), selected: selected.is_none() }),
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
                    .child(ui(model_label, 14., 18., FontWeight::NORMAL, c.ink).truncate())
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
            .child(Textarea::new(&ed.prompt).appearance(false).h(px(150.)).text_size(px(14.)));

        let title = if ed.is_new { "New style".to_string() } else { format!("Edit {}", ed.base.name) };
        let file_note = if ed.is_new {
            format!("Saved as a file in {}.", styles_dir)
        } else {
            match ed.origin {
                Some(StyleOrigin::BuiltIn) => "A built-in style. Saving writes your copy to the styles folder.".into(),
                Some(StyleOrigin::Overridden) => "You changed this built-in style. Reset brings back the original.".into(),
                _ => format!("{}/{}.toml", styles_dir, ed.id),
            }
        };

        let mut buttons = div().flex().items_center().gap(px(8.)).child(
            Button::new("save-style", "Save").primary().on_click(cx.listener(|this, _, _, cx| this.save_editor(cx))),
        );
        buttons = buttons.child(Button::new("cancel-style", "Cancel").ghost().on_click(cx.listener(|this, _, _, cx| {
            this.editor = None;
            cx.notify();
        })));
        buttons = buttons.child(div().flex_1());
        if ed.origin == Some(StyleOrigin::Overridden) {
            let id = ed.id.clone();
            buttons = buttons.child(Button::new("reset-style", "Reset to built-in").icon(Icon::Undo).on_click(cx.listener(
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
                buttons = buttons.child(Button::new("delete-style-yes", "Delete style").danger().on_click(cx.listener(move |this, _, _, cx| {
                    let id = id.clone();
                    this.model.update(cx, |m, cx| m.delete_style(&id, cx));
                    this.editor = None;
                    cx.notify();
                })));
            } else {
                buttons = buttons.child(Button::new("delete-style", "Delete").danger().icon(Icon::Trash).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(ed) = this.editor.as_mut() {
                        ed.confirm_delete = true;
                    }
                    cx.notify();
                })));
            }
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
                    .child(div().flex().flex_col().gap(px(6.)).child(text::title(title, 28., &c)).child(ui(
                        file_note,
                        13.,
                        18.,
                        FontWeight::NORMAL,
                        c.graphite,
                    )))
                    .child(kit::icon_button("close-editor", Icon::Close, 12., cx).on_click(cx.listener(|this, _, _, cx| {
                        this.editor = None;
                        cx.notify();
                    }))),
            )
            .child(kit::field("Name", kit::input_well(&ed.name, cx), None, cx))
            .child(kit::field("Description", kit::input_well(&ed.description, cx), None, cx))
            .child(kit::field("Ink", swatches, None, cx))
            .child(kit::field("Prompt", prompt, Some("Sayso adds your dictionary words and asks for JSON. You only write the style."), cx))
            .child(
                div()
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
                            .child(ui("Use the base prompt", 13., 16., FontWeight::SEMIBOLD, c.ink))
                            .child(ui(
                                "The base prompt cleans up the text: filler words, corrections, numbers. Turn it off for a style that does something else, for example a translation.",
                                12.,
                                16.,
                                FontWeight::NORMAL,
                                c.graphite,
                            )),
                    )
                    .child(div().flex_none().child(Switch::new("use-base", ed.use_base).on_toggle({
                        let page = cx.entity();
                        move |on, _, cx| {
                            page.update(cx, |this, cx| {
                                if let Some(ed) = this.editor.as_mut() {
                                    ed.use_base = on;
                                }
                                cx.notify();
                            })
                        }
                    }))),
            )
            .child(kit::field("Provider", provider_field, None, cx))
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(kit::field("Model", model_field, None, cx).flex_1())
                    .child(kit::field("Timeout in seconds", kit::input_well(&ed.timeout, cx), None, cx).w(px(150.))),
            )
            .when_some(ed.error.clone(), |d, e| d.child(Banner::new(BannerKind::Warning, e)))
            .child(div().pt(px(6.)).child(buttons));

        Some(self.side_panel("style-editor", body, cx))
    }

    /// A panel on the right of the page, over a scrim. A click on the scrim closes it.
    fn side_panel(&self, id: &'static str, body: Div, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let panel = div()
            .id(id)
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
        let scrim = div()
            .id("scrim")
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
        let prompt = cx.new(|cx| TextareaState::new(window, cx).placeholder("Rules that all styles share. Leave empty for no shared rules.").default_value(text));
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
                    ed.error = Some(format!("Sayso could not save the base prompt: {e}"));
                }
            }
        }
        cx.notify();
    }

    /// The row under the style cards that opens the base prompt.
    fn base_prompt_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let changed = self.model.read(cx).styles.user_base_prompt.is_some();
        let note = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .when(changed, |d| d.child(Badge::new("Changed", BadgeTone::Ink)))
            .child(Button::new("edit-base", "Edit").small().on_click(cx.listener(|this, _, w, cx| this.open_base_editor(w, cx))));
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(kit::section_head("Base prompt", Some(note.into_any_element()), cx))
            .child(ui(
                "The cleanup rules that all styles share: filler words, corrections, numbers, and acronyms. Each style adds its own prompt to them.",
                13.,
                18.,
                FontWeight::NORMAL,
                c.graphite,
            ))
            .into_any_element()
    }

    fn base_editor_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = cx.paper().colors;
        let ed = self.base_editor.as_ref()?;
        let m = self.model.read(cx);
        let changed = m.styles.user_base_prompt.is_some();
        let file_note = if changed {
            format!("You changed the base prompt. It is the file {}/{}.", m.styles_dir_display(), sayso_core::style::BASE_PROMPT_FILE)
        } else {
            "The default base prompt. Saving a change writes your copy to the styles folder.".to_string()
        };
        let prompt = div()
            .p(px(4.))
            .rounded(px(10.))
            .debossed(&c)
            .text_size(px(14.))
            .child(Textarea::new(&ed.prompt).appearance(false).h(px(380.)).text_size(px(14.)));
        let mut buttons = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(Button::new("save-base", "Save").primary().on_click(cx.listener(|this, _, _, cx| this.save_base_editor(cx))))
            .child(Button::new("cancel-base", "Cancel").ghost().on_click(cx.listener(|this, _, _, cx| {
                this.base_editor = None;
                cx.notify();
            })))
            .child(div().flex_1());
        if changed {
            buttons = buttons.child(Button::new("reset-base", "Reset to default").icon(Icon::Undo).on_click(cx.listener(|this, _, _, cx| {
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
                    .child(div().flex().flex_col().flex_1().min_w_0().gap(px(6.)).child(text::title("Base prompt", 28., &c)).child(ui(
                        file_note,
                        13.,
                        18.,
                        FontWeight::NORMAL,
                        c.graphite,
                    )))
                    .child(kit::icon_button("close-base", Icon::Close, 12., cx).on_click(cx.listener(|this, _, _, cx| {
                        this.base_editor = None;
                        cx.notify();
                    }))),
            )
            .child(kit::field(
                "Rules for all styles",
                prompt,
                Some("Short rules and a few short examples work best with a small model. A style with \"Use the base prompt\" off does not get this text."),
                cx,
            ))
            .when_some(ed.error.clone(), |d, e| d.child(Banner::new(BannerKind::Warning, e)))
            .child(div().pt(px(6.)).child(buttons));
        Some(self.side_panel("base-editor", body, cx))
    }

    // -----------------------------------------------------------------------
    // Cards
    // -----------------------------------------------------------------------

    fn card(&self, style: &Style, active: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let route = self.model.read(cx).style_route(style);
        let line_color = match &route {
            StyleRoute::Cloud { .. } => c.accent,
            StyleRoute::Local { .. } => c.success,
            StyleRoute::NoAi => c.graphite,
            StyleRoute::NeedsProvider => c.pencil,
            StyleRoute::NeedsModel { .. } => c.danger,
        };
        let hovered = self.hovered.as_deref() == Some(style.id.as_str());
        let id2 = style.id.clone();
        let id3 = style.id.clone();
        let st = style.clone();
        let edit = Button::new(SharedString::from(format!("edit-{}", style.id)), "Edit").small().ghost().on_click(cx.listener(
            move |this, _, w, cx| {
                cx.stop_propagation();
                this.open_editor(Some(st.clone()), w, cx);
            },
        ));
        let mut shadows = Vec::new();
        if active {
            shadows.push(BoxShadow::new(px(0.), px(0.), c.ink).spread_radius(px(2.)));
        }
        shadows.extend([
            BoxShadow::new(px(0.), px(1.), c.highlight(1.0)).inset(),
            BoxShadow::new(px(0.), px(1.), c.shadow(0.14)).blur_radius(px(2.)),
            BoxShadow::new(px(0.), px(8.), c.shadow(if active { 0.12 } else { 0.08 })).blur_radius(px(22.)),
        ]);
        div()
            .id(SharedString::from(format!("style-{}", style.id)))
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h(px(178.))
            .gap(px(10.))
            .py(px(18.))
            .px(px(20.))
            .rounded(px(14.))
            .bg(c.sheet_raised)
            .shadow(shadows)
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(22.))
                    .child(kit::seal_dot(style.uses_ai().then_some(style.ink.as_str()), 16., cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .when(hovered || active, |d| d.child(edit))
                            .when(active, |d| d.child(Badge::new("Active", BadgeTone::Ink))),
                    ),
            )
            .child(text::title(style.name.clone(), 24., &c).line_height(px(30.)))
            .child(ui(style.description.clone(), 14., 20., FontWeight::NORMAL, c.graphite).flex_1().line_clamp(2).text_ellipsis())
            .child(ui(route.line(), 12., 16., FontWeight::MEDIUM, line_color).truncate())
            .into_any_element()
    }

    // -----------------------------------------------------------------------
    // Providers
    // -----------------------------------------------------------------------

    fn test(&mut self, id: String, cx: &mut Context<Self>) {
        self.tests.insert(id.clone(), Test::Running);
        let entity = cx.entity();
        self.model.update(cx, |m, cx| {
            m.test_provider(&id.clone(), cx, move |_, result, cx| {
                entity.update(cx, |this, cx| {
                    this.tests.insert(id, match result {
                        Ok(ms) => Test::Passed(ms),
                        Err(e) => Test::Failed(test_error(&e)),
                    });
                    cx.notify();
                });
            })
        });
        cx.notify();
    }

    fn provider_card(&self, p: &Provider, default: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let hovered = self.hovered_provider.as_deref() == Some(p.id.as_str());
        let test = self.tests.get(&p.id).cloned();
        let (dot, line, line_color) = match &test {
            Some(Test::Running) => (c.accent, "Testing".to_string(), c.graphite),
            Some(Test::Passed(ms)) => (c.success, format!("Answered in {ms} ms"), c.success),
            Some(Test::Failed(e)) => (c.danger, e.clone(), c.danger),
            None => (
                if p.is_cloud() { c.pencil } else { c.success },
                provider_detail(p),
                if p.is_cloud() { c.accent } else { c.graphite },
            ),
        };
        let id_hover = p.id.clone();
        let confirm = self.confirm_remove.as_deref() == Some(p.id.as_str());
        let menu_open = self.provider_menu.as_deref() == Some(p.id.as_str());
        let needs_model = p.needs_model();
        let (dot, line, line_color) = if needs_model && test.is_none() {
            (c.danger, "Choose a model to use this provider".to_string(), c.danger)
        } else {
            (dot, line, line_color)
        };

        let card = div()
            .id(SharedString::from(format!("provider-{}", p.id)))
            .relative()
            .flex()
            .items_center()
            .gap(px(12.))
            .flex_basis(px(230.))
            .flex_grow(1.)
            .min_w(px(230.))
            .h(px(CARD_H))
            .px(px(14.))
            .rounded(px(12.))
            .bg(c.deboss)
            .shadow(vec![
                BoxShadow::new(px(0.), px(1.), c.shadow(0.18)).blur_radius(px(3.)).inset(),
                BoxShadow::new(px(0.), px(-1.), c.highlight(0.75)).inset(),
            ])
            .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                if *h {
                    this.hovered_provider = Some(id_hover.clone());
                } else if this.hovered_provider.as_deref() == Some(id_hover.as_str()) {
                    this.hovered_provider = None;
                    if this.confirm_remove.as_deref() == Some(id_hover.as_str()) {
                        this.confirm_remove = None;
                    }
                }
                cx.notify();
            }));

        // Removing asks first, in place of the card's content.
        if confirm {
            let (rid, cid) = (p.id.clone(), p.id.clone());
            return card
                .flex_col()
                .items_start()
                .justify_center()
                .gap(px(6.))
                .child(ui(format!("Remove {}?", p.name), 14., 18., FontWeight::SEMIBOLD, c.ink).truncate())
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .gap(px(6.))
                        .child(Button::new(SharedString::from(format!("rm-yes-{}", p.id)), "Remove").small().danger().on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.confirm_remove = None;
                                this.tests.remove(&rid);
                                let id = rid.clone();
                                this.model.update(cx, |m, cx| m.remove_provider(&id, cx));
                            },
                        )))
                        .child(Button::new(SharedString::from(format!("rm-no-{}", p.id)), "Cancel").small().ghost().on_click(cx.listener(
                            move |this, _, _, cx| {
                                if this.confirm_remove.as_deref() == Some(cid.as_str()) {
                                    this.confirm_remove = None;
                                }
                                cx.notify();
                            },
                        ))),
                )
                .into_any_element();
        }

        // The actions sit in a corner menu, so hovering never squeezes the text.
        let more = (hovered || menu_open).then(|| {
            let mid = p.id.clone();
            let mut corner = div().absolute().top(px(8.)).right(px(8.)).child(
                kit::icon_button(SharedString::from(format!("more-{}", p.id)), Icon::More, 14., cx)
                    .size(px(24.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.provider_menu = if this.provider_menu.as_deref() == Some(mid.as_str()) { None } else { Some(mid.clone()) };
                        cx.notify();
                    })),
            );
            if menu_open {
                let mut items: Vec<(SharedString, bool)> = Vec::new();
                let mut actions: Vec<&'static str> = Vec::new();
                if !needs_model && !matches!(test, Some(Test::Running)) {
                    items.push(("Test connection".into(), false));
                    actions.push("test");
                }
                if !default {
                    items.push(("Make default".into(), false));
                    actions.push("default");
                }
                items.push(("Remove".into(), false));
                actions.push("remove");
                let (pick_page, close_page) = (cx.entity(), cx.entity());
                let pid = p.id.clone();
                // The menu opens at its parent's left edge; a parent as wide as
                // the menu lines its right edge up with the button.
                corner = corner.child(div().absolute().top_full().right_0().w(px(200.)).child(kit::menu(
                    "provider-actions",
                    items,
                    move |i, _, cx| {
                        let action = actions.get(i).copied();
                        let id = pid.clone();
                        pick_page.update(cx, |this, cx| {
                            this.provider_menu = None;
                            match action {
                                Some("test") => this.test(id, cx),
                                Some("default") => {
                                    this.model.update(cx, |m, cx| m.edit_config(cx, |c| c.ai.default_provider = Some(id)));
                                }
                                Some("remove") => this.confirm_remove = Some(id),
                                _ => {}
                            }
                            cx.notify();
                        });
                    },
                    move |_, cx| {
                        close_page.update(cx, |this, cx| {
                            this.provider_menu = None;
                            cx.notify();
                        })
                    },
                    cx,
                )));
            }
            corner
        });

        let picker = self.model_picker(p, cx);
        card.child(div().flex_none().size(px(8.)).rounded_full().bg(dot))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            // Room for the corner menu button.
                            .pr(px(26.))
                            .child(ui(p.name.clone(), 14., 18., FontWeight::SEMIBOLD, c.ink).truncate())
                            .when(default, |d| d.child(ui("Default", 11., 14., FontWeight::MEDIUM, c.graphite))),
                    )
                    .child(picker)
                    .child(ui(line, 12., 16., FontWeight::NORMAL, line_color).truncate()),
            )
            .children(more)
            .into_any_element()
    }

    /// The model line of a provider card. A click opens the provider's list.
    fn model_picker(&self, p: &Provider, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let label = match p.model() {
            Some(id) => m.model_label(&p.id, id),
            None if p.needs_model() => "Choose a model".into(),
            // macOS chooses the model. Its name comes with the list.
            None if p.kind == ProviderKind::AppleIntelligence => match m.model_lists.get(&p.id) {
                Some(crate::model::ModelList::Ready(list)) if !list.is_empty() => list[0].name.clone(),
                _ => "Default model".into(),
            },
            None => "Default model".into(),
        };
        let pid = p.id.clone();
        model_picker::trigger(&format!("provider:{}", p.id), label, !p.needs_model(), cx)
            .on_click(cx.listener(move |this, _, _, cx| this.open_models(&pid, cx)))
            .into_any_element()
    }

    /// Open the model list of a saved provider under its card.
    fn open_models(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(p) = self.model.read(cx).config.ai.providers.iter().find(|p| p.id == id).cloned() else { return };
        self.model.update(cx, |m, cx| m.load_provider_models(id, false, cx));
        let page = cx.entity().downgrade();
        let model = self.model.clone();
        let (pick_id, refresh_id) = (p.id.clone(), p.id.clone());
        let request = model_picker::Request {
            key: format!("provider:{}", p.id),
            list_key: p.id.clone(),
            selected: p.model().map(str::to_string),
            extra: None,
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

    fn suggestion(&self, id: &'static str, title: &str, detail: String, provider: Provider, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        kit::dashed(id, cx)
            .justify_start()
            .gap(px(12.))
            .flex_basis(px(230.))
            .flex_grow(1.)
            .min_w(px(230.))
            .h(px(CARD_H))
            .px(px(14.))
            .rounded(px(12.))
            .cursor_default()
            .child(div().flex_none().size(px(8.)).rounded_full().border_1().border_color(c.success))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.))
                    .child(ui(title.to_string(), 14., 18., FontWeight::SEMIBOLD, c.ink))
                    .child(ui(detail, 12., 16., FontWeight::NORMAL, c.graphite).truncate()),
            )
            .child(Button::new(SharedString::from(format!("{id}-add")), "Add").small().on_click(cx.listener(move |this, _, window, cx| {
                let p = provider.clone();
                this.added(p, None, window, cx);
            })))
            .into_any_element()
    }

    fn providers(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let (providers, default, enabled, detected) = {
            let m = self.model.read(cx);
            (m.config.ai.providers.clone(), m.config.ai.default_provider.clone(), m.config.ai.enabled, m.detected.clone())
        };
        let model = self.model.clone();
        let note = div()
            .flex()
            .items_center()
            .gap(px(14.))
            .child(ui("Cloud providers receive the style prompt and your transcript. Nothing else.", 13., 16., FontWeight::NORMAL, c.graphite))
            .child(
                div().flex().items_center().gap(px(8.)).child(ui("Use AI", 13., 16., FontWeight::MEDIUM, c.ink)).child(
                    Switch::new("ai-on", enabled).on_toggle(move |on, _, cx| model.update(cx, |m, cx| m.set_ai_enabled(on, cx))),
                ),
            );
        let mut strip = div().flex().flex_wrap().gap(px(12.));
        for p in &providers {
            let is_default = default.as_deref() == Some(p.id.as_str());
            strip = strip.child(self.provider_card(p, is_default, cx));
        }
        // Providers found on this Mac that are not set up yet.
        let has_ollama = providers.iter().any(|p| matches!(&p.kind, ProviderKind::OpenAiCompatible { base_url, .. } if base_url.contains(":11434")));
        if let (Some(models), false) = (&detected.ollama, has_ollama) {
            // With no model pulled, the picker opens after Add instead.
            let first = models.first().cloned().unwrap_or_default();
            let p = Provider {
                id: self.model.read(cx).new_provider_id("ollama"),
                name: "Ollama".into(),
                kind: ProviderKind::OpenAiCompatible {
                    base_url: "http://localhost:11434/v1".into(),
                    model: first.clone(),
                    api_key_account: None,
                    zero_data_retention: false,
                },
            };
            let detail = if models.is_empty() {
                format!("Running on this {} · no models pulled yet", crate::shell::COMPUTER)
            } else {
                format!("Running on this {} · {}", crate::shell::COMPUTER, kit::plural(models.len(), "model", "models"))
            };
            strip = strip.child(self.suggestion("suggest-ollama", "Ollama", detail, p, cx));
        }
        let has_claude = providers.iter().any(|p| matches!(p.kind, ProviderKind::ClaudeCli { .. }));
        if let (Some(path), false) = (&detected.claude_cli, has_claude) {
            let p = Provider {
                id: self.model.read(cx).new_provider_id("claude"),
                name: "Claude CLI".into(),
                kind: ProviderKind::ClaudeCli { path: Some(path.display().to_string()), model: "haiku".into() },
            };
            strip = strip.child(self.suggestion("suggest-claude", "Claude CLI", format!("Found on this {} · Cloud", crate::shell::COMPUTER), p, cx));
        }
        let has_codex = providers.iter().any(|p| matches!(p.kind, ProviderKind::CodexCli { .. }));
        if let (Some(path), false) = (&detected.codex_cli, has_codex) {
            let p = Provider {
                id: self.model.read(cx).new_provider_id("codex"),
                name: "Codex CLI".into(),
                kind: ProviderKind::CodexCli { path: Some(path.display().to_string()), model: None },
            };
            strip = strip.child(self.suggestion("suggest-codex", "Codex CLI", format!("Found on this {} · Cloud · about 5 s", crate::shell::COMPUTER), p, cx));
        }
        let has_apple = providers.iter().any(|p| matches!(p.kind, ProviderKind::AppleIntelligence));
        if let (Some(model), false) = (&detected.apple_intelligence, has_apple) {
            let p = Provider {
                id: self.model.read(cx).new_provider_id("apple-intelligence"),
                name: "Apple Intelligence".into(),
                kind: ProviderKind::AppleIntelligence,
            };
            strip = strip.child(self.suggestion("suggest-apple", "Apple Intelligence", format!("Runs on this Mac · {model}"), p, cx));
        }
        strip = strip.child(
            kit::dashed("add-provider", cx)
                .w(px(140.))
                .h(px(CARD_H))
                .rounded(px(12.))
                .on_click(cx.listener(|this, _, w, cx| this.open_add(w, cx)))
                .child(icon(Icon::Plus, 12., c.graphite))
                .child(ui("Add", 14., 18., FontWeight::NORMAL, c.graphite)),
        );
        let mut col = div().flex().flex_col().gap(px(12.)).child(kit::section_head("Providers", Some(note.into_any_element()), cx).pb(px(8.)));
        if providers.is_empty() && !detected.done {
            col = col.child(ui(format!("Looking for AI providers on this {}.", crate::shell::COMPUTER), 13., 16., FontWeight::NORMAL, c.graphite));
        }
        col = col.child(strip);
        if let Some(panel) = self.add_panel(cx) {
            col = col.child(panel);
        }
        col.into_any_element()
    }

    fn open_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.add.is_some() {
            self.add = None;
            cx.notify();
            return;
        }
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("For example, OpenRouter"));
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("https://openrouter.ai/api/v1"));
        let key = cx.new(|cx| InputState::new(window, cx).placeholder(crate::shell::os_text!("Stored in your Keychain", "Stored in Windows Credential Manager", "Stored in your system keyring")).masked(true));
        self._subs.push(cx.subscribe_in(&url, window, |_, _, _: &InputEvent, _, cx| cx.notify()));
        name.update(cx, |s, cx| s.focus(window, cx));
        self.add = Some(AddProvider { kind: AddKind::OpenAi, name, url, key, error: None });
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
                    add.error = Some("Enter a name for the provider.".into());
                } else if !(url.starts_with("http://") || url.starts_with("https://")) {
                    add.error = Some("Enter a base URL that starts with http:// or https://.".into());
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
                    add.error = Some(if claude {
                        "Sayso cannot find the Claude CLI. Install it and sign in, then try again.".into()
                    } else {
                        "Sayso cannot find the Codex CLI. Install it and sign in, then try again.".into()
                    });
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
            },
            // Haiku is the fast choice for cleanup; the picker offers the rest.
            (AddKind::Claude, _) => ProviderKind::ClaudeCli { path: detected.claude_cli.map(|p| p.display().to_string()), model: "haiku".into() },
            _ => ProviderKind::CodexCli { path: detected.codex_cli.map(|p| p.display().to_string()), model: None },
        };
        let provider = Provider { id, name, kind };
        self.add = None;
        self.added(provider, key, window, cx);
    }

    /// Save a new provider and fetch its models. A provider that cannot run
    /// without a model opens its picker; one that can is tested right away.
    fn added(&mut self, provider: Provider, key: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let id = provider.id.clone();
        let needs_model = provider.needs_model();
        self.model.update(cx, |m, cx| {
            m.save_provider(provider, key, false, cx);
            m.load_provider_models(&id, true, cx);
        });
        if needs_model {
            // The card's trigger records its place when it is drawn; open after that.
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(120)).await;
                let _ = this.update(cx, |this, cx| this.open_models(&id, cx));
            })
            .detach();
        } else {
            self.test(id, cx);
        }
    }

    fn add_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = cx.paper().colors;
        let add = self.add.as_ref()?;
        let (claude, codex) = {
            let m = self.model.read(cx);
            (m.detected.claude_cli.clone(), m.detected.codex_cli.clone())
        };
        let kinds = vec!["OpenAI-compatible", "Claude CLI", "Codex CLI"];
        let selected = match add.kind {
            AddKind::OpenAi => 0,
            AddKind::Claude => 1,
            AddKind::Codex => 2,
        };
        let entity = cx.entity();
        let seg = Segmented::new("add-kind", kinds, selected).on_select(move |i, _, cx| {
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
                let mut presets = div().flex().gap(px(6.));
                for (i, (name, _)) in PRESETS.iter().enumerate() {
                    presets = presets.child(
                        Chip::new(("preset", i), *name, false).on_click(cx.listener(move |this, _, w, cx| this.apply_preset(i, w, cx))),
                    );
                }
                let url = add.url.read(cx).value().to_string();
                let cloud = !url.trim().is_empty() && !is_local_url(&url);
                body = body
                    .child(kit::field("Start from", presets, None, cx))
                    .child(
                        div()
                            .flex()
                            .gap(px(12.))
                            .child(kit::field("Name", kit::input_well(&add.name, cx), None, cx).w(px(220.)))
                            .child(kit::field("Base URL", kit::input_well(&add.url, cx), None, cx).flex_1()),
                    )
                    .child(kit::field(
                        "API key",
                        kit::input_well(&add.key, cx),
                        Some("Not needed for local servers. You choose the model from the provider's list after you add it."),
                        cx,
                    ))
                    .when(cloud, |d| {
                        d.child(Banner::new(
                            BannerKind::Info,
                            "This is a cloud provider. When a style uses it, Sayso sends the style prompt, your dictionary words, and the transcript to it.",
                        ))
                    });
            }
            AddKind::Claude | AddKind::Codex => {
                let (path, name) = if add.kind == AddKind::Claude { (claude, "Claude CLI") } else { (codex, "Codex CLI") };
                let status = match path {
                    Some(p) => format!("Found at {}. Sayso runs it with no tools and no saved session.", p.display()),
                    None => format!("Sayso cannot find the {name}. Install it and sign in, then try again."),
                };
                body = body.child(ui(status, 13., 18., FontWeight::NORMAL, c.graphite)).child(Banner::new(
                    BannerKind::Info,
                    if add.kind == AddKind::Claude {
                        "Cloud. About 1.8 s for each dictation with Haiku. The CLI uses your own account. You can choose another model after you add it."
                    } else {
                        "Cloud. About 5 s for each dictation. The CLI uses your own account. You can choose a model after you add it."
                    },
                ));
            }
        }
        let panel = div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .mt(px(6.))
            .p(px(22.))
            .rounded(px(14.))
            .raised(&c)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(text::title("Add a provider", 22., &c))
                    .child(seg),
            )
            .child(body)
            .when_some(add.error.clone(), |d, e| d.child(Banner::new(BannerKind::Warning, e)))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(Button::new("save-provider", "Add provider").primary().on_click(cx.listener(|this, _, window, cx| this.save_add(window, cx))))
                    .child(Button::new("cancel-provider", "Cancel").ghost().on_click(cx.listener(|this, _, _, cx| {
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
    if lower.contains("refused") || lower.contains("connect") || lower.contains("dns") {
        "Cannot connect. Start the server or check the URL, then test again.".into()
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "No answer in time. Test again, or raise the timeout.".into()
    } else if lower.contains("401") || lower.contains("403") || lower.contains("unauthorized") || lower.contains("api key") {
        "The provider refused the API key. Check the key, then test again.".into()
    } else if lower.contains("404") || lower.contains("model") {
        format!("The provider did not accept the model. {raw}")
    } else {
        raw.to_string()
    }
}

fn trim_float(v: f64) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

impl Render for StylesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c: Colors = cx.paper().colors;
        let (styles, active, dir) = {
            let m = self.model.read(cx);
            (m.styles.styles().cloned().collect::<Vec<_>>(), m.active_style().id, m.styles_dir_display())
        };
        let new_style = Button::new("new-style", "New style")
            .primary()
            .large()
            .icon(Icon::Plus)
            .on_click(cx.listener(|this, _, w, cx| this.open_editor(None, w, cx)))
            .into_any_element();
        let head = kit::page_head(
            "Styles",
            Some(kit::intro(
                format!("A style rewrites what you said before it is inserted. Each style is a file in {dir}, so you can edit it by hand."),
                560.,
                cx,
            )),
            Some(new_style),
            cx,
        );
        let mut grid = div().flex().flex_col().gap(px(18.));
        for chunk in styles.chunks(3) {
            let mut row = div().flex().gap(px(18.));
            for s in chunk {
                row = row.child(self.card(s, s.id == active, cx));
            }
            for _ in chunk.len()..3 {
                row = row.child(div().flex_1());
            }
            grid = grid.child(row);
        }
        let errors = self.model.read(cx).styles.errors.clone();
        let base_prompt = self.base_prompt_row(cx);
        let providers = self.providers(cx);
        let editor = self.editor_panel(cx).or_else(|| self.base_editor_panel(cx));
        let _ = window;
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
                        .gap(px(28.))
                        .py(px(44.))
                        .px(px(52.))
                        .child(head)
                        .children(errors.into_iter().map(|e| Banner::new(BannerKind::Warning, format!("A style file has an error: {e}"))))
                        .child(grid)
                        .child(base_prompt)
                        .child(providers),
                ),
            )
            .children(editor)
    }
}
