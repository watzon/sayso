//! The menu bar icon and its themed popover (Paper page "Menu bar").
//!
//! The status item has no NSMenu. A click shows our own panel under the icon
//! (or above it, for a taskbar icon at the bottom of the screen); a click
//! outside it, a second icon click, or Esc closes it.

use crate::hub::Route;
use crate::model::AppModel;
use gpui_kit::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use sayso_core::dictation::State;
use crate::os::window as mac;
use sayso_ui::ActivePaper;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::fonts::UI;
use sayso_ui::paper::PaperStyled;
use sayso_ui::text;
use sayso_ui::texture::{Grain, grain};
use std::cell::RefCell;
use std::ffi::c_void;
use std::time::Duration;

const WIDTH: f32 = 340.;
/// The Model and Microphone submenus open beside the sheet, in the same window.
const SUBMENU_W: f32 = 250.;
const SUBMENU_GAP: f32 = 4.;
const SUBMENU_MAX_H: f32 = 320.;
/// The window is wider and taller than the sheet, so a submenu has room at
/// either side and below the last row. The rest of the window is clear, and a
/// click there closes the popover.
const WINDOW_W: f32 = WIDTH + SUBMENU_GAP + SUBMENU_W + SUBMENU_SHADOW;
/// Room past a submenu for its shadow.
const SUBMENU_SHADOW: f32 = 16.;
const WINDOW_H: f32 = 760.;

/// The native handle of a GPUI window, as the pointer the window helpers
/// take: the NSView on macOS, the HWND on Windows.
pub fn ns_window(window: &Window) -> Option<*mut c_void> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr()),
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as *mut c_void),
        _ => None,
    }
}

struct Tray {
    icon: tray_icon::TrayIcon,
    popover: Option<AnyWindowHandle>,
    visible: bool,
    _monitor: Option<mac::MonitorToken>,
}

/// Where the popover window goes for a menu bar icon at `icon`, and the side
/// of the sheet that has room for a submenu. The sheet sits under the icon,
/// or above it when `above` (a taskbar at the bottom of the screen).
/// Submenus open to the right, or to the left when the screen ends too soon.
fn placement(icon: mac::Rect, screen_right: f64, above: bool) -> (f64, f64, SubmenuSide) {
    let sheet_x = (icon.x + icon.width / 2.0 - WIDTH as f64 / 2.0).min(screen_right - WIDTH as f64 - 8.0);
    let side_room = (SUBMENU_GAP + SUBMENU_W) as f64;
    // With submenus at the left, the sheet is at the right end of the window.
    let window_past_sheet = (WINDOW_W - WIDTH) as f64;
    // Cocoa coordinates: the window's top edge meets the icon's bottom edge,
    // or its bottom edge meets the icon's top edge.
    let y = if above { icon.y + icon.height + 4.0 } else { icon.y - WINDOW_H as f64 - 4.0 };
    if sheet_x + WIDTH as f64 + side_room + 8.0 <= screen_right {
        (sheet_x, y, SubmenuSide::Right)
    } else {
        (sheet_x - window_past_sheet, y, SubmenuSide::Left)
    }
}

/// Put the popover under the menu bar icon and show it. Runs outside a GPUI update.
fn place_and_show(ns: *mut c_void, origin: Option<(f64, f64)>) {
    if let Some((x, y)) = origin {
        mac::set_frame_origin(ns, x, y);
    }
    mac::show_and_focus(ns);
}

thread_local! {
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

fn tray_icon_image() -> Option<tray_icon::Icon> {
    let bytes = sayso_ui::assets::bytes("app/tray.png")?;
    #[allow(unused_mut)]
    let mut img = image::load_from_memory(&bytes).ok()?.into_rgba8();
    // The icon is a black template. macOS tints it; on a dark Windows
    // taskbar it must be white to show.
    #[cfg(windows)]
    if mac::taskbar_is_dark() {
        for p in img.pixels_mut() {
            p.0[..3].copy_from_slice(&[255, 255, 255]);
        }
    }
    let (w, h) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), w, h).ok()
}

pub fn install(model: &Entity<AppModel>, cx: &mut App) {
    let mut builder = tray_icon::TrayIconBuilder::new().with_tooltip("Sayso");
    if let Some(icon) = tray_icon_image() {
        #[cfg(target_os = "macos")]
        {
            builder = builder.with_icon_templated(icon);
        }
        #[cfg(not(target_os = "macos"))]
        {
            builder = builder.with_icon(icon);
        }
    } else {
        builder = builder.with_title("Sayso");
    }
    let icon = match builder.build() {
        Ok(i) => i,
        Err(e) => {
            log::error!("could not create the menu bar icon: {e}");
            return;
        }
    };
    TRAY.with(|t| *t.borrow_mut() = Some(Tray { icon, popover: None, visible: false, _monitor: None }));

    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let tx = std::sync::Mutex::new(tx);
    tray_icon::TrayIconEvent::set_event_handler(Some(move |e: tray_icon::TrayIconEvent| {
        if let tray_icon::TrayIconEvent::Click {
            button: tray_icon::MouseButton::Left,
            button_state: tray_icon::MouseButtonState::Down,
            ..
        } = e
        {
            let _ = tx.lock().map(|t| t.send(()));
        }
    }));
    let model = model.clone();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_millis(50)).await;
            while rx.try_recv().is_ok() {
                let model = model.clone();
                cx.update(|cx| toggle(&model, cx));
            }
        }
    })
    .detach();
}

#[cfg(target_os = "macos")]
fn icon_rect() -> Option<mac::Rect> {
    TRAY.with(|t| {
        let t = t.borrow();
        let item = t.as_ref()?.icon.ns_status_item()?;
        mac::status_item_screen_rect(objc2::rc::Retained::as_ptr(&item) as *mut c_void)
    })
}

/// The icon in the notification area. tray-icon gives physical pixels with
/// the origin at the top left.
#[cfg(not(target_os = "macos"))]
fn icon_rect() -> Option<mac::Rect> {
    TRAY.with(|t| {
        let r = t.borrow().as_ref()?.icon.rect()?;
        Some(mac::rect_from_physical(r.position.x, r.position.y, f64::from(r.size.width), f64::from(r.size.height)))
    })
}

pub fn toggle(model: &Entity<AppModel>, cx: &mut App) {
    // A click on the icon can take focus from the popover first, which closes
    // it. That click means "close", so it must not open the popover again.
    let just_closed = LAST_FOCUS_CLOSE.with(|t| t.get().is_some_and(|at| at.elapsed() < Duration::from_millis(400)));
    if is_visible() {
        hide(cx)
    } else if !just_closed {
        show(model, cx)
    }
}

/// Close the popover and give focus back to the app the user came from.
pub fn hide(cx: &mut App) {
    hide_popover(cx);
    crate::app::appkit_later(cx, mac::deactivate_app);
}

/// Close the popover but keep Sayso active: for "Open Sayso", and for a
/// click into another Sayso window.
fn hide_popover(cx: &mut App) {
    let handle = TRAY.with(|t| {
        let mut t = t.borrow_mut();
        let t = t.as_mut()?;
        t.visible = false;
        t._monitor = None;
        t.popover
    });
    // Deferred: a click handler in the popover runs inside the popover
    // window's update, and GPUI refuses a nested update of the same window.
    if let Some(h) = handle {
        cx.defer(move |cx| {
            if let Ok(Some(ns)) = h.update(cx, |_, window, _| ns_window(window)) {
                crate::app::appkit_later(cx, move || mac::order_out(ns));
            }
        });
    }
}

fn is_visible() -> bool {
    TRAY.with(|t| t.borrow().as_ref().is_some_and(|t| t.visible))
}

fn show(model: &Entity<AppModel>, cx: &mut App) {
    let existing = TRAY.with(|t| t.borrow().as_ref().and_then(|t| t.popover));
    let handle = match existing {
        Some(h) => h,
        None => {
            let model2 = model.clone();
            // Plain GPUI, not `gpui_kit::open_window`: the kit's root paints the
            // theme background over the whole window, and this window is
            // larger than the sheet.
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.), px(0.)), size: size(px(WINDOW_W), px(WINDOW_H)) })),
                    titlebar: None,
                    focus: true,
                    show: false,
                    kind: WindowKind::PopUp,
                    is_movable: false,
                    is_resizable: false,
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                move |window, cx| cx.new(|cx| PopoverView::new(model2, window, cx)),
            );
            match opened {
                Ok(h) => {
                    let h: AnyWindowHandle = h.into();
                    // The sheet draws its own shadow. Without this, the window
                    // rectangle shows as a frame.
                    if let Ok(Some(ns)) = h.update(cx, |_, window, _| ns_window(window)) {
                        crate::app::appkit_later(cx, move || mac::make_clear(ns));
                    }
                    TRAY.with(|t| {
                        if let Some(t) = t.borrow_mut().as_mut() {
                            t.popover = Some(h);
                        }
                    });
                    h
                }
                Err(e) => {
                    log::error!("could not open the popover: {e:#}");
                    return;
                }
            }
        }
    };
    let rect = icon_rect();
    let origin = rect.map(|r| {
        let screens = mac::screens();
        let screen = screens.iter().find(|s| {
            r.x >= s.frame.x && r.x <= s.frame.x + s.frame.width && r.y >= s.frame.y && r.y <= s.frame.y + s.frame.height
        });
        let screen_right = screen.map(|s| s.visible_frame.x + s.visible_frame.width).unwrap_or(f64::MAX);
        // An icon in the lower half of its screen sits in a taskbar at the bottom.
        let above = screen.is_some_and(|s| r.y + r.height / 2.0 < s.frame.y + s.frame.height / 2.0);
        let (x, y, side) = placement(r, screen_right, above);
        SUBMENU_SIDE.with(|s| s.set(side));
        OPENS_ABOVE.with(|a| a.set(above));
        (x, y)
    });
    if let Ok(Some(ns)) = handle.update(cx, |_, window, _| ns_window(window)) {
        crate::app::appkit_later(cx, move || place_and_show(ns, origin));
    }
    let monitor = mac::install_global_click_monitor(move |p| {
        // A click on our own icon toggles; let the tray handler do it.
        if let Some(r) = rect
            && p.x >= r.x && p.x <= r.x + r.width && p.y >= r.y && p.y <= r.y + r.height {
                return;
            }
        HIDE_REQUESTED.with(|h| h.set(true));
    });
    TRAY.with(|t| {
        if let Some(t) = t.borrow_mut().as_mut() {
            t.visible = true;
            t._monitor = Some(monitor);
        }
    });
    model.update(cx, |m, cx| {
        m.refresh_permissions();
        cx.notify();
    });
}

thread_local! {
    static HIDE_REQUESTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// When the popover last closed because it lost focus.
    static LAST_FOCUS_CLOSE: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
    /// The side of the sheet where submenus open. `show` sets it for each open.
    static SUBMENU_SIDE: std::cell::Cell<SubmenuSide> = const { std::cell::Cell::new(SubmenuSide::Right) };
    /// The sheet opens above the icon, so it sits at the bottom of the window.
    static OPENS_ABOVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SubmenuSide {
    Right,
    Left,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Submenu {
    Model,
    Microphone,
}

/// What a pick in a submenu does to the app model.
type OnPick = Box<dyn Fn(&mut AppModel, &mut Context<AppModel>)>;

/// One line of a submenu.
struct SubmenuItem {
    label: String,
    /// Small text at the right, for example "Cloud".
    note: Option<&'static str>,
    selected: bool,
    on_pick: OnPick,
}

pub struct PopoverView {
    model: Entity<AppModel>,
    submenu: Option<Submenu>,
    _observe: Subscription,
    _activation: Subscription,
}

impl PopoverView {
    fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        // The global click monitor sees clicks in other apps only. Losing key
        // focus also covers a click into the Hub or another Sayso window.
        let activation = cx.observe_window_activation(window, |_, window, cx| {
            if !window.is_window_active() && is_visible() {
                LAST_FOCUS_CLOSE.with(|t| t.set(Some(std::time::Instant::now())));
                hide_popover(cx);
            }
        });
        // The next open starts with no submenu.
        cx.observe_window_activation(window, |this: &mut Self, window, cx| {
            if !window.is_window_active() && this.submenu.take().is_some() {
                cx.notify();
            }
        })
        .detach();
        // Close when the outside-click monitor asks (it runs outside GPUI).
        cx.spawn(async move |_, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(60)).await;
                if HIDE_REQUESTED.with(|h| h.replace(false)) {
                    cx.update(hide);
                }
            }
        })
        .detach();
        Self { model, submenu: None, _observe: observe, _activation: activation }
    }

    fn act(&self, cx: &mut App, f: impl FnOnce(&mut AppModel, &mut Context<AppModel>) + 'static) {
        hide(cx);
        let model = self.model.clone();
        // Let the previous app take focus back before acting.
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_millis(160)).await;
            model.update(cx, f);
        })
        .detach();
    }

    /// The models that can do a final pass now: local models on disk, and the
    /// models of the speech providers that are set up.
    fn model_items(&self, cx: &App) -> Vec<SubmenuItem> {
        let m = self.model.read(cx);
        let active = m.active_model().id;
        m.catalog()
            .into_iter()
            .filter(|info| {
                let status = m.status_of(&info.id);
                info.final_pass && if info.is_remote() { status.is_usable() } else { status.is_on_disk() }
            })
            .map(|info| {
                let id = info.id.clone();
                SubmenuItem {
                    selected: info.id == active,
                    note: info.is_remote().then_some("Cloud"),
                    label: info.name,
                    on_pick: Box::new(move |m, cx| m.set_active_model(id.clone(), cx)),
                }
            })
            .collect()
    }

    fn microphone_items(&self, cx: &App) -> Vec<SubmenuItem> {
        let m = self.model.read(cx);
        let current = m.config.audio.input_device.clone();
        let mut options: Vec<(Option<String>, String)> = vec![(None, "System default".into())];
        options.extend(m.input_devices().into_iter().map(|d| (Some(d.id), d.name)));
        // The config can hold a device name from a hand-edited file.
        let selected_at = options.iter().position(|(id, name)| match &current {
            None => id.is_none(),
            Some(cur) => id.as_ref() == Some(cur) || (id.is_some() && name == cur),
        });
        options
            .into_iter()
            .enumerate()
            .map(|(i, (id, label))| SubmenuItem {
                label,
                note: None,
                selected: Some(i) == selected_at,
                on_pick: Box::new(move |m, cx| {
                    let id = id.clone();
                    m.edit_config(cx, |c| c.audio.input_device = id)
                }),
            })
            .collect()
    }

    /// The floating list of a row. `footer` is a last line that opens the Hub.
    fn submenu(&self, kind: Submenu, footer: Option<(&'static str, Route)>, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let (items, empty) = match kind {
            Submenu::Model => (self.model_items(cx), "No model is downloaded yet."),
            Submenu::Microphone => (self.microphone_items(cx), "No microphone found."),
        };
        let mut rows = div().id("submenu-rows").flex().flex_col().max_h(px(SUBMENU_MAX_H)).overflow_y_scroll();
        if items.is_empty() {
            rows = rows.child(div().px(px(10.)).py(px(8.)).child(text::ui(empty, 13., FontWeight::NORMAL, c.graphite)));
        }
        for (i, item) in items.into_iter().enumerate() {
            let SubmenuItem { label, note, selected, on_pick } = item;
            rows = rows.child(
                div()
                    .id(("submenu-item", i))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .h(px(30.))
                    .px(px(8.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .hover(|s| s.bg(c.deboss))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.submenu = None;
                        this.model.update(cx, |m, cx| on_pick(m, cx));
                        cx.notify();
                    }))
                    .child(div().flex_none().w(px(14.)).when(selected, |d| d.child(icon(Icon::Check, 12., c.ink))))
                    .child(text::ui(label, 13., if selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }, c.ink).flex_1().truncate())
                    .when_some(note, |d, note| d.child(text::ui(note, 11., FontWeight::NORMAL, c.graphite).flex_none())),
            );
        }
        let mut list = div()
            .id("submenu")
            .flex()
            .flex_col()
            .w(px(SUBMENU_W))
            .p(px(4.))
            .rounded(px(12.))
            .bg(c.sheet_raised)
            .shadow(sayso_ui::paper::floating(&c))
            .font_family(UI)
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.submenu = None;
                cx.notify();
            }))
            .child(rows);
        if let Some((label, route)) = footer {
            list = list.child(
                div()
                    .id("submenu-footer")
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(30.))
                    .mt(px(4.))
                    .pl(px(30.))
                    .pr(px(8.))
                    .rounded(px(7.))
                    .border_t_1()
                    .border_color(c.rule)
                    .cursor_pointer()
                    .hover(|s| s.bg(c.deboss))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.submenu = None;
                        hide_popover(cx);
                        this.model.update(cx, |m, cx| m.open_hub(route, cx));
                    }))
                    .child(text::ui(label, 13., FontWeight::NORMAL, c.ink)),
            );
        }
        // The row is 12 px from the edge of the sheet. The marker sits at the
        // near corner of the list, and the list keeps inside the window.
        let reach = 12. + SUBMENU_GAP;
        let (marker, anchor) = match SUBMENU_SIDE.with(|s| s.get()) {
            SubmenuSide::Right => (div().absolute().top(px(-4.)).right(px(-reach)), Anchor::TopLeft),
            SubmenuSide::Left => (div().absolute().top(px(-4.)).left(px(-reach)), Anchor::TopRight),
        };
        marker.child(deferred(anchored().anchor(anchor).snap_to_window_with_margin(px(8.)).child(list)).with_priority(2))
    }
}

impl Render for PopoverView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let (status, _detail, dot) = m.engine_summary();
        let recording = matches!(m.state(), State::Recording { .. });
        let toggle_caps = m.config.hotkeys.toggle.map(|h| h.keycaps()).unwrap_or_default();
        let paste_caps = m.config.hotkeys.paste_last.map(|h| h.keycaps().join("")).unwrap_or_default();
        let last = m.recent.first().cloned();
        let words_today = sayso_core::stats::format_count(m.stats.words_today);
        let active_style = m.config.ai.active_style.clone();
        let styles: Vec<(String, String, Hsla)> = m
            .styles
            .styles()
            .map(|s| (s.id.clone(), s.name.clone(), sayso_core::ink::Rgb::parse(&s.ink).map(sayso_ui::theme::hsla).unwrap_or(c.ink)))
            .collect();
        let model_name = m.active_model().name;
        let devices = m.input_devices();
        let device_name = m
            .config
            .audio
            .input_device
            .as_ref()
            .and_then(|cur| devices.iter().find(|d| &d.id == cur || &d.name == cur).map(|d| d.name.clone()))
            .or_else(|| devices.iter().find(|d| d.is_default).map(|d| d.name.clone()))
            .unwrap_or_else(|| "System default".into());
        let side = SUBMENU_SIDE.with(|s| s.get());
        let open = self.submenu;

        let dictate_label = if recording { "Stop dictation" } else { "Start dictation" };
        let mut sheet = div()
            .id("popover-sheet")
            .relative()
            .flex()
            .flex_col()
            .w(px(WIDTH))
            .p(px(8.))
            .gap(px(6.))
            .rounded(px(16.))
            .bg(c.sheet)
            .shadow(sayso_ui::paper::floating(&c))
            .font_family(UI)
            // A click on the sheet must not reach the clear part of the window.
            .occlude()
            .on_key_down(cx.listener(|_, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    hide(cx);
                }
            }))
            .child(grain(Grain::Sheet, px(16.), cx))
            // Status.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(10.))
                    .pt(px(10.))
                    .pb(px(6.))
                    .child(status_dot(dot(&c), 8.))
                    .child(text::ui(status, 14., FontWeight::SEMIBOLD, c.ink).flex_1())
                    .child(text::ui(format!("{words_today} words today"), 12., FontWeight::NORMAL, c.graphite)),
            )
            // The big ink button.
            .child(
                div()
                    .id("dictate")
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .h(px(52.))
                    .pl(px(14.))
                    .pr(px(12.))
                    .rounded(px(12.))
                    .ink_button(&c)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.act(cx, |m, cx| m.toggle_dictation(cx))))
                    .child(status_dot(if c.is_dark() { c.accent } else { sayso_ui::theme::hsla(sayso_core::ink::Rgb(0x8D9BE6)) }, 12.))
                    .child(text::ui(dictate_label, 15., FontWeight::SEMIBOLD, c.on_ink).flex_1())
                    .child(
                        div().flex().gap(px(3.)).children(toggle_caps.into_iter().map(|k| {
                            div()
                                .flex()
                                .items_center()
                                .h(px(22.))
                                .px(px(6.))
                                .rounded(px(5.))
                                .bg(c.on_ink.opacity(0.12))
                                .child(text::mono(k, 11., c.on_ink.opacity(0.85)))
                        })),
                    ),
            );

        // Last transcript.
        if let Some(entry) = last {
            let text_copy = entry.final_text.clone();
            let text_paste = entry.final_text.clone();
            let app = entry.app.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| "Sayso".into());
            let time = entry.created_at.with_timezone(&chrono::Local).format("%-H:%M").to_string();
            let model_copy = self.model.clone();
            sheet = sheet.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .mt(px(2.))
                    .px(px(14.))
                    .py(px(12.))
                    .rounded(px(12.))
                    .debossed(&c)
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .child(text::caps(format!("Last · {app} · {time}"), &c))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .id("copy-last")
                                            .cursor_pointer()
                                            .on_click(move |_, _, cx| {
                                                model_copy.read(cx).copy_text(&text_copy);
                                                hide(cx);
                                            })
                                            .child(text::ui("Copy", 12., FontWeight::SEMIBOLD, c.accent)),
                                    )
                                    .child(
                                        div()
                                            .id("paste-last")
                                            .cursor_pointer()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                let t = text_paste.clone();
                                                this.act(cx, move |m, cx| m.insert_text(t, cx))
                                            }))
                                            .child(text::ui(format!("Paste {paste_caps}"), 12., FontWeight::SEMIBOLD, c.accent)),
                                    ),
                            ),
                    )
                    .child(text::serif(entry.final_text.replace('\n', " "), 14., c.ink).line_clamp(3).text_ellipsis()),
            );
        }

        // Styles.
        let mut chips = div().flex().flex_wrap().gap(px(6.));
        for (i, (id, name, color)) in styles.into_iter().enumerate() {
            let selected = id == active_style;
            let model = self.model.clone();
            chips = chips.child(
                Chip::new(("style", i), name, selected)
                    .dot(color)
                    .on_click(move |_, _, cx| model.update(cx, |m, cx| m.set_active_style(&id, cx))),
            );
        }
        sheet = sheet.child(
            div().flex().flex_col().gap(px(8.)).px(px(6.)).pt(px(10.)).pb(px(4.)).child(text::caps("Style", &c).px(px(4.))).child(chips),
        );

        // Model and microphone rows. Each opens a submenu beside the sheet, on
        // hover or on a click, as a native menu does.
        let row = |kind: Submenu, id: &'static str, ic: Icon, label: &'static str, value: String, cx: &mut Context<Self>| {
            let active = open == Some(kind);
            div()
                .id(id)
                .relative()
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(38.))
                .px(px(8.))
                .border_t_1()
                .border_color(c.rule)
                .cursor_pointer()
                .when(active, |d| d.bg(c.deboss.opacity(0.5)))
                .hover(|s| s.bg(c.deboss.opacity(0.5)))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered && this.submenu != Some(kind) {
                        this.submenu = Some(kind);
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.submenu = Some(kind);
                    cx.notify();
                }))
                .when(side == SubmenuSide::Left, |d| d.child(icon(Icon::ChevronLeft, 10., c.pencil)))
                .child(icon(ic, 15., c.graphite))
                .child(text::ui(label, 14., FontWeight::NORMAL, c.ink).flex_1())
                .child(text::ui(value, 13., FontWeight::NORMAL, c.graphite).max_w(px(150.)).truncate())
                .when(side == SubmenuSide::Right, |d| d.child(icon(Icon::ChevronRight, 10., c.pencil)))
        };
        let mut model_row = row(Submenu::Model, "model-row", Icon::Models, "Model", model_name, cx);
        let mut mic_row = row(Submenu::Microphone, "mic-row", Icon::Mic, "Microphone", device_name, cx);
        match open {
            Some(Submenu::Model) => model_row = model_row.child(self.submenu(Submenu::Model, Some(("Manage models…", Route::Models)), cx)),
            Some(Submenu::Microphone) => mic_row = mic_row.child(self.submenu(Submenu::Microphone, None, cx)),
            None => {}
        }
        let rows = div().flex().flex_col().px(px(4.)).pt(px(6.)).child(model_row).child(mic_row);
        sheet = sheet.child(rows);

        // Footer.
        sheet = sheet.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(4.))
                .pt(px(8.))
                .pb(px(2.))
                .border_t_1()
                .border_color(c.rule)
                .child(div().flex_1().child(Button::new("open", "Open Sayso").full_width().on_click(cx.listener(|this, _, _, cx| {
                    hide_popover(cx);
                    this.model.update(cx, |m, cx| m.open_hub(Route::Home, cx));
                }))))
                .child(Button::new("quit", "Quit").ghost().on_click(|_, _, cx| cx.quit())),
        );

        // The sheet takes the side of the window under the icon. The other
        // side is clear and holds the open submenu.
        div()
            .size_full()
            .flex()
            .map(|d| if OPENS_ABOVE.with(|a| a.get()) { d.items_end() } else { d.items_start() })
            .when(side == SubmenuSide::Left, |d| d.justify_end())
            .on_mouse_down(MouseButton::Left, |_, _, cx| hide(cx))
            .child(sheet)
    }
}

use gpui_kit::prelude::FluentBuilder as _;

#[cfg(test)]
mod tests {
    // Not `super::*`: the GPUI glob has its own `test` macro.
    use super::{SubmenuSide, WIDTH, WINDOW_H, WINDOW_W, mac, placement};

    fn icon_at(x: f64) -> mac::Rect {
        mac::Rect { x, y: 1000.0, width: 24.0, height: 24.0 }
    }

    #[test]
    fn submenus_open_to_the_right_when_the_screen_has_room() {
        let (x, y, side) = placement(icon_at(800.0), 1728.0, false);
        assert_eq!(side, SubmenuSide::Right);
        // The sheet is centered under the icon, at the left of the window.
        assert_eq!(x, 800.0 + 12.0 - WIDTH as f64 / 2.0);
        assert_eq!(y, 1000.0 - WINDOW_H as f64 - 4.0);
    }

    #[test]
    fn a_taskbar_icon_at_the_bottom_opens_the_sheet_above_it() {
        let (x, y, side) = placement(icon_at(800.0), 1728.0, true);
        assert_eq!(side, SubmenuSide::Right);
        assert_eq!(x, 800.0 + 12.0 - WIDTH as f64 / 2.0);
        // Cocoa y: the window's bottom edge is 4 points above the icon's top edge.
        assert_eq!(y, 1000.0 + 24.0 + 4.0);
    }

    #[test]
    fn submenus_open_to_the_left_near_the_right_edge_of_the_screen() {
        let screen_right = 1728.0;
        let (x, _, side) = placement(icon_at(1300.0), screen_right, false);
        assert_eq!(side, SubmenuSide::Left);
        // The sheet is at the right of the window, and it stays under the icon.
        let sheet_x = x + (WINDOW_W - WIDTH) as f64;
        assert_eq!(sheet_x, 1300.0 + 12.0 - WIDTH as f64 / 2.0);
        assert!(sheet_x + WIDTH as f64 <= screen_right - 8.0);
    }

    #[test]
    fn the_sheet_stays_on_the_screen_under_an_icon_at_the_edge() {
        let screen_right = 1728.0;
        let (x, _, side) = placement(icon_at(1700.0), screen_right, false);
        assert_eq!(side, SubmenuSide::Left);
        let sheet_x = x + (WINDOW_W - WIDTH) as f64;
        assert_eq!(sheet_x + WIDTH as f64, screen_right - 8.0);
    }
}
