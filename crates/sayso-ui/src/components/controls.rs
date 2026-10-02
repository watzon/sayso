//! Interactive controls: buttons, switches, segmented controls, nav items.

use super::{OnClick, basics::icon};
use crate::ActivePaper;
use crate::assets::Icon;
use crate::fonts::{MONO, UI};
use crate::paper::{self, PaperStyled};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

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
    kind: ButtonKind,
    size: ButtonSize,
    leading: Option<Icon>,
    trailing: Option<Icon>,
    disabled: bool,
    full_width: bool,
    on_click: Option<OnClick>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: ButtonKind::Secondary,
            size: ButtonSize::Medium,
            leading: None,
            trailing: None,
            disabled: false,
            full_width: false,
            on_click: None,
        }
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

impl RenderOnce for Button {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
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
        let mut b = div()
            .id(self.id)
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
            .when_some(self.trailing, |b, i| b.child(icon(i, 14., fg)));
        b = match kind {
            ButtonKind::Primary => b
                .ink_button(&c)
                .hover(|s| s.bg(c.ink_fill.opacity(0.92)))
                .active(|s| s.shadow(paper::ink_pressed(&c))),
            ButtonKind::Secondary | ButtonKind::Danger => b
                .bg(c.sheet_raised)
                .shadow(paper::raised_small(&c))
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

/// The paper-tab switch: an ink track when on, a debossed track when off.
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    on: bool,
    on_toggle: Option<OnBool>,
}

impl Switch {
    pub fn new(id: impl Into<ElementId>, on: bool) -> Self {
        Self { id: id.into(), on, on_toggle: None }
    }
    pub fn on_toggle(mut self, f: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(std::rc::Rc::new(f));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
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
        let mut track = div()
            .id(self.id)
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
            track.justify_end().bg(c.ink_fill).shadow(vec![
                BoxShadow::new(px(0.), px(1.), gpui_kit::black().opacity(0.45)).blur_radius(px(3.)).inset(),
            ])
        } else {
            track.justify_start().bg(c.deboss).shadow(vec![
                BoxShadow::new(px(0.), px(1.), c.shadow(0.3)).blur_radius(px(3.)).inset(),
                BoxShadow::new(px(0.), px(-1.), c.highlight(0.7)).inset(),
            ])
        };
        if let Some(f) = self.on_toggle {
            track = track.on_click(move |_, w, cx| f(!on, w, cx));
        }
        track
    }
}

/// A debossed track with one raised segment for the selection.
#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    options: Vec<(SharedString, Option<Icon>)>,
    selected: usize,
    compact: bool,
    on_select: Option<OnIndex>,
}

impl Segmented {
    pub fn new(id: impl Into<ElementId>, options: impl IntoIterator<Item = impl Into<SharedString>>, selected: usize) -> Self {
        Self {
            id: id.into(),
            options: options.into_iter().map(|o| (o.into(), None)).collect(),
            selected,
            compact: false,
            on_select: None,
        }
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
    pub fn on_select(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(std::rc::Rc::new(f));
        self
    }
}

impl RenderOnce for Segmented {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.paper().colors;
        let (h, text) = if self.compact { (28., 12.) } else { (30., 13.) };
        let mut track = div().id(self.id).flex().p(px(3.)).gap(px(2.)).rounded(px(10.)).debossed(&c);
        if self.compact {
            track = track.w_full();
        }
        for (i, (label, ic)) in self.options.into_iter().enumerate() {
            let selected = i == self.selected;
            let fg = if selected { c.ink } else { c.graphite };
            let mut seg = div()
                .id(i)
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
                .when_some(ic, |s, ic| s.child(icon(ic, 14., fg)))
                .child(label);
            if self.compact {
                seg = seg.flex_1().px(px(4.));
            }
            if selected {
                seg = seg.bg(c.sheet_raised).shadow(vec![
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
    on_click: Option<OnClick>,
}

impl NavItem {
    pub fn new(id: impl Into<ElementId>, icon: Icon, label: impl Into<SharedString>, active: bool) -> Self {
        Self { id: id.into(), icon, label: label.into(), active, on_click: None }
    }
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(std::rc::Rc::new(f));
        self
    }
}

impl RenderOnce for NavItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.paper().colors;
        let fg = if self.active { c.ink } else { c.graphite };
        let mut row = div()
            .id(self.id)
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
            .child(icon(self.icon, 18., fg))
            .child(self.label);
        if self.active {
            row = row.bg(c.deboss).shadow(vec![
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

/// A debossed track with an ink fill.
#[derive(IntoElement)]
pub struct Progress {
    fraction: f32,
    height: f32,
}

impl Progress {
    pub fn new(fraction: f32) -> Self {
        Self { fraction: fraction.clamp(0.0, 1.0), height: 6. }
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
}

impl RenderOnce for Progress {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.paper().colors;
        div()
            .flex()
            .w_full()
            .h(px(self.height))
            .rounded_full()
            .bg(c.deboss)
            .shadow(vec![BoxShadow::new(px(0.), px(1.), c.shadow(0.25)).blur_radius(px(2.)).inset()])
            .child(div().h_full().w(relative(self.fraction)).rounded_full().bg(c.ink_fill))
    }
}

/// The − value + stepper.
#[derive(IntoElement)]
pub struct Stepper {
    id: ElementId,
    value: SharedString,
    on_change: Option<OnDelta>,
}

impl Stepper {
    pub fn new(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Self {
        Self { id: id.into(), value: value.into(), on_change: None }
    }
    /// Called with -1 or +1.
    pub fn on_change(mut self, f: impl Fn(i32, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(std::rc::Rc::new(f));
        self
    }
}

impl RenderOnce for Stepper {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.paper().colors;
        let step = |id: &'static str, label: &'static str, delta: i32, f: Option<OnDelta>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(26.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(15.))
                .text_color(c.graphite)
                .hover(|s| s.bg(c.sheet_raised).text_color(c.ink))
                .child(label)
                .when_some(f, move |d, f| d.on_click(move |_, w, cx| f(delta, w, cx)))
        };
        div()
            .id(self.id)
            .flex()
            .items_center()
            .gap(px(2.))
            .h(px(32.))
            .px(px(3.))
            .rounded(px(9.))
            .debossed(&c)
            .child(step("dec", "−", -1, self.on_change.clone()))
            .child(
                div()
                    .w(px(44.))
                    .text_center()
                    .font_family(MONO)
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c.ink)
                    .child(self.value),
            )
            .child(step("inc", "+", 1, self.on_change))
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

/// A filter chip: ink when selected, raised paper otherwise.
#[derive(IntoElement)]
pub struct Chip {
    id: ElementId,
    label: SharedString,
    selected: bool,
    dot: Option<Hsla>,
    trailing: Option<Icon>,
    on_click: Option<OnClick>,
}

impl Chip {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>, selected: bool) -> Self {
        Self { id: id.into(), label: label.into(), selected, dot: None, trailing: None, on_click: None }
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

impl RenderOnce for Chip {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.paper().colors;
        let fg = if self.selected && self.dot.is_none() { c.on_ink } else if self.selected { c.ink } else { c.graphite };
        let mut chip = div()
            .id(self.id)
            .flex()
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
            (true, true) => chip.bg(c.sheet_raised).shadow(vec![
                BoxShadow::new(px(0.), px(0.), c.ink).spread_radius(px(1.5)),
                BoxShadow::new(px(0.), px(1.), c.shadow(0.16)).blur_radius(px(2.)),
            ]),
            (true, false) => chip.bg(c.ink_fill).shadow(vec![
                BoxShadow::new(px(0.), px(-1.5), gpui_kit::black().opacity(0.35)).blur_radius(px(2.)).inset(),
            ]),
            (false, _) => chip.raised_small(&c).hover(|s| s.text_color(c.ink)),
        };
        if let Some(f) = self.on_click {
            chip = chip.on_click(move |e, w, cx| f(e, w, cx));
        }
        chip
    }
}
