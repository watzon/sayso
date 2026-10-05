//! The dictionary page. See the Paper board "Hub — Dictionary".
//!
//! Words are raised slips. Replacements are a ledger with an inline add row.
//! The "Try a phrase" tray runs the replacements live as you type.

use super::kit::{self, caps, fraunces, mono, ui};
use crate::model::AppModel;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::dictionary::Replacement;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::fonts::{DISPLAY, UI};
use sayso_ui::paper::{self, PaperStyled};
use sayso_ui::{ActivePaper, Colors};

pub struct DictionaryPage {
    model: Entity<AppModel>,
    filter: Entity<InputState>,
    new_word: Entity<InputState>,
    adding_word: bool,
    hovered_word: Option<i64>,
    hovered_rule: Option<i64>,
    from: Entity<InputState>,
    to: Entity<InputState>,
    /// The rule being edited, with its own inputs.
    editing: Option<(i64, Entity<InputState>, Entity<InputState>)>,
    phrase: Entity<InputState>,
    error: Option<String>,
    /// The page has no room for two columns (set at each render).
    narrow: bool,
    _subs: Vec<Subscription>,
}

impl DictionaryPage {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let new_word = cx.new(|cx| InputState::new(window, cx).placeholder("New word"));
        let from = cx.new(|cx| InputState::new(window, cx).placeholder("When I say"));
        let to = cx.new(|cx| InputState::new(window, cx).placeholder("Write"));
        let phrase = cx.new(|cx| InputState::new(window, cx).placeholder("Type a phrase to see what your rules do"));
        let subs = vec![
            observe,
            cx.subscribe_in(&filter, window, |_, _, _: &InputEvent, _, cx| cx.notify()),
            cx.subscribe_in(&phrase, window, |_, _, _: &InputEvent, _, cx| cx.notify()),
            cx.subscribe_in(&new_word, window, |this: &mut Self, state, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => {
                    let text = state.read(cx).value().to_string();
                    this.add_word(&text, window, cx);
                }
                InputEvent::Blur
                    if state.read(cx).value().trim().is_empty() => {
                        this.adding_word = false;
                        cx.notify();
                    }
                _ => {}
            }),
            cx.subscribe_in(&from, window, |this: &mut Self, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.to.update(cx, |s, cx| s.focus(window, cx));
                } else {
                    this.error = None;
                }
            }),
            cx.subscribe_in(&to, window, |this: &mut Self, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.add_rule(window, cx);
                }
            }),
        ];
        Self {
            model,
            filter,
            new_word,
            adding_word: false,
            hovered_word: None,
            hovered_rule: None,
            from,
            to,
            editing: None,
            phrase,
            error: None,
            narrow: false,
            _subs: subs,
        }
    }

    fn start_word(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.adding_word = true;
        self.new_word.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    /// Close the new word field without adding anything.
    fn cancel_word(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.adding_word = false;
        self.error = None;
        self.new_word.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    fn add_word(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.model.read(cx).words.iter().any(|w| w.text.eq_ignore_ascii_case(&text)) {
            self.error = Some(format!("“{text}” is already in your words."));
            cx.notify();
            return;
        }
        self.error = None;
        self.model.update(cx, |m, cx| m.add_word(&text, cx));
        // Keep the input open so you can add several words in a row.
        self.new_word.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn add_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let from = self.from.read(cx).value().trim().to_string();
        let to = self.to.read(cx).value().to_string();
        if from.is_empty() {
            self.error = Some("Enter what you say in “When I say”.".into());
            cx.notify();
            return;
        }
        if self.model.read(cx).replacements.iter().any(|r| r.from.eq_ignore_ascii_case(&from)) {
            self.error = Some(format!("A rule for “{from}” already exists. Edit that rule instead."));
            cx.notify();
            return;
        }
        self.error = None;
        self.model.update(cx, |m, cx| m.add_replacement(&from, &to, cx));
        self.from.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        self.to.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    fn start_edit(&mut self, rule: &Replacement, window: &mut Window, cx: &mut Context<Self>) {
        let from = rule.from.clone();
        let to = rule.to.clone();
        let f = cx.new(|cx| InputState::new(window, cx).default_value(from));
        let t = cx.new(|cx| InputState::new(window, cx).default_value(to));
        let id = rule.id;
        self._subs.push(cx.subscribe_in(&t, window, move |this: &mut Self, _, ev: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.save_edit(cx);
            }
        }));
        self._subs.push(cx.subscribe_in(&f, window, move |this: &mut Self, _, ev: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.save_edit(cx);
            }
        }));
        f.update(cx, |s, cx| s.focus(window, cx));
        self.editing = Some((id, f, t));
        cx.notify();
    }

    fn save_edit(&mut self, cx: &mut Context<Self>) {
        let Some((id, f, t)) = self.editing.take() else { return };
        let from = f.read(cx).value().trim().to_string();
        let to = t.read(cx).value().to_string();
        if from.is_empty() {
            self.error = Some("A rule needs a “When I say” phrase.".into());
            self.editing = Some((id, f, t));
            cx.notify();
            return;
        }
        let rule = self.model.read(cx).replacements.iter().find(|r| r.id == id).cloned();
        if let Some(r) = rule {
            self.model.update(cx, |m, cx| m.update_replacement(&Replacement { from, to, ..r }, cx));
        }
        self.error = None;
        cx.notify();
    }

    fn words(&mut self, needle: &str, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let words: Vec<(i64, String)> = self
            .model
            .read(cx)
            .words
            .iter()
            .filter(|w| needle.is_empty() || w.text.to_lowercase().contains(needle))
            .map(|w| (w.id, w.text.clone()))
            .collect();
        let total = self.model.read(cx).words.len();
        let mut slips = div().flex().flex_wrap().gap(px(8.));
        for (id, text) in words {
            let hovered = self.hovered_word == Some(id);
            slips = slips.child(
                div()
                    .id(("word", id as u64))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .h(px(32.))
                    .pl(px(12.))
                    .pr(px(if hovered { 6. } else { 12. }))
                    .rounded(px(6.))
                    .bg(c.sheet_raised)
                    .shadow(vec![
                        BoxShadow::new(px(0.), px(1.), c.highlight(1.0)).inset(),
                        BoxShadow::new(px(0.), px(1.), c.shadow(0.16)).blur_radius(px(2.)),
                        BoxShadow::new(px(0.), px(3.), c.shadow(0.06)).blur_radius(px(8.)),
                    ])
                    .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                        if *h {
                            this.hovered_word = Some(id);
                        } else if this.hovered_word == Some(id) {
                            this.hovered_word = None;
                        }
                        cx.notify();
                    }))
                    .child(fraunces(text, 15., 18., c.ink))
                    .when(hovered, |d| {
                        d.child(
                            kit::icon_button(("rm-word", id as u64), Icon::Close, 9., cx)
                                .size(px(18.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.hovered_word = None;
                                    this.model.update(cx, |m, cx| m.remove_word(id, cx));
                                })),
                        )
                    }),
            );
        }
        if self.adding_word {
            slips = slips.child(
                div()
                    .id("new-word")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(32.))
                    .w(px(170.))
                    .pl(px(10.))
                    .pr(px(6.))
                    .rounded(px(6.))
                    .debossed(&c)
                    .font_family(DISPLAY)
                    .text_size(px(15.))
                    // Capture, so Escape closes the field before the input sees it.
                    .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        if ev.keystroke.key == "escape" {
                            cx.stop_propagation();
                            this.cancel_word(window, cx);
                        }
                    }))
                    .child(div().flex_1().min_w_0().child(kit::bare_input(&self.new_word, 15.)))
                    .child(
                        kit::icon_button("cancel-word", Icon::Close, 9., cx)
                            .size(px(18.))
                            .flex_none()
                            .on_click(cx.listener(|this, _, window, cx| this.cancel_word(window, cx))),
                    ),
            );
        } else {
            slips = slips.child(
                kit::dashed("add-word", cx)
                    .h(px(32.))
                    .px(px(12.))
                    .on_click(cx.listener(|this, _, w, cx| this.start_word(w, cx)))
                    .child(icon(Icon::Plus, 12., c.graphite))
                    .child(ui("Add word", 14., 18., FontWeight::NORMAL, c.graphite)),
            );
        }
        let note = format!("{total} · boosted in recognition");
        let mut col = div()
            .flex()
            .flex_col()
            .flex_none()
            .map(|d| if self.narrow { d.w_full() } else { d.w(px(380.)) })
            .gap(px(16.))
            .child(kit::section_head("Words", Some(ui(note, 13., 16., FontWeight::NORMAL, c.graphite).into_any_element()), cx));
        if total == 0 {
            col = col.child(ui(
                "Add names and terms that Sayso spells wrong. Sayso listens for them and spells them as you write them here.",
                14.,
                20.,
                FontWeight::NORMAL,
                c.graphite,
            ));
        }
        col.child(slips).child(self.try_tray(cx)).into_any_element()
    }

    fn try_tray(&self, cx: &App) -> impl IntoElement {
        let c = cx.paper().colors;
        let phrase = self.phrase.read(cx).value().to_string();
        let result = if phrase.trim().is_empty() {
            fraunces("The result shows here.", 15., 21., c.pencil)
        } else {
            let out = self.model.read(cx).try_phrase(&phrase).replace("\\n", "↵").replace('\n', "↵");
            fraunces(out, 15., 21., c.ink)
        };
        let rules = self.model.read(cx).replacements.len();
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .mt(px(20.))
            .p(px(18.))
            .rounded(px(14.))
            .bg(c.deboss)
            .shadow(vec![
                BoxShadow::new(px(0.), px(2.), c.shadow(0.16)).blur_radius(px(4.)).inset(),
                BoxShadow::new(px(0.), px(-1.), c.highlight(0.75)).inset(),
            ])
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(caps("Try a phrase", 12., c.graphite))
                    .child(ui(kit::plural(rules, "rule", "rules") + " apply", 13., 16., FontWeight::MEDIUM, c.graphite)),
            )
            .child(
                div()
                    .font_family(DISPLAY)
                    .text_size(px(15.))
                    .italic()
                    .text_color(c.graphite)
                    .child(kit::bare_input(&self.phrase, 15.).h(px(26.))),
            )
            .child(div().h(px(1.)).bg(c.deboss_shade))
            .child(result)
    }

    fn ledger(&mut self, needle: &str, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let rules: Vec<Replacement> = self
            .model
            .read(cx)
            .replacements
            .iter()
            .filter(|r| needle.is_empty() || r.from.to_lowercase().contains(needle) || r.to.to_lowercase().contains(needle))
            .cloned()
            .collect();
        let total = self.model.read(cx).replacements.len();
        let head_cell = |s: &str| caps(s.to_string(), 12., c.graphite);
        let mut col = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(kit::section_head(
                "Replacements",
                Some(
                    ui(format!("{} · applied before the style", kit::plural(total, "rule", "rules")), 13., 16., FontWeight::NORMAL, c.graphite)
                        .into_any_element(),
                ),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(12.))
                    .h(px(36.))
                    .child(head_cell("When I say").w(px(150.)).flex_none())
                    .child(div().w(px(18.)).flex_none())
                    .child(head_cell("Write").flex_1())
                    .child(head_cell("Used").w(px(56.)).flex_none().text_right()),
            );
        if total == 0 {
            col = col.child(
                ui("Add a rule below. For example, when you say “git hub”, Sayso writes “GitHub”.", 14., 20., FontWeight::NORMAL, c.graphite)
                    .py(px(10.)),
            );
        }
        for r in rules {
            let editing = self.editing.as_ref().filter(|e| e.0 == r.id).map(|e| (e.1.clone(), e.2.clone()));
            col = col.child(self.rule_row(&r, editing, cx));
        }
        col.child(self.add_row(cx)).into_any_element()
    }

    fn rule_row(&self, r: &Replacement, editing: Option<(Entity<InputState>, Entity<InputState>)>, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let id = r.id;
        let row = div()
            .id(("rule", id as u64))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(12.))
            .h(px(48.))
            .border_t_1()
            .border_color(c.rule);
        if let Some((f, t)) = editing {
            return row
                .child(small_input(&f, true, cx).w(px(150.)).flex_none())
                .child(icon(Icon::ArrowRight, 16., c.ink))
                .child(small_input(&t, false, cx).flex_1())
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .w(px(56.))
                        .justify_end()
                        .gap(px(2.))
                        .child(kit::icon_button("save-rule", Icon::Check, 12., cx).on_click(cx.listener(|this, _, _, cx| this.save_edit(cx))))
                        .child(kit::icon_button("cancel-rule", Icon::Close, 10., cx).on_click(cx.listener(|this, _, _, cx| {
                            this.editing = None;
                            this.error = None;
                            cx.notify();
                        }))),
                )
                .into_any_element();
        }
        let hovered = self.hovered_rule == Some(id);
        let breaks = r.to.matches("\\n").count() + r.to.matches('\n').count();
        let to = if breaks > 0 && r.to.replace("\\n", "").replace('\n', "").trim().is_empty() {
            div().flex().flex_1().min_w_0().child(
                div()
                    .flex()
                    .items_center()
                    .h(px(22.))
                    .px(px(8.))
                    .rounded(px(5.))
                    .bg(c.deboss)
                    .shadow(vec![BoxShadow::new(px(0.), px(1.), c.shadow(0.16)).blur_radius(px(2.)).inset()])
                    .child(mono(vec!["↵"; breaks].join(" "), 12., c.ink)),
            )
        } else {
            fraunces(r.to.replace("\\n", "↵").replace('\n', "↵"), 15., 18., c.ink).flex_1().min_w_0().truncate()
        };
        let rule = r.clone();
        row.cursor_pointer()
            .hover(|s| s.bg(c.deboss.opacity(0.3)))
            .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                if *h {
                    this.hovered_rule = Some(id);
                } else if this.hovered_rule == Some(id) {
                    this.hovered_rule = None;
                }
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _, w, cx| this.start_edit(&rule, w, cx)))
            .child(fraunces(r.from.clone(), 15., 18., c.graphite).italic().w(px(150.)).flex_none().truncate())
            .child(icon(Icon::ArrowRight, 16., c.ink).w(px(18.)))
            .child(to)
            .child(div().flex().flex_none().w(px(56.)).justify_end().map(|d| {
                if hovered {
                    d.child(kit::icon_button(("rm-rule", id as u64), Icon::Trash, 13., cx).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.hovered_rule = None;
                        this.model.update(cx, |m, cx| m.remove_replacement(id, cx));
                    })))
                } else {
                    d.child(mono(r.uses.to_string(), 12., c.graphite))
                }
            }))
            .into_any_element()
    }

    fn add_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let mut col = div().flex().flex_col().child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(12.))
                .h(px(52.))
                .border_t_1()
                .border_b_1()
                .border_color(c.rule)
                .child(small_input(&self.from, true, cx).w(px(150.)).flex_none())
                .child(icon(Icon::ArrowRight, 16., c.pencil).w(px(18.)))
                .child(small_input(&self.to, false, cx).flex_1())
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .w(px(56.))
                        .justify_end()
                        .child(Button::new("add-rule", "Add").small().on_click(cx.listener(|this, _, w, cx| this.add_rule(w, cx)))),
                ),
        );
        if let Some(e) = &self.error {
            col = col.child(ui(e.clone(), 13., 18., FontWeight::NORMAL, c.danger).pt(px(8.)));
        }
        col
    }
}

/// A compact input in a debossed well for the ledger.
fn small_input(state: &Entity<InputState>, italic: bool, cx: &App) -> Div {
    let c = cx.paper().colors;
    div()
        .flex()
        .items_center()
        .h(px(32.))
        .px(px(10.))
        .rounded(px(7.))
        .bg(c.deboss)
        .shadow(paper::debossed(&c))
        .font_family(if italic { DISPLAY } else { UI })
        .text_size(px(14.))
        .when(italic, |d| d.italic())
        .child(kit::bare_input(state, 14.))
}

impl Render for DictionaryPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c: Colors = cx.paper().colors;
        let pad = crate::layout::page_pad(window, &self.model.read(cx).config);
        self.narrow = crate::layout::page_narrow(window, &self.model.read(cx).config);
        let narrow = self.narrow;
        let needle = self.filter.read(cx).value().trim().to_lowercase();
        let add = Button::new("add", "Add")
            .primary()
            .large()
            .icon(Icon::Plus)
            .on_click(cx.listener(|this, _, w, cx| this.start_word(w, cx)));
        let actions = div()
            .flex()
            .gap(px(8.))
            .child(kit::search_well(&self.filter, 36., None, cx).w(px(220.)).gap(px(8.)))
            .child(add)
            .into_any_element();
        let head = kit::page_head(
            "Dictionary",
            Some(kit::intro("Words help Sayso hear names and terms. Replacements change the text after it is transcribed.", 520., cx)),
            Some(actions),
            cx,
        );
        let words = self.words(&needle, cx);
        let ledger = self.ledger(&needle, cx);
        div().id("dictionary").absolute().inset_0().overflow_y_scroll().text_color(c.ink).child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(px(32.))
                .py(px(pad.min(44.)))
                .px(px(pad))
                .child(head)
                // A narrow page: the words go above the replacements.
                .child(if narrow {
                    div().flex().flex_col().w_full().gap(px(32.)).child(words).child(ledger)
                } else {
                    div().flex().w_full().gap(px(44.)).child(words).child(ledger)
                }),
        )
    }
}
