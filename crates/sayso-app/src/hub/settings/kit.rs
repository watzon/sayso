//! Building blocks for the settings pages: the page frame, groups, the key
//! recorder field, a volume slider, and permission badges.

use super::recorder::{Recorder, Slot};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::hotkey::Hotkey;
use sayso_i18n::t;
use sayso_platform::PermissionState;
use sayso_ui::ActivePaper;
use sayso_ui::assets::Icon;
use sayso_ui::a11y::{A11yExt as _, Pressable, Role};
use sayso_ui::components::*;
use sayso_ui::text;
use std::cell::Cell;
use std::rc::Rc;

type OnRelease = Rc<dyn Fn(&mut Window, &mut App)>;
type OnFraction = Rc<dyn Fn(f32, &mut Window, &mut App)>;

/// A settings page: title, subtitle, then the body, in a scrolling column.
pub fn page(id: &'static str, title: &str, subtitle: &str, body: impl IntoElement, cx: &App) -> Div {
    // The page sits in an absolute layer, so long text never widens the sheet.
    div().relative().size_full().child(
        div().absolute().top_0().left_0().right_0().bottom_0().child(
            div()
                .id(id)
                .size_full()
                .flex()
                .flex_col()
                .pt(px(44.))
                .px(px(52.))
                .pb(px(32.))
                .child(crate::widgets::page_header(title, Some(subtitle), None, cx).pb(px(28.)).flex_none())
                .child(body)
                .overflow_y_scrollbar(),
        ),
    )
}

/// The page body: a column of groups.
pub fn body() -> Div {
    div().flex().flex_col().flex_none().w_full().min_w_0().gap(px(28.))
}

/// A group of rows under a small caps label.
#[track_caller]
pub fn group(title: &str, cx: &App) -> Div {
    div().flex().flex_col().w_full().min_w_0().pt(px(8.)).child(crate::widgets::section(title, cx))
}

/// A setting row with a control on the right (the design's row: 15/18 title, 13/16 description).
pub fn row(title: &str, description: &str, control: impl IntoElement, cx: &App) -> Div {
    row_s(title.to_string(), description.to_string(), control, cx)
}

/// A row with owned strings.
pub fn row_s(title: String, description: String, control: impl IntoElement, cx: &App) -> Div {
    let c = cx.paper().colors;
    div()
        .flex()
        .items_center()
        .w_full()
        .gap(px(24.))
        .py(px(14.))
        .border_b_1()
        .border_color(c.rule)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .flex_1()
                .min_w_0()
                .child(text::ui(title, 15., FontWeight::SEMIBOLD, c.ink).line_height(px(18.)))
                .when(!description.is_empty(), |d| {
                    d.child(text::ui(description, 13., FontWeight::NORMAL, c.graphite).line_height(px(16.)))
                }),
        )
        .child(control)
}

/// The focus target of the key recorder: an empty element with a name. While
/// the recorder listens, the focus is here, so a screen reader reads "Press
/// keys to record". Its parent handles the keys.
pub fn key_listener(id: &'static str, focus: &FocusHandle) -> Stateful<Div> {
    div().id(id).role(Role::Group).aria_label(t!("settings.common.press_keys")).absolute().size_0().track_focus(focus)
}

/// A banner with vertical room, for use between rows.
#[track_caller]
pub fn banner(kind: BannerKind, text: impl Into<SharedString>, cx: &App) -> Div {
    div().w_full().min_w_0().py(px(8.)).child(notice(kind, text, cx))
}

/// The − value + stepper, with room for values such as "10 min". Give it
/// the row title with `.label(...)`.
pub fn stepper(id: impl Into<ElementId>, value: impl Into<SharedString>, on_change: impl Fn(i32, &mut Window, &mut App) + 'static, _cx: &App) -> Stepper {
    Stepper::new(id, value).on_change(on_change)
}

/// The key well: keycaps plus "Change", as in the Dictation design.
/// `label` is the title of the row: "Toggle hotkey".
pub fn key_well(id: impl Into<ElementId>, label: &str, hotkey: &Hotkey, on_change: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static, cx: &App) -> Pressable {
    let c = cx.paper().colors;
    let keys = sayso_ui::a11y::spoken_keys(&hotkey.keycaps());
    Pressable::new(id)
        .label(t!("settings.common.change_hotkey", name = label, keys = keys))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .h(px(38.))
        .px(px(6.))
        .rounded(px(10.))
        .bg(c.deboss)
        .shadows(sayso_ui::paper::debossed(&c))
        .cursor_pointer()
        .on_click(on_change)
        .child(crate::widgets::hotkey_keycaps(hotkey))
        .child(div().pl(px(10.)).pr(px(8.)).child(text::ui(t!("settings.common.change"), 13., FontWeight::MEDIUM, c.accent)))
}

/// A dashed box: the empty or recording state of a key field, "Add app".
///
/// It is a button; its text is its name unless it gets a `.label(...)`.
pub fn dashed(id: impl Into<ElementId>, cx: &App) -> Pressable {
    let c = cx.paper().colors;
    Pressable::new(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .h(px(38.))
        .px(px(14.))
        .rounded(px(10.))
        .border(px(1.5))
        .border_dashed()
        .border_color(c.deboss_shade)
        .cursor_pointer()
        .hover(|s| s.bg(c.deboss.opacity(0.4)))
}

/// The "Press keys to record…" box.
pub fn recording_box(id: impl Into<ElementId>, window_only: bool, on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static, cx: &App) -> Pressable {
    let c = cx.paper().colors;
    let label = if window_only { t!("settings.common.press_keys_here") } else { t!("settings.common.press_keys") };
    dashed(id, cx)
        .label(label.clone())
        .a11y_live(label.clone(), false)
        .on_click(on_cancel)
        .child(div().size(px(7.)).rounded_full().bg(c.accent))
        .child(text::ui(label, 13., FontWeight::MEDIUM, c.graphite))
}

/// A complete key field for a settings row.
#[allow(clippy::too_many_arguments)]
pub fn key_field<V: 'static>(
    slot: Slot,
    label: &str,
    hotkey: Option<Hotkey>,
    recorder: &Recorder,
    clearable: bool,
    extra: Option<AnyElement>,
    start: impl Fn(&mut V, Slot, &mut Window, &mut Context<V>) + 'static + Clone,
    cancel: impl Fn(&mut V, &mut Context<V>) + 'static,
    clear: impl Fn(&mut V, Slot, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> Div {
    let c = cx.paper().colors;
    let id = format!("{slot:?}");
    let mut out = div().flex().flex_none().items_center().gap(px(8.));
    if recorder.is_recording(slot) {
        out = out.child(recording_box(
            SharedString::from(format!("{id}-rec")),
            recorder.window_only,
            cx.listener(move |this, _, _, cx| cancel(this, cx)),
            cx,
        ));
        return out;
    }
    if let Some(extra) = extra {
        out = out.child(extra);
    }
    match hotkey {
        Some(hk) => {
            let s = start.clone();
            out = out.child(key_well(
                SharedString::from(format!("{id}-well")),
                label,
                &hk,
                cx.listener(move |this, _, window, cx| s(this, slot, window, cx)),
                cx,
            ));
            if clearable {
                out = out.child(
                    Button::new(SharedString::from(format!("{id}-clear")), t!("settings.common.clear"))
                        .a11y_label(t!("settings.common.clear_hotkey", name = label))
                        .ghost()
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| clear(this, slot, cx))),
                );
            }
        }
        None => {
            let s = start.clone();
            out = out.child(
                dashed(SharedString::from(format!("{id}-empty")), cx)
                    .label(t!("settings.common.record_hotkey", name = label))
                    .on_click(cx.listener(move |this, _, window, cx| s(this, slot, window, cx)))
                    .child(icon(Icon::Keyboard, 14., c.graphite))
                    .child(text::ui(t!("settings.common.record_key"), 13., FontWeight::MEDIUM, c.graphite)),
            );
        }
    }
    out
}

/// A horizontal slider with a raised knob. `on_change` gets 0.0 to 1.0;
/// `on_release` runs when the mouse goes up over the slider, and after each
/// step from the keyboard or an assistive client (5 %). `label` is the row
/// title.
pub fn slider(
    id: impl Into<ElementId>,
    label: &str,
    value: f32,
    width: f32,
    on_change: impl Fn(f32, &mut Window, &mut App) + 'static,
    on_release: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Pressable {
    let c = cx.paper().colors;
    let value = value.clamp(0.0, 1.0);
    let on_release: OnRelease = Rc::new(on_release);
    let bounds: Rc<Cell<Bounds<Pixels>>> = Rc::new(Cell::new(Bounds::default()));
    let on_change = Rc::new(on_change);
    let fraction = {
        let bounds = bounds.clone();
        move |x: Pixels| {
            let b = bounds.get();
            // The knob center travels from 10 px to width - 10 px.
            let w = (b.size.width.as_f32() - 20.).max(1.0);
            (((x - b.origin.x).as_f32() - 10.) / w).clamp(0.0, 1.0)
        }
    };
    let knob_x = (width - 20.) * value;
    let b2 = bounds.clone();
    let (f1, f2) = (fraction.clone(), fraction);
    let (c1, c2) = (on_change.clone(), on_change.clone());
    let step: OnFraction = {
        let on_release = on_release.clone();
        Rc::new(move |delta: f32, w: &mut Window, cx: &mut App| {
            on_change((value + delta).clamp(0.0, 1.0), w, cx);
            on_release(w, cx);
        })
    };
    let (s1, s2, s3) = (step.clone(), step.clone(), step);
    let release = on_release.clone();
    Pressable::new(id)
        .a11y_role(Role::Slider)
        .label(label.to_string())
        .aria_numeric_value((value * 100.).round() as f64)
        .aria_min_numeric_value(0.)
        .aria_max_numeric_value(100.)
        .aria_numeric_value_step(5.)
        .on_a11y_action(AccessibleAction::Increment, move |_, w, cx| s1(0.05, w, cx))
        .on_a11y_action(AccessibleAction::Decrement, move |_, w, cx| s2(-0.05, w, cx))
        .on_key_down(move |e, w, cx| {
            let delta = match e.keystroke.key.as_str() {
                "right" | "up" => 0.05,
                "left" | "down" => -0.05,
                _ => return,
            };
            cx.stop_propagation();
            s3(delta, w, cx);
        })
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .w(px(width))
        .h(px(20.))
        .cursor_pointer()
        .child(
            canvas(move |b, _, _| b2.set(b), |_, _, _, _| {})
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
        )
        .child(
            div()
                .flex()
                .w_full()
                .h(px(6.))
                .rounded_full()
                .bg(c.deboss)
                .shadow(vec![BoxShadow::new(px(0.), px(1.), c.shadow(0.28)).blur_radius(px(2.)).inset()])
                .child(div().h_full().w(px(knob_x + 10.)).rounded_full().bg(c.ink_fill)),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left(px(knob_x))
                .size(px(20.))
                .rounded_full()
                .bg(c.sheet_raised)
                .shadow(vec![
                    BoxShadow::new(px(0.), px(1.), c.highlight(1.0)).inset(),
                    BoxShadow::new(px(0.), px(1.), c.shadow(0.35)).blur_radius(px(3.)),
                ]),
        )
        .on_mouse_down(MouseButton::Left, move |e, w, cx| {
            c1(f1(e.position.x), w, cx);
        })
        .on_mouse_move(move |e, w, cx| {
            if e.pressed_button == Some(MouseButton::Left) {
                c2(f2(e.position.x), w, cx);
            }
        })
        // The view keeps the pending value, so release saves only after a press here.
        .on_mouse_up(MouseButton::Left, move |_, w, cx| release(w, cx))
}

/// A permission state as a small label with a dot or check.
pub fn permission_badge(state: PermissionState, cx: &App) -> Div {
    let c = cx.paper().colors;
    let (label, color, check) = match state {
        PermissionState::Granted => (t!("settings.common.allowed"), c.ink, true),
        PermissionState::Denied => (t!("settings.common.blocked"), c.danger, false),
        PermissionState::NotDetermined => (t!("settings.common.waiting"), c.accent, false),
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .when(check, |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(16.))
                    .rounded_full()
                    .bg(c.ink_fill)
                    .child(icon(Icon::Check, 10., c.on_ink)),
            )
        })
        .when(!check, |d| d.child(halo_dot(8., color, 3., 0.16)))
        .child(text::ui(label, 13., FontWeight::SEMIBOLD, color))
}

/// A path line with a Reveal button.
pub fn path_row(id: &'static str, title: &str, path: &std::path::Path, cx: &App) -> Div {
    let c = cx.paper().colors;
    let shown = sayso_core::paths::Paths::display(path, &sayso_core::paths::PathEnv::home(&sayso_core::paths::SystemEnv));
    let p = path.to_path_buf();
    div()
        .flex()
        .items_center()
        .gap(px(24.))
        .py(px(14.))
        .border_b_1()
        .border_color(c.rule)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .flex_1()
                .min_w_0()
                .child(text::ui(title.to_string(), 15., FontWeight::SEMIBOLD, c.ink))
                .child(text::mono(shown, 12., c.graphite).font_weight(FontWeight::NORMAL)),
        )
        .child(Button::new(id, t!("settings.common.reveal")).a11y_label(t!("settings.common.reveal_named", name = title)).small().on_click(move |_, _, _| {
            let _ = std::fs::create_dir_all(&p);
            crate::model::AppModel::open_path(&p);
        }))
}

/// A notice row like `Banner`, but its text wraps inside the row
/// (the text shrinks below its one-line width).
///
/// Like `Banner`, it is a live node: a screen reader reads it when it shows.
#[track_caller]
pub fn notice(kind: BannerKind, message: impl Into<SharedString>, cx: &App) -> Stateful<Div> {
    let message: SharedString = message.into();
    let warning = kind == BannerKind::Warning;
    let c = cx.paper().colors;
    let (bg, fg, border, ic) = match kind {
        BannerKind::Warning => (c.danger_wash, c.danger, c.danger.opacity(0.18), Icon::Warning),
        BannerKind::Info => (gpui_kit::transparent_black(), c.graphite, c.rule, Icon::Info),
    };
    let text_color = if kind == BannerKind::Warning && !c.is_dark() { hsla(0.02, 0.6, 0.23, 1.0) } else { fg };
    div()
        .id(sayso_ui::a11y::call_site_id(""))
        .role(if warning { Role::Alert } else { Role::Status })
        .aria_label(message.clone())
        .a11y_live(message.clone(), warning)
        .flex()
        .items_start()
        .w_full()
        .min_w_0()
        .gap(px(12.))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(10.))
        .bg(bg)
        .border_1()
        .border_color(border)
        .child(icon(ic, 16., fg).mt(px(1.)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(sayso_ui::fonts::UI)
                .text_size(px(13.))
                .line_height(px(19.))
                .text_color(text_color)
                .child(message),
        )
}

/// A round dot with a soft round halo. The halo does not take layout space.
pub fn halo_dot(size: f32, color: Hsla, halo: f32, alpha: f32) -> Div {
    div()
        .relative()
        .flex_none()
        .size(px(size))
        .child(
            div()
                .absolute()
                .top(px(-halo))
                .left(px(-halo))
                .size(px(size + halo * 2.))
                .rounded_full()
                .bg(color.opacity(alpha)),
        )
        .child(div().absolute().top_0().left_0().size(px(size)).rounded_full().bg(color))
}
