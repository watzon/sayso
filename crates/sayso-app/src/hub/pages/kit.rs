//! Small pieces shared by the Hub pages: formatting, headings, wells,
//! empty states, and a dropdown menu. Values follow the Paper design file.

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::ink::Rgb;
use sayso_ui::assets::Icon;
use sayso_ui::components::icon;
use sayso_ui::fonts::{DISPLAY, MONO, UI};
use sayso_ui::paper::{self, PaperStyled};
use sayso_ui::{ActivePaper, Colors, text};

/// A history entry that another page asked History to select.
#[derive(Default)]
pub struct PendingHistorySelect(pub Option<i64>);

impl Global for PendingHistorySelect {}

/// Ask History to select an entry the next time it renders.
pub fn select_history_entry(id: i64, cx: &mut App) {
    cx.set_global(PendingHistorySelect(Some(id)));
}

/// Take the entry that History should select, if any.
pub fn take_history_selection(cx: &mut App) -> Option<i64> {
    if !cx.has_global::<PendingHistorySelect>() {
        return None;
    }
    cx.global_mut::<PendingHistorySelect>().0.take()
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

/// "9:41", "18:04".
pub fn clock(at: chrono::DateTime<chrono::Utc>) -> String {
    at.with_timezone(&chrono::Local).format("%-H:%M").to_string()
}

/// The time of an entry from today ("9:41"), or the day of an older one ("Sep 30").
pub fn clock_or_day(at: chrono::DateTime<chrono::Utc>) -> String {
    let local = at.with_timezone(&chrono::Local);
    if local.date_naive() == chrono::Local::now().date_naive() { clock(at) } else { local.format("%b %-d").to_string() }
}

/// "0:07", "1:32".
pub fn duration(ms: u64) -> String {
    let s = (ms + 500) / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// "7 s", "1 min 12 s".
pub fn duration_words(ms: u64) -> String {
    let s = ((ms + 500) / 1000).max(1);
    if s < 60 { format!("{s} s") } else { format!("{} min {} s", s / 60, s % 60) }
}

/// "Today", "Yesterday", "Monday, September 28".
pub fn day_label(day: chrono::NaiveDate) -> String {
    let today = chrono::Local::now().date_naive();
    if day == today {
        "Today".into()
    } else if Some(day) == today.pred_opt() {
        "Yesterday".into()
    } else if (today - day).num_days() < 300 {
        day.format("%A, %B %-d").to_string()
    } else {
        day.format("%B %-d, %Y").to_string()
    }
}

/// "1 replacement", "3 replacements".
pub fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

/// A style or app color from `#RRGGBB`, made readable on dark paper.
pub fn ink_color(hex: &str, c: &Colors) -> Hsla {
    let Some(rgb) = Rgb::parse(hex) else { return c.ink };
    if c.is_dark() {
        // Dark inks vanish on dark paper: lift them toward the paper ink.
        let sheet = Rgb(0x201F1C);
        if rgb.contrast(sheet) < 2.6 {
            return sayso_ui::theme::hsla(rgb.mix(Rgb(0xECE6DA), 0.55));
        }
    }
    sayso_ui::theme::hsla(rgb)
}

/// The first word of the user's full name, for the greeting.
pub fn first_name() -> Option<String> {
    static NAME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    NAME.get_or_init(|| crate::shell::user_full_name()?.split_whitespace().next().map(str::to_string)).clone()
}

/// A short slug for ids: "My Style" becomes "my-style".
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { "custom".into() } else { out }
}

// ---------------------------------------------------------------------------
// Type
// ---------------------------------------------------------------------------

/// Small caps at a given size (the design uses 12 and 13 px).
pub fn caps(s: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    let s: SharedString = s.into();
    div()
        .font_family(UI)
        .text_size(px(size))
        .line_height(px(16.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .child(SharedString::from(s.to_uppercase()))
}

/// Fraunces at a reading size, regular weight.
pub fn fraunces(s: impl Into<SharedString>, size: f32, line: f32, color: Hsla) -> Div {
    div().font_family(DISPLAY).text_size(px(size)).line_height(px(line)).text_color(color).child(s.into())
}

pub fn mono(s: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div().font_family(MONO).text_size(px(size)).line_height(px(16.)).text_color(color).child(s.into())
}

pub fn ui(s: impl Into<SharedString>, size: f32, line: f32, weight: FontWeight, color: Hsla) -> Div {
    text::ui(s, size, weight, color).line_height(px(line))
}

/// A page title with an intro line, and actions at the bottom right.
pub fn page_head(title: &str, intro: Option<AnyElement>, actions: Option<AnyElement>, cx: &App) -> Div {
    let c = cx.paper().colors;
    div()
        .flex()
        .flex_none()
        .items_end()
        .justify_between()
        .gap(px(24.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(text::display(title.to_string(), 44., &c))
                .when_some(intro, |d, i| d.child(i)),
        )
        .when_some(actions, |d, a| d.child(div().flex().flex_none().items_center().gap(px(8.)).child(a)))
}

/// The 15 px graphite intro under a page title.
pub fn intro(s: impl Into<SharedString>, width: f32, cx: &App) -> AnyElement {
    let c = cx.paper().colors;
    ui(s, 15., 22., FontWeight::NORMAL, c.graphite).max_w(px(width)).into_any_element()
}

/// A section heading: 22 px Fraunces, a note on the right, a rule below.
pub fn section_head(title: &str, note: Option<AnyElement>, cx: &App) -> Div {
    let c = cx.paper().colors;
    div()
        .flex()
        .flex_none()
        .items_end()
        .justify_between()
        .gap(px(16.))
        .pb(px(10.))
        .border_b_1()
        .border_color(c.rule)
        .child(text::title(title.to_string(), 22., &c).line_height(px(28.)))
        .when_some(note, |d, n| d.child(div().pb(px(4.)).child(n)))
}

// ---------------------------------------------------------------------------
// Surfaces
// ---------------------------------------------------------------------------

/// An input with no chrome, at a given text size, for use inside a well.
pub fn bare_input(state: &Entity<InputState>, size: f32) -> Input {
    let line = (size * 1.6).round();
    Input::new(state).appearance(false).px_0().py_0().h(px(line + 4.)).text_size(px(size)).line_height(px(line))
}

/// A search or filter well with an icon and an input.
pub fn search_well(state: &Entity<InputState>, height: f32, trailing: Option<AnyElement>, cx: &App) -> Div {
    let c = cx.paper().colors;
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .h(px(height))
        .px(px(12.))
        .rounded(px(10.))
        .debossed(&c)
        .text_size(px(14.))
        .child(icon(Icon::Search, 15., c.graphite))
        .child(div().flex_1().min_w_0().child(bare_input(state, 14.)))
        .when_some(trailing, |d, t| d.child(t))
}

/// A form field: label above a debossed input.
pub fn field(label: &str, input: impl IntoElement, hint: Option<&str>, cx: &App) -> Div {
    let c = cx.paper().colors;
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(ui(label.to_string(), 13., 16., FontWeight::SEMIBOLD, c.ink))
        .child(input)
        .when_some(hint, |d, h| d.child(ui(h.to_string(), 12., 16., FontWeight::NORMAL, c.graphite)))
}

/// A single-line input in a debossed well, 36 px high.
pub fn input_well(state: &Entity<InputState>, cx: &App) -> Div {
    let c = cx.paper().colors;
    div()
        .flex()
        .items_center()
        .h(px(36.))
        .px(px(12.))
        .rounded(px(9.))
        .debossed(&c)
        .text_size(px(14.))
        .child(bare_input(state, 14.).w_full())
}

/// A tiny keycap with one label, for hints like ⌘F.
pub fn mini_key(label: impl Into<SharedString>, cx: &App) -> Div {
    let c = cx.paper().colors;
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(20.))
        .px(px(6.))
        .rounded(px(5.))
        .bg(c.sheet)
        .shadow(vec![
            BoxShadow::new(px(0.), px(-1.), c.shadow(0.22)).inset(),
            BoxShadow::new(px(0.), px(1.), c.shadow(0.14)).blur_radius(px(1.)),
        ])
        .child(mono(label, 11., c.graphite).line_height(px(14.)))
}

/// A dashed slip: "Add word", "Add" provider.
pub fn dashed(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let c = cx.paper().colors;
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .rounded(px(6.))
        .border_1()
        .border_dashed()
        .border_color(c.deboss_shade)
        .cursor_pointer()
        .hover(|s| s.bg(c.deboss.opacity(0.4)))
}

/// A centered message for a page or pane with nothing to show.
pub fn empty_state(ic: Icon, title: &str, body: &str, action: Option<AnyElement>, cx: &App) -> Div {
    let c = cx.paper().colors;
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(10.))
        .py(px(36.))
        .px(px(24.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(44.))
                .rounded_full()
                .debossed(&c)
                .child(icon(ic, 20., c.graphite)),
        )
        .child(text::title(title.to_string(), 22., &c).pt(px(4.)))
        .child(ui(body.to_string(), 14., 20., FontWeight::NORMAL, c.graphite).max_w(px(380.)).text_center())
        .when_some(action, |d, a| d.child(div().pt(px(6.)).child(a)))
}

/// A text link in the accent color.
pub fn link(id: impl Into<ElementId>, label: impl Into<SharedString>, size: f32, cx: &App) -> Stateful<Div> {
    let c = cx.paper().colors;
    div()
        .id(id)
        .flex_none()
        .cursor_pointer()
        .font_family(UI)
        .text_size(px(size))
        .line_height(px(18.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(c.accent)
        .hover(|s| s.opacity(0.75))
        .child(label.into())
}

/// A small round icon button for row actions (remove, edit).
pub fn icon_button(id: impl Into<ElementId>, ic: Icon, size: f32, cx: &App) -> Stateful<Div> {
    let c = cx.paper().colors;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size + 10.))
        .rounded_full()
        .cursor_pointer()
        .hover(|s| s.bg(c.deboss))
        .child(icon(ic, size, c.graphite))
}

/// "Cloud" (accent) or "Local" (success) badge in the design's mixed case.
pub fn place_badge(cloud: bool, cx: &App) -> Div {
    let c = cx.paper().colors;
    let (bg, fg, label) = if cloud { (c.accent_wash, c.accent, "Cloud") } else { (c.success.opacity(0.14), c.success, "Local") };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .h(px(22.))
        .px(px(8.))
        .rounded_full()
        .bg(bg)
        .when(cloud, |d| d.child(icon(Icon::Cloud, 11., fg)))
        .child(ui(label, 12., 16., FontWeight::SEMIBOLD, fg))
}

/// A floating menu. Place it inside a `relative()` parent.
pub fn menu<F>(id: &'static str, items: Vec<(SharedString, bool)>, on_pick: F, on_close: impl Fn(&mut Window, &mut App) + 'static, cx: &App) -> impl IntoElement
where
    F: Fn(usize, &mut Window, &mut App) + 'static,
{
    let c = cx.paper().colors;
    let on_pick = std::rc::Rc::new(on_pick);
    let mut list = div()
        .id(id)
        .flex()
        .flex_col()
        .min_w(px(200.))
        .max_h(px(320.))
        .overflow_y_scroll()
        .p(px(4.))
        .rounded(px(10.))
        .bg(c.sheet_raised)
        .shadow(paper::floating(&c))
        .occlude()
        .on_mouse_down_out(move |_, w, cx| on_close(w, cx));
    for (i, (label, selected)) in items.into_iter().enumerate() {
        let f = on_pick.clone();
        list = list.child(
            div()
                .id(i)
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(30.))
                .px(px(10.))
                .rounded(px(7.))
                .cursor_pointer()
                .hover(|s| s.bg(c.deboss))
                .on_click(move |_, w, cx| f(i, w, cx))
                .child(div().w(px(14.)).when(selected, |d| d.child(icon(Icon::Check, 12., c.ink))))
                .child(ui(label, 13., 16., if selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }, c.ink)),
        );
    }
    deferred(anchored().snap_to_window_with_margin(px(8.)).child(div().mt(px(4.)).child(list))).with_priority(2)
}

/// One line of an [`action_menu`].
pub struct MenuAction {
    pub icon: Icon,
    pub label: SharedString,
    /// A destructive action: danger color, below a rule.
    pub danger: bool,
}

impl MenuAction {
    pub fn new(icon: Icon, label: impl Into<SharedString>) -> Self {
        Self { icon, label: label.into(), danger: false }
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

/// A floating menu of actions, each with an icon. Place it inside a
/// `relative()` parent. For a list with one selected item, use [`menu`].
pub fn action_menu<F>(id: &'static str, items: Vec<MenuAction>, on_pick: F, on_close: impl Fn(&mut Window, &mut App) + 'static, cx: &App) -> impl IntoElement
where
    F: Fn(usize, &mut Window, &mut App) + 'static,
{
    let c = cx.paper().colors;
    let on_pick = std::rc::Rc::new(on_pick);
    let mut list = div()
        .id(id)
        .flex()
        .flex_col()
        .gap(px(1.))
        .min_w(px(184.))
        .p(px(5.))
        .rounded(px(12.))
        .bg(c.sheet_raised)
        .border_1()
        .border_color(c.rule)
        .shadow(paper::floating(&c))
        .occlude()
        .on_mouse_down_out(move |_, w, cx| on_close(w, cx));
    for (i, item) in items.into_iter().enumerate() {
        let f = on_pick.clone();
        let (fg, icon_fg, hover) = if item.danger { (c.danger, c.danger, c.danger_wash) } else { (c.ink, c.graphite, c.deboss) };
        if item.danger && i > 0 {
            list = list.child(div().flex_none().h(px(1.)).mx(px(6.)).my(px(4.)).bg(c.rule));
        }
        list = list.child(
            div()
                .id(i)
                .flex()
                .flex_none()
                .items_center()
                .gap(px(10.))
                .h(px(32.))
                .pl(px(10.))
                .pr(px(14.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_click(move |_, w, cx| f(i, w, cx))
                .child(icon(item.icon, 14., icon_fg))
                .child(ui(item.label, 13., 16., FontWeight::MEDIUM, fg)),
        );
    }
    deferred(anchored().snap_to_window_with_margin(px(8.)).child(div().mt(px(6.)).child(list))).with_priority(2)
}

/// The circle with a check from the "All set" list, or a warning ring.
pub fn check_dot(ok: bool, cx: &App) -> Div {
    let c = cx.paper().colors;
    if ok {
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(16.))
            .rounded_full()
            .bg(c.deboss)
            .child(icon(Icon::Check, 10., c.ink))
    } else {
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(16.))
            .rounded_full()
            .bg(c.danger_wash)
            .child(icon(Icon::Warning, 10., c.danger))
    }
}

/// A seal dot in a style's ink. Raw has no ink: a hollow ring.
pub fn seal_dot(hex: Option<&str>, size: f32, cx: &App) -> Div {
    let c = cx.paper().colors;
    match hex {
        Some(h) => div().flex_none().size(px(size)).rounded_full().bg(ink_color(h, &c)).shadow(vec![
            BoxShadow::new(px(0.), px(-1.5), black().opacity(0.35)).blur_radius(px(2.)).inset(),
            BoxShadow::new(px(0.), px(1.), white().opacity(0.25)).inset(),
        ]),
        None => div().flex_none().size(px(size)).rounded_full().border_1().border_color(c.pencil),
    }
}
