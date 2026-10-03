//! The model picker: a trigger that shows the chosen model, and a list of the
//! provider's models with a filter and a refresh row.
//!
//! The list opens in its own borderless window, so it can reach past the edge
//! of the Hub or the onboarding window. It opens under its trigger, or above
//! it when the screen has no room below, and it takes the height of its
//! content. A pick, Escape, or a click anywhere else closes it.
//!
//! The models themselves live in [`AppModel::model_lists`], so every view that
//! shows the same provider shares one fetch, and an open list updates when a
//! fetch ends.

use crate::model::{AppModel, ModelList};
use crate::popover::ns_window;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use crate::os::window as mac;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::{ActivePaper, text};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// A list longer than this gets a filter field.
const FILTER_FROM: usize = 8;
const SHEET_W: f32 = 340.;
/// Room around the sheet inside the window for its shadow, which is kept
/// small: the margin is part of the window and takes clicks.
const SIDE: f32 = 12.;
const TOP: f32 = 6.;
const BOTTOM: f32 = 16.;
/// Space between the trigger and the sheet.
const GAP: f64 = 4.;
/// The tallest the sheet gets: filter, 300 px of rows, footer.
const MAX_SHEET_H: f32 = 400.;
/// A click on the trigger first closes the open list (the list loses focus).
/// That click means "close", so it must not open the list again.
const REOPEN_GUARD: Duration = Duration::from_millis(400);

/// A trigger's place on screen, in Cocoa coordinates (origin at the bottom
/// left of the main display, y up). GPUI's window bounds are relative to the
/// window's own display, so they cannot place a window on another display.
#[derive(Clone, Copy)]
struct Anchor {
    left: f64,
    top: f64,
    bottom: f64,
    /// The window the trigger is in. The list becomes its child window.
    window: *mut c_void,
}

thread_local! {
    static ANCHORS: RefCell<HashMap<String, Anchor>> = RefCell::default();
    static OPEN: RefCell<Option<(String, AnyWindowHandle)>> = const { RefCell::new(None) };
    static LAST_CLOSE: RefCell<Option<(String, Instant)>> = const { RefCell::new(None) };
}

/// What the list shows above the provider's models, for example
/// "Provider's model" in the style editor. Picking it gives `None`.
pub struct Extra {
    pub label: SharedString,
    pub selected: bool,
}

/// Gets the model id, or None for the [`Extra`] row.
pub type OnPick = Box<dyn Fn(Option<String>, &mut App)>;

pub struct Request {
    /// The trigger, for example "provider:openrouter". One list per key.
    pub key: String,
    /// The key in [`AppModel::model_lists`].
    pub list_key: String,
    pub selected: Option<String>,
    pub extra: Option<Extra>,
    pub on_pick: OnPick,
    pub on_refresh: Box<dyn Fn(&mut App)>,
}

/// Records where a trigger is, so its list can open under it. Add it as a
/// child of a `relative()` trigger.
pub fn anchor(key: impl Into<String>) -> impl IntoElement {
    let key = key.into();
    canvas(
        move |bounds, window, _| {
            // The content view fills the window frame (full-size content).
            let Some(ns) = ns_window(window) else { return };
            let Some(frame) = mac::frame(ns) else { return };
            let window_top = frame.y + frame.height;
            let anchor = Anchor {
                left: frame.x + bounds.left().as_f32() as f64,
                top: window_top - bounds.top().as_f32() as f64,
                bottom: window_top - bounds.bottom().as_f32() as f64,
                window: ns,
            };
            ANCHORS.with(|a| a.borrow_mut().insert(key, anchor));
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

/// The inline trigger: "Haiku 4.5 ⌄", or "Choose a model ⌄" in accent when
/// nothing is chosen. Attach `on_click` that calls [`open`].
pub fn trigger(key: &str, label: impl Into<SharedString>, chosen: bool, cx: &App) -> Stateful<Div> {
    let c = cx.paper().colors;
    let fg = if chosen { c.ink } else { c.accent };
    div()
        .id(SharedString::from(format!("trigger-{key}")))
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .h(px(22.))
        .px(px(7.))
        .ml(px(-7.))
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|s| s.bg(c.sheet_raised))
        .child(text::ui(label.into(), 12., FontWeight::SEMIBOLD, fg).truncate())
        .child(icon(Icon::ChevronDown, 10., fg))
        .child(anchor(key))
}

/// Open the list for `req.key`, or close it when it is open.
pub fn open(model: &Entity<AppModel>, req: Request, cx: &mut App) {
    if close_open(cx).as_deref() == Some(req.key.as_str()) {
        return;
    }
    let just_closed = LAST_CLOSE.with(|l| l.borrow().as_ref().is_some_and(|(k, at)| *k == req.key && at.elapsed() < REOPEN_GUARD));
    if just_closed {
        return;
    }
    let Some(anchor) = ANCHORS.with(|a| a.borrow().get(&req.key).copied()) else { return };

    // Below the trigger, or above it when the screen has more room there.
    let screens = mac::screens();
    let screen = screens
        .iter()
        .find(|s| anchor.left >= s.frame.x && anchor.left < s.frame.x + s.frame.width && anchor.bottom >= s.frame.y && anchor.bottom <= s.frame.y + s.frame.height)
        .or(screens.first())
        .map(|s| s.visible_frame);
    let Some(area) = screen else { return };
    let below = anchor.bottom - area.y;
    let above = area.y + area.height - anchor.top;
    let flipped = below < (MAX_SHEET_H + BOTTOM) as f64 + GAP && above > below;
    let width = (SHEET_W + SIDE * 2.) as f64;
    // The sheet's left edge meets the trigger's.
    let left = (anchor.left - SIDE as f64).max(area.x).min(area.x + area.width - width);
    let place = Place {
        parent: anchor.window,
        left,
        edge: if flipped { anchor.top + GAP - BOTTOM as f64 } else { anchor.bottom - GAP + TOP as f64 },
        flipped,
    };

    let key = req.key.clone();
    let model = model.clone();
    // The window measures its sheet in its first frame and then takes its
    // size. Windows draws only a shown window, so there the list opens shown,
    // at its place with a first guess of the height; macOS opens it hidden.
    const FIRST_H: f32 = 160.;
    let cocoa_top = if place.flipped { place.edge + FIRST_H as f64 } else { place.edge };
    let origin = point(px(place.left as f32), px((mac::primary_screen_height() - cocoa_top) as f32));
    // Plain GPUI, not `gpui_kit::open_window`: the kit's root paints the theme
    // background over the whole window, which shows around the sheet as a frame.
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size: size(px(SHEET_W + SIDE * 2.), px(FIRST_H)) })),
            titlebar: None,
            focus: true,
            show: !cfg!(target_os = "macos"),
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        move |window, cx| cx.new(|cx| PickerWindow::new(model, req, place, window, cx)),
    );
    match opened {
        Ok(handle) => OPEN.with(|o| *o.borrow_mut() = Some((key, handle.into()))),
        Err(e) => log::error!("could not open the model list: {e:#}"),
    }
}

/// Close the open list. Returns its key.
fn close_open(cx: &mut App) -> Option<String> {
    let (key, handle) = OPEN.with(|o| o.borrow_mut().take())?;
    LAST_CLOSE.with(|l| *l.borrow_mut() = Some((key.clone(), Instant::now())));
    // Deferred: the call may come from inside the list window's own update.
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    });
    Some(key)
}

/// Where the window goes, in Cocoa coordinates. `edge` is the window's top
/// (below the trigger) or its bottom (above it, `flipped`).
#[derive(Clone, Copy)]
struct Place {
    parent: *mut c_void,
    left: f64,
    edge: f64,
    flipped: bool,
}

struct PickerWindow {
    model: Entity<AppModel>,
    req: Request,
    place: Place,
    filter: Entity<InputState>,
    ns: Option<*mut c_void>,
    /// The window height last set, and the sheet height measured in prepaint.
    height: f32,
    measured: Rc<Cell<f32>>,
    was_active: bool,
    _subs: Vec<Subscription>,
}

impl PickerWindow {
    fn new(model: Entity<AppModel>, req: Request, place: Place, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter models"));
        filter.update(cx, |s, cx| s.focus(window, cx));
        let subs = vec![
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify()),
            // A click anywhere else takes focus away: close.
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.was_active = true;
                } else if this.was_active {
                    let _ = close_open(cx);
                }
            }),
        ];
        let ns = ns_window(window);
        Self { model, req, place, filter, ns, height: 0., measured: Rc::default(), was_active: false, _subs: subs }
    }

    fn pick(&mut self, pick: Option<String>, cx: &mut Context<Self>) {
        (self.req.on_pick)(pick, cx);
        let _ = close_open(cx);
    }

    /// Size the window to the sheet, keeping the edge at the trigger fixed.
    fn fit(&mut self, cx: &mut Context<Self>) {
        let sheet = self.measured.get();
        if sheet <= 0. || (sheet + TOP + BOTTOM - self.height).abs() < 0.5 {
            return;
        }
        let first = self.height == 0.;
        self.height = sheet + TOP + BOTTOM;
        let Some(ns) = self.ns else { return };
        let (w, h) = ((SHEET_W + SIDE * 2.) as f64, self.height as f64);
        // Cocoa y is the bottom edge.
        let y = if self.place.flipped { self.place.edge } else { self.place.edge - h };
        let x = self.place.left;
        let parent = self.place.parent;
        crate::app::appkit_later(cx, move || {
            mac::set_frame(ns, x, y, w, h);
            if first {
                // The sheet draws its own shadow in the margin.
                mac::make_clear(ns);
                // Move with the window that holds the trigger.
                mac::add_child_window(parent, ns);
                mac::show_and_focus(ns);
            }
        });
    }
}

impl Render for PickerWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The size measured in the last frame; the first frame only measures.
        self.fit(cx);
        let c = cx.paper().colors;
        let list = self.model.read(cx).model_lists.get(&self.req.list_key).cloned();
        let query = self.filter.read(cx).value().trim().to_lowercase();
        let selected = self.req.selected.clone();

        let mut rows = div().id("model-rows").flex().flex_col().max_h(px(300.)).overflow_y_scroll().p(px(4.));
        let row = |id: ElementId, name: String, note: Option<String>, is_selected: bool| {
            div()
                .id(id)
                .flex()
                .items_start()
                .gap(px(8.))
                .py(px(6.))
                .px(px(8.))
                .rounded(px(7.))
                .cursor_pointer()
                .hover(|s| s.bg(c.deboss))
                .child(div().flex_none().w(px(14.)).pt(px(2.)).when(is_selected, |d| d.child(icon(Icon::Check, 12., c.ink))))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(1.))
                        .child(text::ui(name, 13., if is_selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }, c.ink).truncate())
                        .when_some(note, |d, n| d.child(text::ui(n, 11., FontWeight::NORMAL, c.graphite).truncate())),
                )
        };
        if let Some(extra) = &self.req.extra {
            rows = rows.child(
                row("model-extra".into(), extra.label.to_string(), None, extra.selected)
                    .on_click(cx.listener(|this, _, _, cx| this.pick(None, cx))),
            );
        }
        let message = |s: String, color: Hsla| div().px(px(8.)).py(px(8.)).child(text::ui(s, 13., FontWeight::NORMAL, color));
        let mut count = None;
        let mut show_filter = false;
        match &list {
            None | Some(ModelList::Loading) => rows = rows.child(message("Loading models…".into(), c.graphite)),
            Some(ModelList::Failed(e)) => {
                rows = rows.child(message(format!("Sayso could not load the models: {e}. Check the provider, then refresh."), c.danger));
            }
            Some(ModelList::Ready(models)) => {
                count = Some(models.len());
                show_filter = models.len() > FILTER_FROM;
                let shown: Vec<_> = models
                    .iter()
                    .filter(|m| query.is_empty() || m.name.to_lowercase().contains(&query) || m.id.to_lowercase().contains(&query))
                    .collect();
                if models.is_empty() {
                    rows = rows.child(message("This provider lists no models.".into(), c.graphite));
                } else if shown.is_empty() {
                    rows = rows.child(message("No model matches the filter.".into(), c.graphite));
                }
                for (i, m) in shown.into_iter().enumerate() {
                    // The CLIs give a description; HTTP lists give only ids, so show the id when it differs.
                    let note = m.description.clone().or_else(|| (m.name != m.id).then(|| m.id.clone()));
                    let id = m.id.clone();
                    rows = rows.child(
                        row(ElementId::from(("model", i)), m.name.clone(), note, selected.as_deref() == Some(m.id.as_str()))
                            .on_click(cx.listener(move |this, _, _, cx| this.pick(Some(id.clone()), cx))),
                    );
                }
            }
        }

        let footer = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .py(px(6.))
            .pl(px(12.))
            .pr(px(6.))
            .border_t_1()
            .border_color(c.rule)
            .child(text::ui(
                match count {
                    Some(1) => "1 model".to_string(),
                    Some(n) => format!("{n} models"),
                    None => String::new(),
                },
                12.,
                FontWeight::NORMAL,
                c.graphite,
            ))
            .child(
                Button::new("refresh-models", "Refresh")
                    .small()
                    .ghost()
                    .disabled(matches!(list, Some(ModelList::Loading)))
                    .on_click(cx.listener(|this, _, _, cx| (this.req.on_refresh)(cx))),
            );

        let measured = self.measured.clone();
        let sheet = div()
            .id("model-picker")
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SHEET_W))
            .rounded(px(12.))
            .bg(c.sheet_raised)
            // Tighter than `paper::floating`, so the margin that holds it stays small.
            .shadow(vec![
                BoxShadow::new(px(0.), px(1.), c.highlight(1.0)).inset(),
                BoxShadow::new(px(0.), px(-1.), c.shadow(0.10)).inset(),
                BoxShadow::new(px(0.), px(1.), c.shadow(0.14)).blur_radius(px(2.)),
                BoxShadow::new(px(0.), px(4.), c.shadow(0.14)).blur_radius(px(12.)),
            ])
            .when(show_filter, |d| {
                d.child(
                    div().p(px(8.)).pb(px(4.)).child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(32.))
                            .px(px(10.))
                            .rounded(px(8.))
                            .debossed(&c)
                            .child(icon(Icon::Search, 12., c.graphite))
                            .child(crate::hub::pages::kit::bare_input(&self.filter, 13.).flex_1()),
                    ),
                )
            })
            .child(rows)
            .child(footer)
            // Measure the sheet, so the window can take its height.
            .child(
                canvas(
                    move |bounds, window, _| {
                        let h = bounds.size.height.as_f32();
                        if (measured.replace(h) - h).abs() >= 0.5 {
                            // Render again, so `fit` sees the new height.
                            window.request_animation_frame();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(self.place.flipped, |d| d.justify_end())
            .pt(px(TOP))
            .px(px(SIDE))
            .pb(px(BOTTOM))
            .font_family(sayso_ui::fonts::UI)
            .text_color(c.ink)
            .capture_key_down(cx.listener(|_, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    let _ = close_open(cx);
                }
            }))
            .child(sheet)
    }
}
