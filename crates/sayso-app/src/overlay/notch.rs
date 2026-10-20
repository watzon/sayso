//! The notch style of the overlay (Paper page "Overlay · Notch").
//!
//! The notch is true black, so it joins the camera housing of a MacBook. While
//! a dictation runs, it opens to the sides in the menu bar: the left wing and
//! the right wing. The live preview and the notices feed out below it on a
//! slip of paper. When no dictation runs, nothing shows.
//!
//! On a display without a camera notch, Sayso draws a notch of the same size
//! at the top center of the display.

use super::{clock, countdown_ring};
use crate::model::{AppModel, NoticeIcon, OverlayNotice};
use crate::shell::ScreenInfo;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::config::{self, OverlayStyle};
use sayso_core::dictation::{Notice, State};
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::fonts::{Face, LocaleFonts as _, SERIF};
use sayso_ui::{Colors, text};
use std::cell::Cell;
use std::rc::Rc;

/// The size of a drawn notch, on a display without one: about the notch of a
/// 14-inch MacBook Pro at the default scale.
const DRAWN_WIDTH: f32 = 185.;
const DRAWN_HEIGHT: f32 = 32.;
/// The narrowest wing. A wing grows to fit a longer label.
const WING: f32 = 76.;
/// The concave corners where the notch meets the top edge of the display.
const SHOULDER: f32 = 5.;
/// The bottom corners of the open notch.
const RADIUS: f32 = 12.;
/// Space between a wing's content and its outer edge.
const WING_PAD: f32 = 13.;
/// The slip is this much narrower than the open notch on each side.
const SLIP_INSET: f32 = 10.;

/// The size of the notch on one display, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Notch {
    pub width: f32,
    pub height: f32,
}

impl Notch {
    /// The notch of a display: its camera housing, or a drawn notch.
    pub fn of(screen: Option<&ScreenInfo>) -> Self {
        match screen.and_then(|s| s.notch) {
            Some(n) => Notch { width: n.width as f32, height: n.height as f32 },
            None => Notch { width: DRAWN_WIDTH, height: DRAWN_HEIGHT },
        }
    }

    /// The notch for a display, or None when the display uses the pill.
    pub fn for_display(o: &config::Overlay, screen: Option<&ScreenInfo>) -> Option<Self> {
        if o.style != OverlayStyle::Notch {
            return None;
        }
        // Only a Mac has displays with a camera notch. Elsewhere every display
        // gets a drawn notch.
        let hardware = screen.is_some_and(|s| s.notch.is_some());
        if cfg!(target_os = "macos") && !hardware && !o.notch_on_other_displays {
            return None;
        }
        Some(Self::of(screen))
    }

    /// The width of a slip, with the wings at their narrowest.
    fn slip_width(self) -> f32 {
        self.width + 2. * (WING - SLIP_INSET)
    }
}

/// The horizontal center of the notch of a display, in Cocoa coordinates.
pub fn center_x(screen: &ScreenInfo) -> f64 {
    match screen.notch {
        Some(n) => n.x + n.width / 2.,
        None => screen.frame.x + screen.frame.width / 2.,
    }
}

/// What the notch shows for one frame.
pub struct Scene<'a> {
    pub state: &'a State,
    /// A notice when no dictation runs (hotkey blocked, model not ready).
    pub blocked: Option<&'a OverlayNotice>,
    pub hovered: bool,
    pub levels: Vec<f32>,
    /// Seconds for the ink animation. Zero with reduce motion.
    pub t: f32,
    pub now_ms: u64,
    pub incognito: bool,
    /// The live preview, when it is on and has text.
    pub preview: Option<(&'a str, &'a str)>,
    /// The live preview is on, but its model still loads.
    pub preview_loading: bool,
    /// The name of the processing stage.
    pub stage: String,
    /// How far the ink line of the processing stage has drawn.
    pub progress: f32,
    pub model: Entity<AppModel>,
    /// The content widths of the two wings in the last frame.
    pub wings: Wings,
}

/// The content widths of the left and the right wing, kept by the view.
pub type Wings = Rc<Cell<[f32; 2]>>;

/// The notch for a scene. None when nothing shows.
pub fn body(n: Notch, s: Scene, c: &Colors, dark: &Colors) -> Option<Div> {
    let wings = s.wings.clone();
    let pale = dark.ink;
    if let Some(notice) = s.blocked.filter(|_| matches!(s.state, State::Idle)) {
        let ic = match notice.icon {
            NoticeIcon::Info => Icon::Info,
            NoticeIcon::Microphone => Icon::Mic,
            NoticeIcon::Style => Icon::Styles,
            NoticeIcon::Incognito => Icon::Wordmark,
        };
        return Some(card(n, &wings, move || icon(ic, 14., pale).into_any_element(), empty, Some(notice_slip(n, &notice.title, notice.detail.as_deref(), c))));
    }
    let body = match s.state {
        State::Idle => return None,
        State::Recording { started_ms, .. } => {
            let elapsed = s.now_ms.saturating_sub(*started_ms);
            let (hovered, incognito, graphite) = (s.hovered, s.incognito, dark.graphite);
            let accent = dark.accent;
            let left = move || {
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    // A click anywhere stops, so hover shows a stop button, as on the pill.
                    .child(if hovered {
                        div().size(px(9.)).rounded(px(2.5)).bg(accent)
                    } else {
                        div().size(px(8.)).rounded_full().bg(accent).shadow(vec![
                            BoxShadow::new(px(0.), px(0.), accent.opacity(0.18)).spread_radius(px(3.)),
                            BoxShadow::new(px(0.), px(0.), accent.opacity(0.55)).blur_radius(px(8.)),
                        ])
                    })
                    .child(text::mono(clock(elapsed), 12., graphite))
                    .when(incognito, |d| d.child(text::ui(sayso_i18n::t!("overlay.incognito"), 12., FontWeight::MEDIUM, graphite)))
                    .into_any_element()
            };
            let (levels, t) = (s.levels, s.t);
            let right = move || ink_waveform(levels.clone(), t, pale).w(px(50.)).h(px(20.)).into_any_element();
            let slip = match s.preview {
                Some((committed, tentative)) => Some(preview_slip(n, committed, tentative, c)),
                None if s.preview_loading => Some(loading_slip(n, s.progress, c)),
                None => None,
            };
            card(n, &wings, left, right, slip)
        }
        State::Processing { .. } => {
            let (stage, graphite) = (s.stage, dark.graphite);
            let left = move || {
                div().font_family(SERIF).locale_fonts(Face::Serif).italic().text_size(px(13.)).text_color(graphite).child(stage.clone()).into_any_element()
            };
            let (p, pencil) = (s.progress, dark.pencil);
            let right = move || div().w(px(58.)).h(px(12.)).child(ink_line(p, pale, pencil).size_full()).into_any_element();
            card(n, &wings, left, right, None)
        }
        State::Showing { notice, until_ms, .. } => match notice {
            Notice::Inserted { .. } => {
                let (accent, deboss) = (dark.accent, dark.deboss);
                let left = move || {
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(20.))
                        .rounded_full()
                        .bg(accent)
                        .shadow(vec![BoxShadow::new(px(0.), px(0.), accent.opacity(0.35)).blur_radius(px(8.))])
                        .child(icon(Icon::Check, 11., deboss))
                        .into_any_element()
                };
                let right = move || text::ui(sayso_i18n::t!("overlay.inserted"), 13., FontWeight::SEMIBOLD, pale).into_any_element();
                card(n, &wings, left, right, None)
            }
            Notice::InsertedRaw { reason } => card(
                n,
                &wings,
                move || icon(Icon::Check, 14., pale).into_any_element(),
                empty,
                Some(notice_slip(n, &sayso_i18n::t!("overlay.inserted_raw"), Some(reason), c)),
            ),
            Notice::Cancelled => {
                let left_ms = until_ms.map(|u| u.saturating_sub(s.now_ms) as f32 / 5000.0).unwrap_or(0.0);
                let graphite = dark.graphite;
                let left = move || text::ui(sayso_i18n::t!("overlay.cancelled"), 13., FontWeight::MEDIUM, graphite).into_any_element();
                let (model, on) = (s.model, dark.deboss);
                let right = move || {
                    let model = model.clone();
                    sayso_ui::a11y::Pressable::new("undo")
                        .label(sayso_i18n::t!("overlay.a11y.undo_cancel"))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .h(px(22.))
                        .pl(px(6.))
                        .pr(px(9.))
                        .rounded_full()
                        .bg(pale)
                        .cursor_pointer()
                        .on_click(move |_, _, cx| model.update(cx, |m, cx| m.undo_cancel(cx)))
                        .child(countdown_ring(left_ms, on).size(px(12.)))
                        .child(text::ui(sayso_i18n::t!("overlay.undo"), 12., FontWeight::SEMIBOLD, on))
                        .into_any_element()
                };
                card(n, &wings, left, right, None)
            }
            Notice::InsertFailed { text: failed, reason } => {
                let (m1, m2, copy) = (s.model.clone(), s.model, failed.clone());
                let slip = slip(n, c)
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(text::ui(reason.clone(), 13., FontWeight::SEMIBOLD, c.danger))
                    .child(text::serif(failed.replace('\n', " "), 15., c.ink).line_clamp(4).text_ellipsis())
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(6.))
                            .child(Button::new("dismiss", sayso_i18n::t!("overlay.dismiss")).small().on_click(move |_, _, cx| m1.update(cx, |m, cx| m.dismiss_notice(cx))))
                            .child(Button::new("copy", sayso_i18n::t!("overlay.copy_text")).small().primary().on_click(move |_, _, cx| {
                                m2.update(cx, |m, cx| {
                                    m.copy_text(&copy);
                                    m.dismiss_notice(cx);
                                })
                            })),
                    )
                    .into_any_element();
                card(n, &wings, move || icon(Icon::Info, 14., pale).into_any_element(), empty, Some(slip))
            }
            Notice::Error { message } => card(n, &wings, move || icon(Icon::Info, 14., pale).into_any_element(), empty, Some(notice_slip(n, message, None, c))),
            Notice::NoSpeech => card(
                n,
                &wings,
                move || icon(Icon::Info, 14., pale).into_any_element(),
                empty,
                Some(notice_slip(n, &sayso_i18n::t!("overlay.error.no_speech"), None, c)),
            ),
            Notice::Discarded => return None,
        },
    };
    Some(body)
}

fn empty() -> AnyElement {
    div().into_any_element()
}

/// The open notch: the left wing, the notch, and the right wing in one black
/// shape, with an optional slip under it.
///
/// Both wings have the width of the wider one, so the middle stays on the
/// camera. Each wing measures its content into `wings` as it draws, and the
/// next frame uses the new widths.
fn card(n: Notch, wings: &Wings, left: impl FnOnce() -> AnyElement, right: impl FnOnce() -> AnyElement, slip: Option<AnyElement>) -> Div {
    let [l, r] = wings.get();
    let width = (l.max(r) + WING_PAD).max(WING);
    let wing = |content: AnyElement, side: usize| {
        let wings = wings.clone();
        let measure = canvas(
            move |b, window, _| {
                let mut w = wings.get();
                let now = f32::from(b.size.width);
                if (w[side] - now).abs() > 0.5 {
                    w[side] = now;
                    wings.set(w);
                    window.request_animation_frame();
                }
            },
            |_, _, _, _| {},
        );
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(width))
            .map(|d| if side == 1 { d.items_end().pr(px(WING_PAD)) } else { d.items_start().pl(px(WING_PAD)) })
            .child(div().relative().flex().items_center().h(px(n.height)).child(content).child(measure.absolute().top_0().left_0().size_full()))
    };
    let band = div()
        .relative()
        .flex()
        .flex_none()
        .h(px(n.height))
        .px(px(SHOULDER))
        .child(canvas(|_, _, _| (), |b, _, window, _| paint_notch(b, window)).absolute().top_0().left_0().size_full())
        .child(wing(left(), 0))
        .child(div().flex_none().w(px(n.width)))
        .child(wing(right(), 1));
    div().relative().flex().flex_col().items_center().child(band).children(slip)
}

/// The black shape of the open notch, with concave shoulders at the top
/// edge of the display and round bottom corners.
fn paint_notch(b: Bounds<Pixels>, window: &mut Window) {
    let (o, w, h) = (b.origin, b.size.width, b.size.height);
    let s = px(SHOULDER);
    let r = px(RADIUS).min(h / 2.);
    let at = |x: Pixels, y: Pixels| point(o.x + x, o.y + y);
    let mut pb = PathBuilder::fill();
    pb.move_to(at(px(0.), px(0.)));
    pb.line_to(at(w, px(0.)));
    pb.curve_to(at(w - s, s), at(w - s, px(0.)));
    pb.line_to(at(w - s, h - r));
    pb.curve_to(at(w - s - r, h), at(w - s, h));
    pb.line_to(at(s + r, h));
    pb.curve_to(at(s, h - r), at(s, h));
    pb.line_to(at(s, s));
    pb.curve_to(at(px(0.), px(0.)), at(s, px(0.)));
    pb.close();
    if let Ok(path) = pb.build() {
        window.paint_path(path, black());
    }
}

/// A slip of paper that feeds out from under the notch.
fn slip(n: Notch, c: &Colors) -> Div {
    div()
        .w(px(n.slip_width()))
        .pt(px(12.))
        .px(px(16.))
        .pb(px(13.))
        .rounded_b(px(12.))
        .bg(c.sheet)
        .shadow(vec![
            // The edge of the notch shades the top of the paper.
            BoxShadow::new(px(0.), px(7.), c.shadow(0.28)).blur_radius(px(6.)).spread_radius(px(-6.)).inset(),
            BoxShadow::new(px(0.), px(1.), c.shadow(0.14)).blur_radius(px(2.)),
            BoxShadow::new(px(0.), px(10.), c.shadow(0.14)).blur_radius(px(24.)),
        ])
}

fn notice_slip(n: Notch, title: &str, detail: Option<&str>, c: &Colors) -> AnyElement {
    slip(n, c)
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(text::ui(title.to_string(), 14., FontWeight::SEMIBOLD, c.ink))
        .when_some(detail.map(str::to_string), |d, s| d.child(text::ui(s, 13., FontWeight::NORMAL, c.graphite)))
        .into_any_element()
}

/// The slip of a live preview whose model still loads. The line height is
/// the same as the preview's, so the words take its place with no jump.
fn loading_slip(n: Notch, progress: f32, c: &Colors) -> AnyElement {
    slip(n, c)
        .px(px(18.))
        .flex()
        .items_center()
        .gap(px(10.))
        .child(div().flex_none().w(px(34.)).h(px(10.)).child(ink_line(progress, c.pencil, c.pencil.opacity(0.5)).size_full()))
        .child(
            div()
                .font_family(SERIF)
                .locale_fonts(Face::Serif)
                .italic()
                .text_size(px(15.))
                .line_height(px(23.))
                .text_color(c.pencil)
                .child(sayso_i18n::t!("overlay.preview_loading")),
        )
        .into_any_element()
}

/// The live preview on a slip: committed words in ink, the tentative tail in
/// pencil italic, and the caret. The same text as the pill's preview.
fn preview_slip(n: Notch, committed: &str, tentative: &str, c: &Colors) -> AnyElement {
    let (text, highlights) = super::preview_text(committed, tentative, 110, c);
    slip(n, c)
        .px(px(18.))
        .font_family(SERIF)
        .locale_fonts(Face::Serif)
        .text_size(px(16.))
        .line_height(px(23.))
        .text_color(c.ink)
        .child(StyledText::new(text).with_highlights(highlights))
        .into_any_element()
}
