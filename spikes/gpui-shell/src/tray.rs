//! S2: tray-icon status item (no NSMenu) + GPUI PopUp popover.
use crate::mac;
use block2::RcBlock;
use futures::{channel::mpsc::unbounded, StreamExt};
use gpui_kit::component::{button::*, switch::Switch, ActiveTheme};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSEventMask, NSWindow};
use objc2_foundation::{NSPoint, NSRect};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

const SIZE: (f32, f32) = (320., 420.);

struct Popover {
    win: Option<Retained<NSWindow>>,
    focus: FocusHandle,
    clicks: u32,
    sounds: bool,
}

impl Popover {
    fn hide(&self) {
        if let Some(w) = &self.win {
            w.orderOut(None);
        }
    }
}

impl Render for Popover {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let (fg, muted, border) = (t.foreground, t.muted_foreground, t.border);
        let langs = ["English (US)", "German", "Spanish", "Japanese", "French"];
        div()
            .id("popover")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, _| {
                if ev.keystroke.key == "escape" {
                    println!("[tray] Esc pressed -> hide");
                    this.hide();
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .text_color(fg)
            .child(div().text_lg().child("Sayso (spike)"))
            .child(
                Button::new("go")
                    .primary()
                    .label(format!("Start dictation ({})", self.clicks))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.clicks += 1;
                        cx.notify();
                    })),
            )
            .child(
                Switch::new("sounds")
                    .label("Play start/stop sounds")
                    .checked(self.sounds)
                    .on_click(cx.listener(|this, v: &bool, _, cx| {
                        this.sounds = *v;
                        cx.notify();
                    })),
            )
            .child(div().text_sm().text_color(muted).child("Languages"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .children(langs.iter().enumerate().map(|(i, l)| {
                        div()
                            .px_3()
                            .py_2()
                            .when(i > 0, |d| d.border_t_1().border_color(border))
                            .child(*l)
                    })),
            )
    }
}

fn status_item_frame(tray: &TrayIcon) -> Option<NSRect> {
    let mtm = MainThreadMarker::new()?;
    Some(tray.ns_status_item()?.button(mtm)?.window()?.frame())
}

/// Frame (Cocoa bottom-left coords) that puts the popover centred under the icon.
fn anchored_origin(icon: NSRect, win: &NSWindow) -> NSPoint {
    let (w, h) = (SIZE.0 as f64, SIZE.1 as f64);
    let mut x = icon.origin.x + icon.size.width / 2. - w / 2.;
    if let Some(screen) = win
        .screen()
        .or_else(|| objc2_app_kit::NSScreen::mainScreen(MainThreadMarker::new().unwrap()))
    {
        let vf = screen.visibleFrame();
        x = x.clamp(vf.origin.x + 4., vf.origin.x + vf.size.width - w - 4.);
    }
    NSPoint::new(x, icon.origin.y - h) // icon bottom edge == menu bar bottom
}

fn tray_icon_image() -> Icon {
    const N: u32 = 44;
    let mut rgba = Vec::new();
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - 21.5, y as f32 - 21.5);
            let a = if (dx * dx + dy * dy).sqrt() < 14. {
                255
            } else {
                0
            };
            rgba.extend_from_slice(&[0, 0, 0, a]);
        }
    }
    Icon::from_rgba(rgba, N, N).expect("icon")
}

fn contains(r: NSRect, p: NSPoint) -> bool {
    p.x >= r.origin.x
        && p.x <= r.origin.x + r.size.width
        && p.y >= r.origin.y
        && p.y <= r.origin.y + r.size.height
}

/// `auto_toggle`: seconds after which to fake a status-item click (for unattended runs).
pub fn open(cx: &mut App, auto_toggle: Option<u64>) {
    // 1. Hidden popover window (PopUp = NSPanel, nonactivating).
    let opened = crate::open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(200.), px(200.)),
                size(px(SIZE.0), px(SIZE.1)),
            ))),
            titlebar: None,
            focus: false,
            show: false,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            window_background: WindowBackgroundAppearance::Opaque,
            ..Default::default()
        },
        cx,
        |window, cx| {
            let win = mac::ns_window(window);
            cx.new(|cx| Popover {
                win,
                focus: cx.focus_handle(),
                clicks: 0,
                sounds: true,
            })
        },
    );
    let (handle, view) = match opened {
        Ok(v) => v,
        Err(e) => return eprintln!("[tray] open_window failed: {e:#}"),
    };
    let win = view.read(cx).win.clone().expect("ns window");

    // 2. Status item with no menu; forward clicks over a channel.
    let (tx, mut rx) = unbounded();
    let fake = tx.clone();
    TrayIconEvent::set_event_handler(Some(move |ev: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Down,
            ..
        } = ev
        {
            let _ = tx.unbounded_send(());
        }
    }));
    let tray = TrayIconBuilder::new()
        .with_icon_templated(tray_icon_image())
        .with_tooltip("Sayso spike")
        .build()
        .expect("tray icon");
    println!(
        "[tray] status item frame at start: {:?}",
        status_item_frame(&tray)
    );

    if let Some(secs) = auto_toggle {
        cx.spawn(async move |cx| {
            for _ in 0..2 {
                cx.background_executor()
                    .timer(Duration::from_secs(secs))
                    .await;
                let _ = fake.unbounded_send(());
            }
        })
        .detach();
    }

    // 3. Outside-click dismissal: global monitor (sees clicks in other apps).
    // The monitor also sees the click that hits our own icon; ignore it (the toggle handles that click).
    let icon_rect = Rc::new(Cell::new(NSRect::ZERO));
    let (w, ir) = (win.clone(), icon_rect.clone());
    let block = RcBlock::new(move |_ev: std::ptr::NonNull<NSEvent>| {
        let p = NSEvent::mouseLocation();
        if w.isVisible() && !contains(w.frame(), p) && !contains(ir.get(), p) {
            println!("[tray] outside click -> hide");
            w.orderOut(None);
        }
    });
    let monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
        NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown,
        &block,
    );

    // 4. Toggle on click-down. The task owns the tray + monitor, keeping them alive.
    cx.spawn(async move |cx| {
        let _keep = (monitor, block);
        let mut first = true;
        while rx.next().await.is_some() {
            if win.isVisible() {
                println!("[tray] icon clicked while open -> hide");
                win.orderOut(None);
                continue;
            }
            let Some(icon) = status_item_frame(&tray) else {
                continue;
            };
            icon_rect.set(icon);
            let origin = anchored_origin(icon, &win);
            win.setFrameOrigin(origin);
            win.makeKeyAndOrderFront(None);
            println!(
                "[tray] show: icon={icon:?} popover={:?} key={}",
                win.frame(),
                win.isKeyWindow()
            );
            let _ = handle.update(cx, |_, window, cx| {
                let f = view.read(cx).focus.clone();
                window.focus(&f, cx);
            });
            if std::mem::take(&mut first) {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
            }
        }
    })
    .detach();
}
