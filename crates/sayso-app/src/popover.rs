//! The menu bar icon and its themed popover (Paper page "Menu bar").
//!
//! The status item has no NSMenu. A click shows our own panel under the icon;
//! a click outside it, a second icon click, or Esc closes it.

use crate::hub::Route;
use crate::model::AppModel;
use gpui_kit::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use sayso_core::dictation::State;
use sayso_platform_macos::window as mac;
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
const HEIGHT: f32 = 470.;

/// The NSView of a GPUI window, as the pointer the mac helpers take.
pub fn ns_window(window: &Window) -> Option<*mut c_void> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr()),
        _ => None,
    }
}

struct Tray {
    icon: tray_icon::TrayIcon,
    popover: Option<AnyWindowHandle>,
    visible: bool,
    _monitor: Option<mac::MonitorToken>,
}

/// Put the popover under the menu bar icon and show it. Runs outside a GPUI update.
fn place_and_show(ns: *mut c_void, rect: Option<mac::Rect>) {
    if let Some(r) = rect {
        // Cocoa coordinates: the popover's top edge meets the icon's bottom edge.
        let screen_right = mac::screens()
            .iter()
            .find(|s| r.x >= s.frame.x && r.x <= s.frame.x + s.frame.width)
            .map(|s| s.visible_frame.x + s.visible_frame.width)
            .unwrap_or(f64::MAX);
        let x = (r.x + r.width / 2.0 - WIDTH as f64 / 2.0).min(screen_right - WIDTH as f64 - 8.0);
        mac::set_frame_origin(ns, x, r.y - HEIGHT as f64 - 4.0);
    }
    mac::show_and_focus(ns);
}

thread_local! {
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

fn tray_icon_image() -> Option<tray_icon::Icon> {
    let bytes = sayso_ui::assets::bytes("app/tray.png")?;
    let img = image::load_from_memory(&bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), w, h).ok()
}

pub fn install(model: &Entity<AppModel>, cx: &mut App) {
    let mut builder = tray_icon::TrayIconBuilder::new().with_tooltip("Sayso");
    if let Some(icon) = tray_icon_image() {
        builder = builder.with_icon_templated(icon);
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

fn icon_rect() -> Option<mac::Rect> {
    TRAY.with(|t| {
        let t = t.borrow();
        let item = t.as_ref()?.icon.ns_status_item()?;
        mac::status_item_screen_rect(objc2::rc::Retained::as_ptr(&item) as *mut c_void)
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
            let opened = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds { origin: point(px(0.), px(0.)), size: size(px(WIDTH), px(HEIGHT)) })),
                    titlebar: None,
                    focus: true,
                    show: false,
                    kind: WindowKind::PopUp,
                    is_movable: false,
                    is_resizable: false,
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                cx,
                move |window, cx| cx.new(|cx| PopoverView::new(model2, window, cx)),
            );
            match opened {
                Ok((h, _)) => {
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
    if let Ok(Some(ns)) = handle.update(cx, |_, window, _| ns_window(window)) {
        crate::app::appkit_later(cx, move || place_and_show(ns, rect));
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
}

pub struct PopoverView {
    model: Entity<AppModel>,
    mic_open: bool,
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
        Self { model, mic_open: false, _observe: observe, _activation: activation }
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
        let current_device = m.config.audio.input_device.clone();
        let device_name = current_device
            .as_ref()
            .and_then(|cur| devices.iter().find(|d| &d.id == cur || &d.name == cur).map(|d| d.name.clone()))
            .or_else(|| devices.iter().find(|d| d.is_default).map(|d| d.name.clone()))
            .unwrap_or_else(|| "System default".into());

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

        // Model and microphone rows.
        let row = |id: &'static str, ic: Icon, label: &'static str, value: String| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(38.))
                .px(px(8.))
                .border_t_1()
                .border_color(c.rule)
                .cursor_pointer()
                .hover(|s| s.bg(c.deboss.opacity(0.5)))
                .child(icon(ic, 15., c.graphite))
                .child(text::ui(label, 14., FontWeight::NORMAL, c.ink).flex_1())
                .child(text::ui(value, 13., FontWeight::NORMAL, c.graphite).max_w(px(150.)).truncate())
                .child(icon(Icon::ChevronRight, 10., c.pencil))
        };
        let mut rows = div()
            .flex()
            .flex_col()
            .px(px(4.))
            .pt(px(6.))
            .child(row("model-row", Icon::Models, "Model", model_name).on_click(cx.listener(|this, _, _, cx| {
                hide_popover(cx);
                this.model.update(cx, |m, cx| m.open_hub(Route::Models, cx));
            })))
            .child(row("mic-row", Icon::Mic, "Microphone", device_name).on_click(cx.listener(|this, _, _, cx| {
                this.mic_open = !this.mic_open;
                cx.notify();
            })));
        if self.mic_open {
            let mut list = div().flex().flex_col().pl(px(32.)).pb(px(4.));
            let mut options: Vec<(Option<String>, String)> = vec![(None, "System default".into())];
            options.extend(devices.into_iter().map(|d| (Some(d.id.clone()), d.name)));
            for (i, (value, label)) in options.into_iter().enumerate() {
                let selected = value == current_device;
                list = list.child(
                    div()
                        .id(("mic", i))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .h(px(30.))
                        .px(px(8.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|s| s.bg(c.deboss.opacity(0.5)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let v = value.clone();
                            this.mic_open = false;
                            this.model.update(cx, |m, cx| m.edit_config(cx, |c| c.audio.input_device = v));
                        }))
                        .child(div().w(px(14.)).when(selected, |d| d.child(icon(Icon::Check, 13., c.ink))))
                        .child(text::ui(label, 13., if selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }, c.ink)),
                );
            }
            rows = rows.child(list);
        }
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

        div().size_full().flex().flex_col().items_center().p(px(0.)).child(sheet)
    }
}

use gpui_kit::prelude::FluentBuilder as _;
