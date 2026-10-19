//! The overlay: the idle pill and every dictation state (Paper page "Overlay").
//!
//! One transparent, non-activating panel. It ignores the mouse except while
//! the cursor is over the visible card, so the empty area never blocks the
//! app underneath. The pill can be dragged anywhere and snaps softly to the
//! screen edges and to the top and bottom centers.
//!
//! With the notch style ([`notch`]), the same panel sits at the top center of
//! the display and shows only while a dictation runs.

mod notch;
mod placement;

use crate::model::{AppModel, NoticeIcon};
use gpui_kit::*;
use notch::Notch;
use placement::{Anchor, Follow, Placement};
use sayso_core::config::{self, OverlaySize, OverlayStyle};
use sayso_core::dictation::{Notice, Stage, State};
use sayso_core::ink::Appearance;
use sayso_core::stt::ModelStatus;
use sayso_ui::ActivePaper;
use sayso_ui::a11y::{A11yExt as _, Pressable, Role};
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::fonts::{Face, LocaleFonts as _, SERIF, UI};
use sayso_ui::paper::{self, PaperStyled};
use sayso_ui::text;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The overlay window size. Content is drawn inside it at the bottom center,
/// or at the top center when the pill is in the top half of a display.
pub const WIDTH: f32 = 520.;
pub const HEIGHT: f32 = 300.;
/// Space between the card and the edge of the window that holds it.
const MARGIN: f32 = 16.;

pub fn open(model: &Entity<AppModel>, cx: &mut App) {
    let placement = Placement::load(&model.read(cx).paths);
    let screens = crate::shell::screens();
    // The overlay opens on the primary display, and then follows the display in use.
    let notch = Notch::for_display(&model.read(cx).config.overlay, screens.first());
    let (origin, anchor) = match notch {
        Some(_) => (placement::notch_origin(screens.first()), Anchor::Top),
        None => placement.window_origin(cx),
    };
    let model2 = model.clone();
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size: size(px(WIDTH), px(HEIGHT)) })),
            titlebar: None,
            focus: false,
            show: true,
            kind: crate::shell::overlay_kind(notch.is_some()),
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            // A PopUp is never "active"; without this GPUI throttles it to ~25 fps.
            inactive_frame_interval: None,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        move |window, cx| cx.new(|cx| OverlayView::new(model2, placement, origin, anchor, notch, window, cx)),
    );
    if let Err(e) = opened {
        log::error!("could not open the overlay: {e:#}");
    }
}

pub struct OverlayView {
    model: Entity<AppModel>,
    placement: Placement,
    /// The visible card in window coordinates, captured during layout.
    card: Rc<Cell<Bounds<Pixels>>>,
    /// The window edge that holds the card. The live preview is on the other side of the pill.
    anchor: Anchor,
    /// The window origin in GPUI coordinates, where the overlay was put last.
    origin: Point<Pixels>,
    /// The notch of the display, with the notch style. None: the pill.
    notch: Option<Notch>,
    /// The content widths of the notch wings, from the last frame.
    wings: notch::Wings,
    /// The style settings that the window was opened with. A change opens a new window.
    style: (OverlayStyle, bool),
    hovered: bool,
    drag: Option<Drag>,
    start: Instant,
    /// When the current notice or stage started, for animations.
    phase_start: Instant,
    last_key: String,
    hidden: bool,
    configured: bool,
    follow: Follow,
    last_follow: Instant,
    /// A dictation was in the recording state at the last model change.
    recording: bool,
    _observe: Subscription,
}

/// A mouse press on the pill, which becomes a drag when the mouse moves.
#[derive(Clone, Copy)]
struct Drag {
    /// The mouse at the press, in global points.
    mouse: Point<Pixels>,
    /// The left of the window and the top of the card at the press, in global points.
    card: Point<Pixels>,
    card_height: Pixels,
    moved: bool,
}

/// The window bounds in global points, with the origin at the top left of the
/// primary display. `Window::bounds` on macOS is relative to the display of
/// the window, so it is wrong when the overlay is not on the primary display.
fn global_bounds(window: &Window, ns: crate::shell::NativeWindow) -> Bounds<Pixels> {
    let Some(frame) = crate::shell::frame(ns) else { return window.bounds() };
    let top = placement::main_screen_height() - (frame.y + frame.height) as f32;
    Bounds { origin: point(px(frame.x as f32), px(top)), size: size(px(frame.width as f32), px(frame.height as f32)) }
}

/// How often the overlay looks for the display in use.
const FOLLOW_EVERY: Duration = Duration::from_millis(250);

impl OverlayView {
    fn new(
        model: Entity<AppModel>,
        placement: Placement,
        origin: Point<Pixels>,
        anchor: Anchor,
        notch: Option<Notch>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe_in(&model, window, |view, _, window, cx| {
            view.reopen_when_style_changes(window, cx);
            view.reopen_when_recording_starts(window, cx);
            cx.notify();
        });
        let recording = matches!(model.read(cx).state(), State::Recording { .. });
        let style = style_key(&model.read(cx).config.overlay);
        let this = Self {
            model,
            placement,
            card: Rc::new(Cell::new(Bounds::default())),
            anchor,
            origin,
            notch,
            wings: Rc::new(Cell::new([0.; 2])),
            style,
            hovered: false,
            drag: None,
            start: Instant::now(),
            phase_start: Instant::now(),
            last_key: String::new(),
            hidden: false,
            configured: false,
            follow: Follow::default(),
            last_follow: Instant::now(),
            recording,
            _observe: observe,
        };
        Self::start_mouse_tracking(window, cx);
        this
    }

    /// Follow the display on a system where the overlay cannot move
    /// (Wayland): open a new overlay when a recording starts. The compositor
    /// puts a new layer surface on the display in use.
    fn reopen_when_recording_starts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let m = self.model.read(cx);
        let recording = matches!(m.state(), State::Recording { .. });
        let started = recording && !self.recording;
        self.recording = recording;
        if !started || crate::shell::can_place_windows() || !m.config.overlay.follow_display {
            return;
        }
        self.reopen(window, cx);
    }

    /// Open a new overlay when the user picks another style: the window kind
    /// and the place depend on it.
    fn reopen_when_style_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let style = style_key(&self.model.read(cx).config.overlay);
        if style != self.style {
            self.style = style;
            self.reopen(window, cx);
        }
    }

    /// Replace this overlay with a new one.
    fn reopen(&self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let old = window.window_handle();
        // The new overlay opens first, so that the pill does not flash on the same display.
        cx.defer(move |cx| {
            open(&model, cx);
            let _ = old.update(cx, |_, window, _| window.remove_window());
        });
    }

    /// The window origin, the anchor, and the notch for a display.
    fn place_on(&self, screens: &[crate::shell::ScreenInfo], display: u32, cx: &App) -> Option<(Point<Pixels>, Anchor, Option<Notch>)> {
        let screen = screens.iter().find(|s| s.id == display)?;
        match Notch::for_display(&self.model.read(cx).config.overlay, Some(screen)) {
            Some(n) => Some((placement::notch_origin(Some(screen)), Anchor::Top, Some(n))),
            None => self.placement.origin_on(screens, display).map(|(origin, anchor)| (origin, anchor, None)),
        }
    }

    /// True when the overlay hides while no dictation runs. The notch always does.
    fn hides_when_idle(&self, o: &config::Overlay) -> bool {
        self.notch.is_some() || !o.idle_pill || (o.hide_in_fullscreen && crate::shell::frontmost_app_is_fullscreen())
    }

    /// Move the overlay to the display of the mouse or of the focused window,
    /// whichever went to another display last.
    fn follow_display(&mut self, window: &mut Window, ns: crate::shell::NativeWindow, mouse: crate::shell::Point, cx: &mut Context<Self>) {
        if self.last_follow.elapsed() < FOLLOW_EVERY || !self.model.read(cx).config.overlay.follow_display {
            return;
        }
        self.last_follow = Instant::now();
        let screens = crate::shell::screens();
        let focus = crate::shell::focused_window_frame().and_then(|f| placement::display_at(&screens, f.x + f.width / 2.0, f.y + f.height / 2.0));
        let Some(target) = self.follow.target(placement::display_at(&screens, mouse.x, mouse.y), focus) else { return };
        if self.placement.display_of(&screens, global_bounds(window, ns), self.anchor) == Some(target) {
            return;
        }
        let Some((origin, anchor, notch)) = self.place_on(&screens, target, cx) else { return };
        if notch != self.notch {
            if notch.is_some() {
                crate::shell::allow_over_menu_bar(ns);
            }
            self.notch = notch;
            cx.notify();
        }
        self.move_to(ns, origin, anchor, cx);
    }

    /// Move the window, and draw the card at the edge of the window that goes with the new place.
    fn move_to(&mut self, ns: crate::shell::NativeWindow, origin: Point<Pixels>, anchor: Anchor, cx: &mut Context<Self>) {
        self.origin = origin;
        if anchor != self.anchor {
            self.anchor = anchor;
            cx.notify();
        }
        let screen_h = placement::main_screen_height();
        let (x, y) = (origin.x.as_f32() as f64, (screen_h - origin.y.as_f32() - HEIGHT) as f64);
        crate::app::appkit_later(cx, move || crate::shell::set_frame_origin(ns, x, y));
    }

    /// Poll the mouse at 30 Hz: hover state, click-through, and dragging.
    fn start_mouse_tracking(window: &mut Window, cx: &mut Context<Self>) {
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let mut last_full = Instant::now();
            let mut busy = true;
            loop {
                // 30 Hz while hovered, dragging, or dictating; 12 Hz at rest.
                cx.background_executor().timer(Duration::from_millis(if busy { 33 } else { 80 })).await;
                let check = last_full.elapsed() > Duration::from_secs(1);
                if check {
                    last_full = Instant::now();
                }
                let result = handle
                    .update(cx, |_, window, cx| this.update(cx, |view, cx| view.track(window, check, cx)).ok())
                    .ok()
                    .flatten();
                match result {
                    Some(b) => busy = b,
                    None => break,
                }
            }
        })
        .detach();
    }

    /// Returns true when the overlay needs fast tracking.
    fn track(&mut self, window: &mut Window, check_fullscreen: bool, cx: &mut Context<Self>) -> bool {
        let Some(ns) = crate::shell::native(window) else { return false };
        if !self.configured {
            crate::shell::configure_overlay(ns);
            crate::shell::make_never_key(ns);
            self.configured = true;
            if self.notch.is_some() && crate::shell::can_place_windows() {
                // macOS keeps a window out of the menu bar. Turn that off,
                // then put the notch at the top edge again.
                crate::shell::allow_over_menu_bar(ns);
                self.move_to(ns, self.origin, self.anchor, cx);
            }
        }
        if !crate::shell::can_place_windows() {
            return self.track_in_place(window, ns, check_fullscreen, cx);
        }
        let mouse = crate::shell::mouse_location();
        let frame = global_bounds(window, ns);
        // These bounds are top-left based; the mouse is Cocoa (bottom-left).
        let screen_h = placement::main_screen_height();
        let local = point(px(mouse.x as f32) - frame.origin.x, px(screen_h - mouse.y as f32) - frame.origin.y);
        let card = self.card.get();
        let inside = card.size.width > px(0.) && card.contains(&local);

        if let Some(mut drag) = self.drag {
            let now = point(px(mouse.x as f32), px(screen_h - mouse.y as f32));
            let delta = now - drag.mouse;
            drag.moved = drag.moved || delta.x.abs() > px(3.) || delta.y.abs() > px(3.);
            self.drag = Some(drag);
            let released = !crate::shell::mouse_button_down();
            if drag.moved {
                let (origin, anchor) = self.placement.drag(drag.card + delta, drag.card_height, mouse);
                // At the release, snap and save the place that the drag gave.
                let (origin, anchor) = if released { self.placement.snap_and_save(origin, anchor, drag.card_height, cx) } else { (origin, anchor) };
                self.move_to(ns, origin, anchor, cx);
            } else if released {
                self.click(cx);
            }
            if released {
                self.drag = None;
            }
            return true;
        }
        self.follow_display(window, ns, mouse, cx);

        if inside != self.hovered {
            self.hovered = inside;
            cx.notify();
        }
        crate::shell::set_click_region(window, ns, card, inside);

        if check_fullscreen {
            let m = self.model.read(cx);
            let idle = matches!(m.state(), State::Idle) && m.blocked_notice.is_none();
            let hide = idle && self.hides_when_idle(&m.config.overlay);
            if hide != self.hidden {
                self.hidden = hide;
                if hide {
                    crate::app::appkit_later(cx, move || crate::shell::order_out(ns));
                } else {
                    crate::app::appkit_later(cx, move || crate::shell::order_front_without_activating(ns));
                }
            }
        } else if self.hidden {
            let m = self.model.read(cx);
            if !matches!(m.state(), State::Idle) || m.blocked_notice.is_some() {
                self.hidden = false;
                crate::app::appkit_later(cx, move || crate::shell::order_front_without_activating(ns));
            }
        }
        self.hovered || !matches!(self.model.read(cx).state(), State::Idle)
    }

    /// [`Self::track`] for a system where the overlay cannot move and the
    /// mouse is only known over our own window (Wayland). The pill stays where
    /// the compositor put it, and "hidden" means drawn empty.
    fn track_in_place(&mut self, window: &mut Window, ns: crate::shell::NativeWindow, check_fullscreen: bool, cx: &mut Context<Self>) -> bool {
        let card = self.card.get();
        let inside = window.is_window_hovered() && card.size.width > px(0.) && card.contains(&window.mouse_position());
        if inside != self.hovered {
            self.hovered = inside;
            cx.notify();
        }
        if check_fullscreen || self.hidden {
            let m = self.model.read(cx);
            let idle = matches!(m.state(), State::Idle) && m.blocked_notice.is_none();
            let hide = idle && self.hides_when_idle(&m.config.overlay);
            if hide != self.hidden {
                self.hidden = hide;
                cx.notify();
            }
        }
        let region = if self.hidden { Bounds::default() } else { card };
        crate::shell::set_click_region(window, ns, region, inside);
        self.hovered || !matches!(self.model.read(cx).state(), State::Idle)
    }

    fn begin_press(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Only the pill takes a click or a drag. A notice has its own buttons,
        // and a press on "Copy text" must not also count as a click on the
        // pill that shows after the notice closes.
        if !is_pill(self.model.read(cx).state()) {
            return;
        }
        if !crate::shell::can_place_windows() || self.notch.is_some() {
            // The pill cannot be dragged here, and the notch never moves, so a press is a click.
            self.click(cx);
            return;
        }
        let Some(ns) = crate::shell::native(window) else { return };
        let mouse = crate::shell::mouse_location();
        let screen_h = placement::main_screen_height();
        let origin = global_bounds(window, ns).origin;
        let card_height = self.card.get().size.height;
        self.drag = Some(Drag {
            mouse: point(px(mouse.x as f32), px(screen_h - mouse.y as f32)),
            card: point(origin.x, Placement::card_top(origin, self.anchor, card_height)),
            card_height,
            moved: false,
        });
    }

    /// A click on the card that was not a drag.
    fn click(&mut self, cx: &mut Context<Self>) {
        if is_pill(self.model.read(cx).state()) {
            self.model.update(cx, |m, cx| m.toggle_dictation(cx));
        }
    }
}

/// True in the states where the overlay is the pill: a click starts or stops
/// a dictation, and a drag moves it.
fn is_pill(state: &State) -> bool {
    matches!(state, State::Idle | State::Recording { .. })
}

/// True in the states that draw motion: the waveform and pulse, the ink line
/// of the processing stage, and the countdown of the undo. The other notices
/// are still, so the overlay draws them once and not on every frame.
fn animates(state: &State) -> bool {
    match state {
        State::Recording { .. } | State::Processing { .. } => true,
        State::Showing { notice, .. } => matches!(notice, Notice::Cancelled),
        State::Idle => false,
    }
}

/// The style settings that need a new overlay window when they change.
fn style_key(o: &config::Overlay) -> (OverlayStyle, bool) {
    (o.style, o.notch_on_other_displays)
}

/// The name of the processing stage. The final pass waits for a model that is not in memory yet.
fn stage_label(stage: &Stage, model_status: &ModelStatus) -> String {
    match (stage, model_status) {
        (_, ModelStatus::Optimizing | ModelStatus::Downloaded) => sayso_i18n::t!("overlay.stage.loading_model"),
        (Stage::Enhancing, _) => sayso_i18n::t!("overlay.stage.applying_style"),
        (Stage::Inserting, _) => sayso_i18n::t!("overlay.stage.inserting"),
        _ => sayso_i18n::t!("overlay.stage.transcribing"),
    }
}

/// How far the ink line of the processing stage has drawn.
fn processing_progress(since: Duration, reduce: bool) -> f32 {
    if reduce { 0.6 } else { ((since.as_secs_f32() * 0.55).fract() * 0.9 + 0.08).min(0.98) }
}

/// The time label "0:07".
fn clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

impl Render for OverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.hidden && !crate::shell::can_hide_windows() {
            self.card.set(Bounds::default());
            return div().into_any_element();
        }
        let m = self.model.read(cx);
        let state = m.state().clone();
        let key = format!("{:?}", std::mem::discriminant(&state)) + &format!("{state:?}").chars().take(40).collect::<String>();
        if key != self.last_key {
            self.last_key = key;
            self.phase_start = Instant::now();
        }
        if animates(&state) {
            window.request_animation_frame();
        }
        let c = cx.paper().colors;
        let reduce = cx.paper().reduce_motion;
        let t = if reduce { 0.0 } else { self.start.elapsed().as_secs_f32() };

        let blocked = m.blocked_notice.clone();
        let incognito = m.incognito;
        let recording_style = m.recording_style().map(|s| s.display_name());
        let preview = m.preview.clone();
        let show_preview = m.config.overlay.show_preview;
        let toggle_caps = m.config.hotkeys.toggle.map(|h| h.keycaps()).unwrap_or_default();
        let model_status = m.status_of(&m.config.dictation.model);
        let levels = m.live.smoothed(28);
        let now_ms = m.now_ms();
        // Settings › Overlay › Size. Shapes scale by `k`; text by the gentler `tk`.
        let size = m.config.overlay.size;
        let (k, tk) = (size.scale(), size.text_scale());
        let z = move |v: f32| px(v * k);

        let anchor = self.anchor;
        let card = self.card.clone();
        let capture = move || {
            let card = card.clone();
            canvas(move |b, _, _| card.set(b), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
        };

        // What a screen reader announces: each change of the dictation state.
        // The overlay never takes focus, so this is a live node.
        let announce: Option<(String, bool)> = match (&blocked, &state) {
            (Some(n), State::Idle) => Some((n.detail.as_ref().map_or(n.title.clone(), |d| format!("{}. {d}", n.title)), true)),
            (_, State::Idle) => None,
            (_, State::Recording { .. }) => Some((sayso_i18n::t!("overlay.a11y.recording"), false)),
            (_, State::Processing { stage, .. }) => Some((stage_label(stage, &model_status), false)),
            (_, State::Showing { notice, .. }) => match notice {
                Notice::Inserted { .. } => Some((sayso_i18n::t!("overlay.inserted"), false)),
                Notice::InsertedRaw { reason } => Some((format!("{}. {reason}", sayso_i18n::t!("overlay.inserted_raw")), false)),
                Notice::Cancelled => Some((sayso_i18n::t!("overlay.cancelled"), false)),
                Notice::InsertFailed { reason, .. } => Some((reason.clone(), true)),
                Notice::Error { message } => Some((message.clone(), true)),
                Notice::NoSpeech => Some((sayso_i18n::t!("overlay.error.no_speech"), false)),
                Notice::Discarded => None,
            },
        };
        let pill = is_pill(&state);
        let recording = matches!(state, State::Recording { .. });

        let body: AnyElement = if let Some(n) = self.notch {
            let scene = notch::Scene {
                state: &state,
                blocked: blocked.as_ref(),
                hovered: self.hovered,
                levels,
                t,
                now_ms,
                incognito,
                preview: (show_preview && !(preview.0.is_empty() && preview.1.is_empty())).then_some((preview.0.as_str(), preview.1.as_str())),
                stage: match &state {
                    State::Processing { stage, .. } => stage_label(stage, &model_status),
                    _ => String::new(),
                },
                progress: processing_progress(self.phase_start.elapsed(), reduce),
                model: self.model.clone(),
                wings: self.wings.clone(),
            };
            match notch::body(n, scene, &c, &cx.paper().colors_for(Appearance::Dark)) {
                Some(card) => card.child(capture()).into_any_element(),
                None => {
                    // Nothing shows, so nothing takes the mouse.
                    self.card.set(Bounds::default());
                    div().into_any_element()
                }
            }
        } else if let Some(notice) = blocked.clone().filter(|_| matches!(state, State::Idle)) {
            let ic = match notice.icon {
                NoticeIcon::Info => Icon::Info,
                NoticeIcon::Microphone => Icon::Mic,
                NoticeIcon::Style => Icon::Styles,
                NoticeIcon::Incognito => Icon::Wordmark,
            };
            notice_pill(ic, &notice.title, notice.detail.as_deref(), size, &c).child(capture()).into_any_element()
        } else {
            match &state {
                State::Idle => idle_pill(self.hovered, &toggle_caps, size, &c).child(capture()).into_any_element(),
                State::Recording { started_ms, .. } => {
                    let elapsed = now_ms.saturating_sub(*started_ms);
                    let pulse = if reduce { 0.0 } else { (t * 2.4).sin() * 0.5 + 0.5 };
                    let pill = div()
                        .relative()
                        .flex()
                        .items_center()
                        .gap(z(14.))
                        .pl(z(10.))
                        .pr(z(18.))
                        .py(z(10.))
                        .rounded_full()
                        .floating(&c)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(z(36.))
                                .rounded_full()
                                .debossed(&c)
                                // A click anywhere on the pill stops, so hover shows a stop button.
                                .child(if self.hovered {
                                    div().size(z(12.)).rounded(z(3.)).bg(c.accent)
                                } else {
                                    div()
                                        .relative()
                                        .size(z(12.))
                                        .rounded_full()
                                        .bg(c.accent)
                                        .child(halo(12. * k, (3. + 2. * pulse) * k, c.accent.opacity(0.16 + 0.1 * pulse)))
                                }),
                        )
                        // The window has room for one tag next to the full waveform.
                        .child(ink_waveform(levels, t, c.ink).w(z(if incognito && recording_style.is_some() { 150. } else { 232. })).h(z(36.)))
                        .child(text::mono(clock(elapsed), 13. * tk, c.graphite).w(px(36. * tk)).text_right())
                        .when_some(recording_style, |d, s| d.child(StyleTag::new(s, Some(c.ink))))
                        .when(incognito, |d| {
                            d.child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .h(z(22.))
                                    .px(z(9.))
                                    .rounded_full()
                                    .debossed(&c)
                                    .child(text::ui(sayso_i18n::t!("overlay.incognito"), 12. * tk, FontWeight::MEDIUM, c.graphite)),
                            )
                        })
                        .child(capture());
                    // The live preview is on the side of the pill that has room on the display.
                    let slip = (show_preview && !(preview.0.is_empty() && preview.1.is_empty())).then(|| preview_slip(&preview.0, &preview.1, size, &c));
                    let col = div().flex().flex_col().items_center().gap(z(12.));
                    match anchor {
                        Anchor::Bottom => col.children(slip).child(pill),
                        Anchor::Top => col.child(pill).children(slip),
                    }
                    .into_any_element()
                }
                State::Processing { stage, .. } => {
                    let label = stage_label(stage, &model_status);
                    let p = processing_progress(self.phase_start.elapsed(), reduce);
                    div()
                        .relative()
                        .flex()
                        .items_center()
                        .gap(z(12.))
                        .px(z(18.))
                        .py(z(10.))
                        .rounded_full()
                        .floating(&c)
                        .child(div().w(z(150.)).h(z(36.)).child(ink_line(p, c.ink, c.pencil).size_full()))
                        .child(div().font_family(SERIF).locale_fonts(Face::Serif).italic().text_size(px(16. * tk)).text_color(c.graphite).child(label))
                        .child(capture())
                        .into_any_element()
                }
                State::Showing { notice, until_ms, .. } => match notice {
                    Notice::Inserted { style_name } => div()
                        .relative()
                        .flex()
                        .items_center()
                        .gap(z(10.))
                        .pl(z(8.))
                        .pr(z(if style_name.is_some() { 10. } else { 18. }))
                        .py(z(8.))
                        .rounded_full()
                        .floating(&c)
                        .child(Seal::new(28. * k))
                        .child(text::ui(sayso_i18n::t!("overlay.inserted"), 14. * tk, FontWeight::SEMIBOLD, c.ink))
                        .when_some(style_name.clone(), |d, s| d.child(StyleTag::new(s, Some(c.ink))))
                        .child(capture())
                        .into_any_element(),
                    Notice::InsertedRaw { reason } => {
                        notice_pill(Icon::Check, &sayso_i18n::t!("overlay.inserted_raw"), Some(reason), size, &c).child(capture()).into_any_element()
                    }
                    Notice::Cancelled => {
                        let total = 5000.0;
                        let left = until_ms.map(|u| u.saturating_sub(now_ms) as f32 / total).unwrap_or(0.0);
                        let model = self.model.clone();
                        div()
                            .relative()
                            .flex()
                            .items_center()
                            .gap(z(12.))
                            .pl(z(18.))
                            .pr(z(8.))
                            .py(z(8.))
                            .rounded_full()
                            .floating(&c)
                            .child(text::ui(sayso_i18n::t!("overlay.cancelled"), 14. * tk, FontWeight::MEDIUM, c.graphite))
                            .child(
                                Pressable::new("undo")
                                    .label(sayso_i18n::t!("overlay.a11y.undo_cancel"))
                                    .flex()
                                    .items_center()
                                    .gap(z(8.))
                                    .h(z(30.))
                                    .pl(z(10.))
                                    .pr(z(12.))
                                    .rounded_full()
                                    .bg(c.ink_fill)
                                    .shadows(paper::ink_button(&c))
                                    .cursor_pointer()
                                    .on_click(move |_, _, cx| model.update(cx, |m, cx| m.undo_cancel(cx)))
                                    .child(countdown_ring(left, c.on_ink).size(z(18.)))
                                    .child(text::ui(sayso_i18n::t!("overlay.undo"), 14. * tk, FontWeight::SEMIBOLD, c.on_ink)),
                            )
                            .child(capture())
                            .into_any_element()
                    }
                    Notice::InsertFailed { text, reason } => {
                        let (m1, m2) = (self.model.clone(), self.model.clone());
                        let copy = text.clone();
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .gap(z(10.))
                            .w(z(320.))
                            .pt(z(14.))
                            .px(z(16.))
                            .pb(z(12.))
                            .rounded(z(14.))
                            .floating(&c)
                            .child(text::ui(reason.clone(), 13. * tk, FontWeight::SEMIBOLD, c.danger))
                            .child(text::serif(text.replace('\n', " "), 15. * tk, c.ink).line_clamp(4).text_ellipsis())
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap(z(6.))
                                    .child(
                                        Button::new("dismiss", sayso_i18n::t!("overlay.dismiss"))
                                            .small()
                                            .on_click(move |_, _, cx| m1.update(cx, |m, cx| m.dismiss_notice(cx))),
                                    )
                                    .child(Button::new("copy", sayso_i18n::t!("overlay.copy_text")).small().primary().on_click(move |_, _, cx| {
                                        m2.update(cx, |m, cx| {
                                            m.copy_text(&copy);
                                            m.dismiss_notice(cx);
                                        })
                                    })),
                            )
                            .child(capture())
                            .into_any_element()
                    }
                    Notice::Error { message } => notice_pill(Icon::Info, message, None, size, &c).child(capture()).into_any_element(),
                    Notice::NoSpeech => {
                        notice_pill(Icon::Info, &sayso_i18n::t!("overlay.error.no_speech"), None, size, &c).child(capture()).into_any_element()
                    }
                    Notice::Discarded => div().into_any_element(),
                },
            }
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .map(|d| match anchor {
                Anchor::Bottom => d.justify_end().pb(px(MARGIN)),
                // The notch sits on the top edge of the display.
                Anchor::Top => d.justify_start().pt(px(if self.notch.is_some() { 0. } else { MARGIN })),
            })
            .font_family(UI)
            .locale_fonts(Face::Sans)
            .child(
                div()
                    .id("overlay-card")
                    // The pill is a button: a press starts or stops a
                    // dictation. A notice is a status with its own buttons.
                    .role(if pill { Role::Button } else { Role::Status })
                    .aria_label(if recording {
                        sayso_i18n::t!("overlay.a11y.stop")
                    } else if pill {
                        sayso_i18n::t!("overlay.a11y.start")
                    } else {
                        sayso_i18n::t!("overlay.a11y.name")
                    })
                    .when(pill, |d| {
                        let view = cx.entity().downgrade();
                        d.on_a11y_action(AccessibleAction::Click, move |_, _, cx| {
                            let _ = view.update(cx, |this, cx| this.click(cx));
                        })
                    })
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.begin_press(window, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| {
                        let model = this.model.clone();
                        crate::popover::toggle(&model, cx);
                    }))
                    // The live node is an empty child. A live parent makes
                    // its children live too, and the clock would be read
                    // each second.
                    .child(
                        div()
                            .id("overlay-status")
                            .role(Role::Status)
                            .absolute()
                            .size_0()
                            .when_some(announce, |d, (text, assertive)| d.aria_label(text.clone()).a11y_live(text, assertive)),
                    )
                    .child(body),
            )
            .into_any_element()
    }
}

fn idle_pill(hovered: bool, caps: &[String], size: OverlaySize, c: &sayso_ui::Colors) -> Div {
    let (k, tk) = (size.scale(), size.text_scale());
    if hovered {
        return div()
            .relative()
            .flex()
            .items_center()
            .gap(px(10. * k))
            .pl(px(8. * k))
            .pr(px(14. * k))
            .py(px(7. * k))
            .rounded_full()
            .bg(c.sheet_raised)
            .shadow(paper::raised(c))
            .child(Keycaps::new(caps.iter().cloned()).size(if k < 0.8 { KeySize::Small } else { KeySize::Medium }))
            .child(text::ui(sayso_i18n::t!("overlay.to_dictate"), 14. * tk, FontWeight::MEDIUM, c.graphite));
    }
    div()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .w(px(52. * k))
        .h(px(14. * k))
        .rounded_full()
        .bg(c.sheet_raised)
        .shadow(vec![
            BoxShadow::new(px(0.), px(1. * k), c.highlight(1.0)).inset(),
            BoxShadow::new(px(0.), px(1. * k), c.shadow(0.16)).blur_radius(px(2. * k)),
            BoxShadow::new(px(0.), px(4. * k), c.shadow(0.12)).blur_radius(px(12. * k)),
        ])
        .child(div().w(px(18. * k)).h(px(2. * k)).rounded(px(2. * k)).bg(c.ink.opacity(0.55)))
}

fn notice_pill(ic: Icon, title: &str, detail: Option<&str>, size: OverlaySize, c: &sayso_ui::Colors) -> Div {
    let (k, tk) = (size.scale(), size.text_scale());
    div()
        .relative()
        .flex()
        .items_center()
        .gap(px(10. * k))
        .pl(px(8. * k))
        .pr(px(18. * k))
        .py(px(8. * k))
        .rounded_full()
        .floating(c)
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(28. * k))
                .rounded_full()
                .debossed(c)
                .child(icon(ic, 14. * k, c.ink)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(1. * k))
                .child(text::ui(title.to_string(), 14. * tk, FontWeight::SEMIBOLD, c.ink))
                .when_some(detail.map(str::to_string), |d, s| d.child(text::ui(s, 13. * tk, FontWeight::NORMAL, c.graphite))),
        )
}

/// The live preview: committed words in ink, the tentative tail in pencil
/// italic, and the caret. One styled text run, so it wraps like a paragraph.
fn preview_slip(committed: &str, tentative: &str, size: OverlaySize, c: &sayso_ui::Colors) -> Div {
    let (k, tk) = (size.scale(), size.text_scale());
    // A smaller slip holds fewer characters per line.
    let (text, highlights) = preview_text(committed, tentative, (150. * k) as usize, c);
    div()
        .w(px(420. * k))
        .px(px(18. * k))
        .py(px(14. * k))
        .rounded(px(12. * k))
        .bg(c.sheet)
        .shadow(paper::raised(c))
        .font_family(SERIF).locale_fonts(Face::Serif)
        .text_size(px(17. * tk))
        .line_height(px(24. * tk))
        .text_color(c.ink)
        .child(StyledText::new(text).with_highlights(highlights))
}

/// The text of the live preview with its colors: committed words in ink, the
/// tentative tail in pencil italic, and the caret. It shows only the last
/// `max` characters of the committed words, so the slip stays at three lines
/// or less.
fn preview_text(committed: &str, tentative: &str, max: usize, c: &sayso_ui::Colors) -> (String, Vec<(std::ops::Range<usize>, HighlightStyle)>) {
    let committed = tail(committed, max);
    let mut text = committed.clone();
    let mut highlights = Vec::new();
    if !tentative.is_empty() {
        if needs_space(&text, tentative) {
            text.push(' ');
        }
        let start = text.len();
        text.push_str(tentative);
        highlights.push((
            start..text.len(),
            HighlightStyle { color: Some(c.pencil), font_style: Some(FontStyle::Italic), ..Default::default() },
        ));
    }
    let trimmed = text.trim_end().len();
    text.truncate(trimmed);
    let caret_start = text.len();
    text.push_str(" \u{2502}");
    highlights.push((caret_start..text.len(), HighlightStyle { color: Some(c.accent.opacity(0.7)), ..Default::default() }));
    (text, highlights)
}

fn tail(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let start = chars.len() - max;
    let cut: String = chars[start..].iter().collect();
    match cut.find(' ') {
        Some(i) => format!("…{}", &cut[i..]),
        None => format!("…{cut}"),
    }
}

/// True when the committed text and the tentative tail need a space between
/// them: not when either side is empty, and not next to a script without
/// spaces between words.
fn needs_space(committed: &str, tentative: &str) -> bool {
    match (committed.chars().next_back(), tentative.chars().next()) {
        (Some(a), Some(b)) => !(is_unspaced(a) || is_unspaced(b)),
        _ => false,
    }
}

/// A character of a script that has no spaces between words (Chinese, Japanese,
/// Thai, Lao, Myanmar, Khmer). Keep the same ranges as `PreviewTail.isUnspaced`
/// in `native/macos/SaysoEngine`.
fn is_unspaced(c: char) -> bool {
    matches!(
        c as u32,
        0x0E00..=0x0EFF | 0x1000..=0x109F | 0x1780..=0x17FF | 0x3000..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF
    )
}

/// The Undo countdown: a ring that empties as the window closes.
fn countdown_ring(left: f32, color: Hsla) -> Canvas<()> {
    canvas(
        |_, _, _| (),
        move |b, _, window, _| {
            let center = b.center();
            let r = b.size.width / 2. - px(1.5);
            // Track.
            let mut track = PathBuilder::stroke(px(1.6));
            track.move_to(point(center.x + r, center.y));
            track.arc_to(point(r, r), px(0.), false, true, point(center.x - r, center.y));
            track.arc_to(point(r, r), px(0.), false, true, point(center.x + r, center.y));
            if let Ok(p) = track.build() {
                window.paint_path(p, color.opacity(0.25));
            }
            let left = left.clamp(0.0, 1.0);
            if left <= 0.001 {
                return;
            }
            let a0 = -std::f32::consts::FRAC_PI_2;
            let a1 = a0 + left * std::f32::consts::TAU;
            let at = |a: f32| point(center.x + r * a.cos(), center.y + r * a.sin());
            let mut arc = PathBuilder::stroke(px(1.6));
            arc.move_to(at(a0));
            arc.arc_to(point(r, r), px(0.), left > 0.5, true, at(a1));
            if let Ok(p) = arc.build() {
                window.paint_path(p, color);
            }
        },
    )
}

use gpui_kit::prelude::FluentBuilder as _;

#[cfg(test)]
mod tests {
    use super::{animates, needs_space};
    use sayso_core::dictation::{Notice, Stage, State, Trigger};

    #[test]
    fn space_between_the_committed_text_and_the_tail() {
        assert!(needs_space("hello", "world"));
        assert!(!needs_space("", "world"));
        assert!(!needs_space("今日は", "いい天気"));
        assert!(!needs_space("I said", "こんにちは"));
        assert!(!needs_space("สวัสดี", "ครับ"));
    }

    #[test]
    fn only_moving_states_ask_for_frames() {
        let showing = |notice| State::Showing { session: 1, notice, until_ms: Some(3000) };
        assert!(animates(&State::Recording { session: 1, trigger: Trigger::Toggle, started_ms: 0 }));
        assert!(animates(&State::Processing { session: 1, stage: Stage::Transcribing, recorded_ms: 0 }));
        assert!(animates(&showing(Notice::Cancelled)));
        assert!(!animates(&State::Idle));
        assert!(!animates(&showing(Notice::Inserted { style_name: None })));
        assert!(!animates(&showing(Notice::InsertedRaw { reason: "x".into() })));
        assert!(!animates(&showing(Notice::InsertFailed { text: "x".into(), reason: "y".into() })));
        assert!(!animates(&showing(Notice::Error { message: "x".into() })));
        assert!(!animates(&showing(Notice::NoSpeech)));
        assert!(!animates(&showing(Notice::Discarded)));
    }
}
