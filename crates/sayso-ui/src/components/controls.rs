//! Interactive controls: buttons, switches, segmented controls, nav items.

use super::{OnClick, basics::icon};
use crate::ActivePaper;
use crate::a11y::{A11yExt as _, Pressable, Role, Toggled};
use crate::assets::Icon;
use crate::fonts::{MONO, UI};
use crate::paper::{self, PaperStyled};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// A tooltip with one line of text, for a control that shows only an icon.
pub fn text_tooltip(label: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let label: SharedString = label.into();
    move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx)
}

type OnBool = std::rc::Rc<dyn Fn(bool, &mut Window, &mut App)>;
type OnIndex = std::rc::Rc<dyn Fn(usize, &mut Window, &mut App)>;
type OnDelta = std::rc::Rc<dyn Fn(i32, &mut Window, &mut App)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonKind {
    /// Ink fill. One per view.
    Primary,
    /// Raised paper.
    #[default]
    Secondary,
    /// Text only.
    Ghost,
    /// Raised paper, danger text.
    Danger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonSize {
    Small,
    #[default]
    Medium,
    Large,
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    /// Text after the label in graphite: "System English".
    detail: Option<SharedString>,
    kind: ButtonKind,
    size: ButtonSize,
    leading: Option<Icon>,
    trailing: Option<Icon>,
    disabled: bool,
    full_width: bool,
    /// The name for a screen reader when the label is not enough: "Delete Parakeet".
    a11y_label: Option<SharedString>,
    expanded: Option<bool>,
    on_click: Option<OnClick>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: None,
            kind: ButtonKind::Secondary,
            size: ButtonSize::Medium,
            leading: None,
            trailing: None,
            disabled: false,
            full_width: false,
            a11y_label: None,
            expanded: None,
            on_click: None,
        }
    }
    /// The name that a screen reader reads, in place of the visible label.
    pub fn a11y_label(mut self, label: impl Into<SharedString>) -> Self {
        self.a11y_label = Some(label.into());
        self
    }
    /// For a button that opens a menu or a popup: is it open now?
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = Some(expanded);
        self
    }
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn primary(mut self) -> Self {
        self.kind = ButtonKind::Primary;
        self
    }
    pub fn ghost(mut self) -> Self {
        self.kind = ButtonKind::Ghost;
        self
    }
    pub fn danger(mut self) -> Self {
        self.kind = ButtonKind::Danger;
        self
    }
    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = kind;
        self
    }
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }
    pub fn small(self) -> Self {
        self.size(ButtonSize::Small)
    }
    pub fn large(self) -> Self {
        self.size(ButtonSize::Large)
    }
    pub fn icon(mut self, icon: Icon) -> Self {
        self.leading = Some(icon);
        self
    }
    pub fn trailing_icon(mut self, icon: Icon) -> Self {
        self.trailing = Some(icon);
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(std::rc::Rc::new(f));
        self
    }
}

impl Button {
    /// The control as a [`Pressable`]; `render` draws it. Tests read its node.
    #[doc(hidden)]
    pub fn pressable(self, cx: &App) -> Pressable {
        let c = cx.paper().colors;
        let (h, pad, text, radius) = match self.size {
            ButtonSize::Small => (28., 12., 13., 8.),
            ButtonSize::Medium => (32., 14., 13., 8.),
            ButtonSize::Large => (38., 18., 14., 10.),
        };
        let (fg, weight) = match self.kind {
            ButtonKind::Primary => (c.on_ink, FontWeight::SEMIBOLD),
            ButtonKind::Secondary => (c.ink, FontWeight::MEDIUM),
            ButtonKind::Ghost => (c.graphite, FontWeight::MEDIUM),
            ButtonKind::Danger => (c.danger, FontWeight::MEDIUM),
        };
        let kind = self.kind;
        let name = self.a11y_label.clone().unwrap_or_else(|| match &self.detail {
            Some(d) => format!("{}, {d}", self.label).into(),
            None => self.label.clone(),
        });
        let mut b = Pressable::new(self.id)
            .label(name)
            .disabled(self.disabled)
            .when_some(self.expanded, |b, open| b.aria_expanded(open))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .h(px(h))
            .px(px(pad))
            .rounded(px(radius))
            .font_family(UI)
            .text_size(px(text))
            .font_weight(weight)
            .text_color(fg)
            .when(self.full_width, |b| b.w_full())
            .when_some(self.leading, |b, i| b.child(icon(i, 14., fg)))
            .child(self.label)
            .when_some(self.detail, |b, d| b.child(div().font_weight(FontWeight::NORMAL).text_color(c.graphite).child(d)))
            .when_some(self.trailing, |b, i| b.child(icon(i, 14., fg)));
        b = match kind {
            ButtonKind::Primary => b
                .bg(c.ink_fill)
                .shadows(paper::ink_button(&c))
                .hover(|s| s.bg(c.ink_fill.opacity(0.92)))
                .active(|s| s.shadow(paper::ink_pressed(&c))),
            ButtonKind::Secondary | ButtonKind::Danger => b
                .bg(c.sheet_raised)
                .shadows(paper::raised_small(&c))
                .hover(|s| s.bg(c.sheet))
                .active(|s| s.bg(c.deboss).shadow(paper::debossed(&c))),
            ButtonKind::Ghost => b.hover(|s| s.bg(c.deboss.opacity(0.6))).active(|s| s.bg(c.deboss)),
        };
        if self.disabled {
            b = b.opacity(0.45);
        } else if let Some(handler) = self.on_click {
            b = b.cursor_pointer().on_click(move |e, w, cx| handler(e, w, cx));
        }
        b
    }
}

impl RenderOnce for Button {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        self.pressable(cx)
    }
}

/// The paper-tab switch: an ink track when on, a debossed track when off.
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    on: bool,
    label: Option<SharedString>,
    disabled: bool,
    on_toggle: Option<OnBool>,
}

impl Switch {
    pub fn new(id: impl Into<ElementId>, on: bool) -> Self {
        Self { id: id.into(), on, label: None, disabled: false, on_toggle: None }
    }
    /// The name that a screen reader reads: the title of the settings row.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn on_toggle(mut self, f: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(std::rc::Rc::new(f));
        self
    }
}

impl Switch {
    /// The control as a [`Pressable`]; `render` draws it. Tests read its node.
    #[doc(hidden)]
    pub fn pressable(self, cx: &App) -> Pressable {
        let c = cx.paper().colors;
        let on = self.on;
        let knob = div()
            .size(px(20.))
            .rounded_full()
            .bg(if c.is_dark() && on { c.on_ink } else { c.sheet_raised })
            .shadow(vec![
                BoxShadow::new(px(0.), px(1.), c.highlight(1.0)).inset(),
                BoxShadow::new(px(0.), px(1.), c.shadow(if on { 0.35 } else { 0.3 })).blur_radius(px(2.)),
            ]);
        let mut track = Pressable::new(self.id)
            .a11y_role(Role::Switch)
            .when_some(self.label, |t, l| t.label(l))
            .disabled(self.disabled)
            .aria_toggled(if on { Toggled::True } else { Toggled::False })
            .flex()
            .flex_none()
            .items_center()
            .w(px(46.))
            .h(px(26.))
            .p(px(3.))
            .rounded_full()
            .cursor_pointer()
            .child(knob);
        track = if on {
            track.justify_end().bg(c.ink_fill).shadows(vec![
                BoxShadow::new(px(0.), px(1.), gpui_kit::black().opacity(0.45)).blur_radius(px(3.)).inset(),
            ])
        } else {
            track.justify_start().bg(c.deboss).shadows(vec![
                BoxShadow::new(px(0.), px(1.), c.shadow(0.3)).blur_radius(px(3.)).inset(),
                BoxShadow::new(px(0.), px(-1.), c.highlight(0.7)).inset(),
            ])
        };
        if self.disabled {
            track = track.opacity(0.45);
        }
        if let Some(f) = self.on_toggle {
            track = track.on_click(move |_, w, cx| f(!on, w, cx));
        }
        track
    }
}

impl RenderOnce for Switch {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        self.pressable(cx)
    }
}

/// A debossed track with one raised segment for the selection.
#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    options: Vec<(SharedString, Option<Icon>)>,
    selected: usize,
    compact: bool,
    rail: bool,
    label: Option<SharedString>,
    on_select: Option<OnIndex>,
}

impl Segmented {
    pub fn new(id: impl Into<ElementId>, options: impl IntoIterator<Item = impl Into<SharedString>>, selected: usize) -> Self {
        Self {
            id: id.into(),
            options: options.into_iter().map(|o| (o.into(), None)).collect(),
            selected,
            compact: false,
            rail: false,
            label: None,
            on_select: None,
        }
    }
    /// The name of the group: the title of the settings row.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }
    pub fn with_icons(mut self, icons: impl IntoIterator<Item = Icon>) -> Self {
        for (opt, ic) in self.options.iter_mut().zip(icons) {
            opt.1 = Some(ic);
        }
        self
    }
    /// The small sidebar version that fills its width.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }
    /// The rail version: a column of icons. Each label is a tooltip.
    pub fn rail(mut self) -> Self {
        self.rail = true;
        self
    }
    pub fn on_select(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(std::rc::Rc::new(f));
        self
    }
}

impl RenderOnce for Segmented {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.paper().colors;
        let (h, text) = if self.compact { (28., 12.) } else { (30., 13.) };
        let count = self.options.len();
        let selected_index = self.selected;
        // The arrow keys move the selection and the focus, as in a group of
        // radio buttons. Each segment is a tab stop, so the focus moves with
        // `focus_next` and `focus_prev`.
        let keys = self.on_select.clone();
        let mut track = div()
            .id(self.id)
            .role(Role::RadioGroup)
            .when_some(self.label, |t, l| t.aria_label(l))
            .flex()
            .p(px(3.))
            .gap(px(2.))
            .rounded(px(10.))
            .debossed(&c)
            .when_some(keys, move |t, f| {
                t.on_key_down(move |e, w, cx| {
                    let next = match e.keystroke.key.as_str() {
                        "right" | "down" if selected_index + 1 < count => selected_index + 1,
                        "left" | "up" if selected_index > 0 => selected_index - 1,
                        _ => return,
                    };
                    cx.stop_propagation();
                    f(next, w, cx);
                    if next > selected_index { w.focus_next(cx) } else { w.focus_prev(cx) }
                })
            });
        if self.compact {
            track = track.w_full();
        }
        if self.rail {
            track = track.flex_col();
        }
        for (i, (label, ic)) in self.options.into_iter().enumerate() {
            let selected = i == self.selected;
            let fg = if selected { c.ink } else { c.graphite };
            let mut seg = Pressable::new(i)
                .a11y_radio(label.clone(), selected, i, count)
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .h(px(h))
                .px(px(14.))
                .rounded(px(7.))
                .font_family(UI)
                .text_size(px(text))
                .font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                .text_color(fg)
                .cursor_pointer()
                .when_some(ic, |s, ic| s.child(icon(ic, 14., fg)));
            seg = if self.rail { seg.w(px(30.)).px_0().tooltip(text_tooltip(label)) } else { seg.child(label) };
            if self.compact {
                seg = seg.flex_1().px(px(4.));
            }
            if selected {
                seg = seg.bg(c.sheet_raised).shadows(vec![
                    BoxShadow::new(px(0.), px(1.), c.highlight(1.0)).inset(),
                    BoxShadow::new(px(0.), px(1.), c.shadow(0.18)).blur_radius(px(2.)),
                ]);
            } else if let Some(f) = self.on_select.clone() {
                seg = seg.hover(|s| s.text_color(c.ink)).on_click(move |_, w, cx| f(i, w, cx));
            }
            track = track.child(seg);
        }
        track
    }
}

/// A sidebar navigation row. The active row is pressed into the paper.
#[derive(IntoElement)]
pub struct NavItem {
    id: ElementId,
    icon: Icon,
    label: SharedString,
    active: bool,
    icon_only: bool,
    position: Option<(usize, usize)>,
    on_click: Option<OnClick>,
}

/// A navigation row is a tab: macOS reads "Home, selected, tab 1 of 6".
/// Put the rows in a container with [`Role::TabList`].
impl NavItem {
    pub fn new(id: impl Into<ElementId>, icon: Icon, label: impl Into<SharedString>, active: bool) -> Self {
        Self { id: id.into(), icon, label: label.into(), active, icon_only: false, position: None, on_click: None }
    }
    /// The place of this row in its list: `index` of `count`.
    pub fn position(mut self, index: usize, count: usize) -> Self {
        self.position = Some((index, count));
        self
    }
    /// The rail version: a square with the icon. The label is a tooltip.
    pub fn icon_only(mut self, icon_only: bool) -> Self {
        self.icon_only = icon_only;
        self
    }
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(std::rc::Rc::new(f));
        self
    }
}

impl NavItem {
    /// The control as a [`Pressable`]; `render` draws it. Tests read its node.
    #[doc(hidden)]
    pub fn pressable(self, cx: &App) -> Pressable {
        let c = cx.paper().colors;
        let fg = if self.active { c.ink } else { c.graphite };
        let mut row = Pressable::new(self.id)
            .a11y_role(Role::Tab)
            .label(self.label.clone())
            .aria_selected(self.active)
            .when_some(self.position, |r, (i, n)| r.aria_position_in_set(i + 1).aria_size_of_set(n))
            .flex()
            .items_center()
            .gap(px(12.))
            .h(px(36.))
            .px(px(10.))
            .rounded(px(9.))
            .font_family(UI)
            .text_size(px(14.))
            .font_weight(if self.active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
            .text_color(fg)
            .child(icon(self.icon, 18., fg));
        row = if self.icon_only { row.w(px(36.)).px_0().justify_center().tooltip(text_tooltip(self.label)) } else { row.child(self.label) };
        if self.active {
            row = row.bg(c.deboss).shadows(vec![
                BoxShadow::new(px(0.), px(1.), c.shadow(0.22)).blur_radius(px(2.5)).inset(),
                BoxShadow::new(px(0.), px(-1.), c.highlight(0.7)).inset(),
            ]);
        } else {
            row = row.cursor_pointer().hover(|s| s.bg(c.deboss.opacity(0.55)).text_color(c.ink));
            if let Some(f) = self.on_click {
                row = row.on_click(move |e, w, cx| f(e, w, cx));
            }
        }
        row
    }
}

impl RenderOnce for NavItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        self.pressable(cx)
    }
}

/// A debossed track with an ink fill.
#[derive(IntoElement)]
pub struct Progress {
    fraction: f32,
    height: f32,
    label: Option<SharedString>,
}

impl Progress {
    pub fn new(fraction: f32) -> Self {
        Self { fraction: fraction.clamp(0.0, 1.0), height: 6., label: None }
    }
    /// What the bar measures: "Downloading Parakeet". Without a label the bar
    /// has no node, for a bar that only repeats text next to it.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
}

impl RenderOnce for Progress {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.paper().colors;
        let percent = (self.fraction * 100.).round() as f64;
        let id = crate::a11y::call_site_id(self.label.as_deref().unwrap_or("progress"));
        div()
            .id(id)
            .when_some(self.label, |d, label| {
                d.role(Role::ProgressIndicator)
                    .aria_label(label)
                    .aria_min_numeric_value(0.)
                    .aria_max_numeric_value(100.)
                    .aria_numeric_value(percent)
            })
            .flex()
            .w_full()
            .h(px(self.height))
            .rounded_full()
            .bg(c.deboss)
            .shadow(vec![BoxShadow::new(px(0.), px(1.), c.shadow(0.25)).blur_radius(px(2.)).inset()])
            .child(div().h_full().w(relative(self.fraction)).rounded_full().bg(c.ink_fill))
    }
}

/// The − value + stepper. For a screen reader it is one spin button: the
/// value is its text, Up and Down change it, and so do the Increment and
/// Decrement actions. − and + are for the pointer and are not tab stops.
#[derive(IntoElement)]
pub struct Stepper {
    id: ElementId,
    label: Option<SharedString>,
    value: SharedString,
    on_change: Option<OnDelta>,
}

impl Stepper {
    pub fn new(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Self {
        Self { id: id.into(), label: None, value: value.into(), on_change: None }
    }
    /// The name that a screen reader reads: the title of the settings row.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }
    /// Called with -1 or +1.
    pub fn on_change(mut self, f: impl Fn(i32, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(std::rc::Rc::new(f));
        self
    }
}

impl Stepper {
    /// The control as a [`Pressable`]; `render` draws it. Tests read its node.
    #[doc(hidden)]
    pub fn pressable(self, cx: &App) -> Pressable {
        let c = cx.paper().colors;
        let step = |id: &'static str, glyph: &'static str, name: SharedString, delta: i32, f: Option<OnDelta>| {
            Pressable::new(id)
                .label(name)
                .tab_stop(false)
                .flex()
                .items_center()
                .justify_center()
                .size(px(26.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(15.))
                .text_color(c.graphite)
                .hover(|s| s.bg(c.sheet_raised).text_color(c.ink))
                .child(glyph)
                .when_some(f, move |d, f| d.on_click(move |_, w, cx| f(delta, w, cx)))
        };
        let f = self.on_change.clone();
        let (inc, dec) = (f.clone(), f.clone());
        Pressable::new(self.id)
            .a11y_role(Role::SpinButton)
            .when_some(self.label, |s, l| s.label(l))
            .aria_value(self.value.clone())
            .when_some(inc, |s, f| s.on_a11y_action(gpui_kit::AccessibleAction::Increment, move |_, w, cx| f(1, w, cx)))
            .when_some(dec, |s, f| s.on_a11y_action(gpui_kit::AccessibleAction::Decrement, move |_, w, cx| f(-1, w, cx)))
            .when_some(f, |s, f| {
                s.on_key_down(move |e, w, cx| {
                    let delta = match e.keystroke.key.as_str() {
                        "up" | "right" => 1,
                        "down" | "left" => -1,
                        _ => return,
                    };
                    cx.stop_propagation();
                    f(delta, w, cx);
                })
            })
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .h(px(32.))
            .px(px(3.))
            .rounded(px(9.))
            .bg(c.deboss)
            .shadows(paper::debossed(&c))
            .child(step("dec", "−", crate::l10n::text("a11y.decrease").into(), -1, self.on_change.clone()))
            .child(
                div()
                    .min_w(px(40.))
                    .px(px(4.))
                    .flex()
                    .justify_center()
                    .whitespace_nowrap()
                    .font_family(MONO)
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c.ink)
                    .child(self.value),
            )
            .child(step("inc", "+", crate::l10n::text("a11y.increase").into(), 1, self.on_change))
    }
}

impl RenderOnce for Stepper {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        self.pressable(cx)
    }
}

/// A radio dot: ink with a paper center when selected, debossed otherwise.
pub fn radio_dot(selected: bool, cx: &App) -> Div {
    let c = cx.paper().colors;
    if selected {
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(18.))
            .rounded_full()
            .bg(c.ink_fill)
            .child(div().size(px(6.)).rounded_full().bg(c.on_ink))
    } else {
        div()
            .flex_none()
            .size(px(18.))
            .rounded_full()
            .bg(c.deboss)
            .shadow(vec![BoxShadow::new(px(0.), px(1.), c.shadow(0.3)).blur_radius(px(2.)).inset()])
    }
}

/// A selectable card: an ink ring when selected.
pub fn option_card(id: impl Into<ElementId>, selected: bool, cx: &App) -> Stateful<Div> {
    let c = cx.paper().colors;
    let base = div().id(id).flex().flex_col().rounded(px(14.)).bg(c.sheet_raised).cursor_pointer();
    if selected {
        let mut shadows = vec![BoxShadow::new(px(0.), px(0.), c.ink).spread_radius(px(2.))];
        shadows.extend(paper::raised(&c));
        base.shadow(shadows)
    } else {
        base.shadow(paper::raised(&c)).hover(|s| s.bg(c.sheet))
    }
}

/// What a chip does, for a screen reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChipRole {
    /// A command, such as a preset. A plain button.
    #[default]
    Command,
    /// A filter that is on or off by itself. A toggle button.
    Filter,
    /// One choice of a set: `index` of `count`. A radio button.
    Choice { index: usize, count: usize },
    /// Opens a menu. A button with an expanded state.
    Menu { open: bool },
}

/// A filter chip: ink when selected, raised paper otherwise.
#[derive(IntoElement)]
pub struct Chip {
    id: ElementId,
    label: SharedString,
    selected: bool,
    dot: Option<Hsla>,
    trailing: Option<Icon>,
    role: ChipRole,
    a11y_label: Option<SharedString>,
    on_click: Option<OnClick>,
}

impl Chip {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>, selected: bool) -> Self {
        Self { id: id.into(), label: label.into(), selected, dot: None, trailing: None, role: ChipRole::Command, a11y_label: None, on_click: None }
    }
    pub fn role(mut self, role: ChipRole) -> Self {
        self.role = role;
        self
    }
    /// The name that a screen reader reads, in place of the visible label.
    pub fn a11y_label(mut self, label: impl Into<SharedString>) -> Self {
        self.a11y_label = Some(label.into());
        self
    }
    pub fn dot(mut self, color: Hsla) -> Self {
        self.dot = Some(color);
        self
    }
    pub fn trailing(mut self, icon: Icon) -> Self {
        self.trailing = Some(icon);
        self
    }
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(std::rc::Rc::new(f));
        self
    }
}

impl Chip {
    /// The control as a [`Pressable`]; `render` draws it. Tests read its node.
    #[doc(hidden)]
    pub fn pressable(self, cx: &App) -> Pressable {
        let c = cx.paper().colors;
        let fg = if self.selected && self.dot.is_none() { c.on_ink } else if self.selected { c.ink } else { c.graphite };
        let name = self.a11y_label.unwrap_or_else(|| self.label.clone());
        let mut chip = Pressable::new(self.id).flex();
        chip = match self.role {
            ChipRole::Command => chip.label(name),
            ChipRole::Filter => chip.label(name).aria_toggled(if self.selected { Toggled::True } else { Toggled::False }),
            ChipRole::Choice { index, count } => chip.a11y_radio(name, self.selected, index, count),
            ChipRole::Menu { open } => chip.label(name).aria_expanded(open),
        };
        let mut chip = chip
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(11.))
            .rounded_full()
            .font_family(UI)
            .text_size(px(13.))
            .font_weight(if self.selected { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
            .text_color(fg)
            .cursor_pointer()
            .when_some(self.dot, |d, dot| d.child(div().size(px(7.)).rounded_full().bg(dot)))
            .child(self.label)
            .when_some(self.trailing, |d, i| d.child(icon(i, 10., fg)));
        chip = match (self.selected, self.dot.is_some()) {
            // A style chip: selected shows an ink ring on paper.
            (true, true) => chip.bg(c.sheet_raised).shadows(vec![
                BoxShadow::new(px(0.), px(0.), c.ink).spread_radius(px(1.5)),
                BoxShadow::new(px(0.), px(1.), c.shadow(0.16)).blur_radius(px(2.)),
            ]),
            (true, false) => chip.bg(c.ink_fill).shadows(vec![
                BoxShadow::new(px(0.), px(-1.5), gpui_kit::black().opacity(0.35)).blur_radius(px(2.)).inset(),
            ]),
            (false, _) => chip.bg(c.sheet_raised).shadows(paper::raised_small(&c)).hover(|s| s.text_color(c.ink)),
        };
        if let Some(f) = self.on_click {
            chip = chip.on_click(move |e, w, cx| f(e, w, cx));
        }
        chip
    }
}

impl RenderOnce for Chip {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        self.pressable(cx)
    }
}
