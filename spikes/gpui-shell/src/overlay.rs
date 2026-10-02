//! S1: transparent, non-activating waveform overlay.
use crate::mac;
use gpui_kit::*;
use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

const SIZE: (f32, f32) = (280., 64.);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fix {
    None,
    /// `setHasShadow:NO` only.
    Shadow,
    /// shadow off + non-opaque + clear background.
    Full,
}

struct Overlay {
    start: Instant,
    frames: Rc<Cell<u32>>,
}

impl Render for Overlay {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.frames.set(self.frames.get() + 1);
        window.request_animation_frame();
        let t = self.start.elapsed().as_secs_f32();
        div().size_full().child(
            div()
                .size_full()
                .rounded(px(32.))
                .bg(rgba(0x101014e6))
                .child(canvas(|_, _, _| (), move |b, _, w, _| paint_wave(b, t, w)).size_full()),
        )
    }
}

/// Fake amplitude: slow envelope * two sines * pseudo-noise.
fn amp(x: f32, t: f32, layer: f32) -> f32 {
    let env = (x * std::f32::consts::PI).sin();
    let talk = 0.55 + 0.45 * (t * 1.9).sin() * (t * 0.7).cos();
    let noise = ((x * 37.0 + t * 5.0 + layer).sin() * 43758.5).fract().abs() * 0.15;
    env * talk * ((x * 9.0 + t * (3.0 + layer)).sin() * 0.8 + noise)
}

fn paint_wave(b: Bounds<Pixels>, t: f32, window: &mut Window) {
    const N: usize = 40;
    let (w, h) = (b.size.width, b.size.height);
    let pad = px(24.);
    for (layer, (alpha, width)) in [(0.35, 4.0), (0.6, 2.5), (1.0, 1.5)]
        .into_iter()
        .enumerate()
    {
        let pts: Vec<Point<Pixels>> = (0..N)
            .map(|i| {
                let x = i as f32 / (N - 1) as f32;
                point(
                    b.origin.x + pad + (w - pad * 2.) * x,
                    b.origin.y + h * 0.5 - h * 0.38 * amp(x, t, layer as f32),
                )
            })
            .collect();
        // Quadratic curve through segment midpoints = smooth line.
        let mut pb = PathBuilder::stroke(px(width));
        pb.move_to(pts[0]);
        for i in 1..N - 1 {
            let mid = point(
                (pts[i].x + pts[i + 1].x) / 2.,
                (pts[i].y + pts[i + 1].y) / 2.,
            );
            pb.curve_to(mid, pts[i]);
        }
        pb.line_to(pts[N - 1]);
        if let Ok(path) = pb.build() {
            window.paint_path(path, hsla(0.5, 0.9, 0.7, alpha));
        }
    }
}

pub fn open(cx: &mut App, fix: Fix, deactivate: bool) {
    let display = cx.primary_display().map(|d| d.bounds()).unwrap_or_default();
    let (w, h) = SIZE;
    let origin = point(
        display.origin.x + (display.size.width - px(w)) / 2.,
        display.origin.y + display.size.height - px(h) - px(96.),
    );
    let frames = Rc::new(Cell::new(0));
    let f = frames.clone();
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin,
                size: size(px(w), px(h)),
            })),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            // Default throttles frames of inactive windows (~25 fps measured); a PopUp is never active.
            inactive_frame_interval: None,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        move |window, cx| {
            println!(
                "[overlay] ns window found: {}",
                mac::ns_window(window).is_some()
            );
            cx.new(|_| Overlay {
                start: Instant::now(),
                frames: f,
            })
        },
    );
    let handle = match opened {
        Ok(h) => h,
        Err(e) => return eprintln!("[overlay] open_window failed: {e:#}"),
    };
    let win: Option<objc2::rc::Retained<NSWindow>> = handle
        .update(cx, |_, window, _| mac::ns_window(window))
        .ok()
        .flatten();
    if let (Some(win), Fix::Shadow | Fix::Full) = (&win, fix) {
        mac::clear_window_chrome(win, fix == Fix::Shadow);
        println!("[overlay] applied window fix: {fix:?}");
    }
    if let (true, Some(mtm)) = (deactivate, objc2::MainThreadMarker::new()) {
        mac::deactivate_app(mtm);
        println!("[overlay] called NSApp.deactivate()");
    }
    // Probe loop: t=1..=5 s.
    cx.spawn(async move |cx| {
        let mut last = (Instant::now(), 0u32);
        for sec in 1..=5u32 {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let n = frames.get();
            let fps = (n - last.1) as f64 / last.0.elapsed().as_secs_f64();
            last = (Instant::now(), n);
            let Some(win) = &win else { continue };
            // Exercise the other NSWindow knobs, one per second.
            match sec {
                2 => win.setLevel(25), // NSStatusWindowLevel
                3 => win.setCollectionBehavior(
                    NSWindowCollectionBehavior::CanJoinAllSpaces
                        | NSWindowCollectionBehavior::FullScreenAuxiliary
                        | NSWindowCollectionBehavior::Stationary
                        | NSWindowCollectionBehavior::IgnoresCycle,
                ),
                4 => win.setIgnoresMouseEvents(true),
                5 => win.setIgnoresMouseEvents(false),
                _ => {}
            }
            println!(
                "[overlay t={sec}s] frontmost={} appActive={} isKey={} isMain={} visible={} level={} behavior={:#x} ignoresMouse={} fps={fps:.1} frame={:?}",
                mac::frontmost_bundle_id(),
                objc2::MainThreadMarker::new().map(mac::app_is_active).unwrap_or(false),
                win.isKeyWindow(),
                win.isMainWindow(),
                win.isVisible(),
                win.level(),
                win.collectionBehavior().0,
                win.ignoresMouseEvents(),
                win.frame(),
            );
        }
        println!("[overlay] probe done");
    })
    .detach();
}
