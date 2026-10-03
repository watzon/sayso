//! Onboarding: seven steps in one window (plan §3, "Onboarding").
//!
//! `OnboardingView` owns the step. Every move is saved with
//! `set_onboarding_step`, so Sayso resumes at the same step after a quit.

mod ai;
mod done;
mod hotkeys;
mod model_step;
mod permissions;
mod practice;
mod welcome;

use crate::hub::settings::recorder::{self, Recorder};
use crate::live::LiveAudio;
use crate::model::AppModel;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::stt::ModelStatus;
use sayso_platform::{CaptureHandle, Permission, PermissionState};
use sayso_ui::ActivePaper;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::texture::{Grain, grain};
use sayso_ui::{Colors, text};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

pub const STEPS: u8 = 7;

pub struct OnboardingView {
    model: Entity<AppModel>,
    step: u8,
    recorder: Recorder,
    /// Mic test levels (not the dictation levels).
    live: Arc<LiveAudio>,
    meter: Option<Box<dyn CaptureHandle>>,
    meter_error: Option<String>,
    started: Instant,
    /// Download samples for the time estimate: (model, [(time, bytes)]).
    download: Option<(sayso_core::models::ModelId, VecDeque<(Instant, u64)>)>,
    ai: ai::AiForm,
    practice: practice::Practice,
    show_why: bool,
    _observe: Subscription,
}

fn rec(v: &mut OnboardingView) -> &mut Recorder {
    &mut v.recorder
}

/// What the footer shows for a step.
struct Footer {
    label: String,
    enabled: bool,
    /// A ghost action left of the primary button, for example "Skip for now".
    skip: Option<&'static str>,
}

impl OnboardingView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let step = model.read(cx).onboarding_step().min(STEPS - 1);
        let ai = ai::AiForm::new(&model, window, cx);
        let practice = practice::Practice::new(window, cx);
        let mut this = Self {
            model,
            step,
            recorder: Recorder::new(cx),
            live: Arc::new(LiveAudio::default()),
            meter: None,
            meter_error: None,
            started: Instant::now(),
            download: None,
            ai,
            practice,
            show_why: false,
            _observe: observe,
        };
        this.entered(step, window, cx);
        this
    }

    fn go_to(&mut self, step: u8, window: &mut Window, cx: &mut Context<Self>) {
        let step = step.min(STEPS - 1);
        self.recorder.cancel();
        self.step = step;
        self.model.update(cx, |m, cx| m.set_onboarding_step(step, cx));
        self.entered(step, window, cx);
        cx.notify();
    }

    /// Work to do when a step appears.
    fn entered(&mut self, step: u8, window: &mut Window, cx: &mut Context<Self>) {
        if step != 3 {
            self.stop_meter();
        }
        if step == 5 {
            self.practice.enter(window, cx);
        }
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.step > 0 {
            self.go_to(self.step - 1, window, cx);
        }
    }

    fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.step {
            1 => {
                let m = self.model.read(cx);
                if !m.status_of(&m.config.dictation.model).is_on_disk() {
                    // Start (or retry) the download and stay on this step until it is done.
                    self.ensure_download(cx);
                    cx.notify();
                    return;
                }
            }
            2 => {
                if !self.save_ai(cx) {
                    cx.notify();
                    return;
                }
            }
            6 => {
                self.stop_meter();
                crate::app::finish_onboarding(&self.model, window, cx);
                return;
            }
            _ => {}
        }
        self.go_to(self.step + 1, window, cx);
    }

    fn footer_for(&self, cx: &App) -> Footer {
        let m = self.model.read(cx);
        match self.step {
            0 => Footer { label: "Get started".into(), enabled: true, skip: None },
            // A downloaded model is required: dictation cannot work without one.
            1 => match m.status_of(&m.config.dictation.model) {
                s if s.is_on_disk() => Footer { label: "Continue".into(), enabled: true, skip: None },
                ModelStatus::Downloading { fraction, .. } => {
                    Footer { label: format!("Downloading {:.0}%", fraction * 100.0), enabled: false, skip: None }
                }
                _ => Footer { label: "Download".into(), enabled: true, skip: None },
            },
            3 => {
                let both = m.permission(Permission::Microphone) == PermissionState::Granted
                    && m.permission(Permission::Accessibility) == PermissionState::Granted;
                Footer { label: "Continue".into(), enabled: both, skip: (!both).then_some("Skip for now") }
            }
            5 => Footer { label: "Finish".into(), enabled: true, skip: None },
            6 => Footer { label: "Open Sayso".into(), enabled: true, skip: None },
            _ => Footer { label: "Continue".into(), enabled: true, skip: None },
        }
    }

    // -----------------------------------------------------------------------
    // Microphone test
    // -----------------------------------------------------------------------

    fn stop_meter(&mut self) {
        if let Some(h) = self.meter.take() {
            h.stop();
        }
        self.live.clear();
    }

    /// Run the mic test while the permissions step shows and the mic is allowed.
    fn sync_meter(&mut self, cx: &mut Context<Self>) {
        let m = self.model.read(cx);
        let want = self.step == 3 && m.permission(Permission::Microphone) == PermissionState::Granted;
        if !want {
            self.stop_meter();
            return;
        }
        if self.meter.is_some() || self.meter_error.is_some() {
            return;
        }
        let device = m.config.audio.input_device.clone();
        match m.start_level_meter(device.as_deref(), self.live.clone()) {
            Ok(h) => self.meter = Some(h),
            Err(e) => self.meter_error = Some(e),
        }
    }

    // -----------------------------------------------------------------------
    // Pieces
    // -----------------------------------------------------------------------

    fn top_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let mut trail = div().flex().flex_1().items_center().justify_center();
        for i in 0..STEPS {
            if i > 0 {
                trail = trail.child(div().w(px(36.)).h(px(1.5)).bg(if i <= self.step { c.ink_fill } else { c.rule }));
            }
            let dot = trail_dot(i.cmp(&self.step), &c);
            let dot = if i < self.step {
                div()
                    .id(("trail", i as usize))
                    .cursor_pointer()
                    .child(dot)
                    .on_click(cx.listener(move |this, _, window, cx| this.go_to(i, window, cx)))
                    .into_any_element()
            } else {
                dot.into_any_element()
            };
            trail = trail.child(dot);
        }
        let step = text::ui(format!("Step {} of {STEPS}", self.step + 1), 12., FontWeight::NORMAL, c.graphite).w(px(120.)).flex_none();
        let header = div().id("onboarding-title").flex().flex_none().items_center().h(px(52.)).px(px(18.));
        if crate::chrome::is_custom(window) {
            // The custom title bar (Linux): the header moves the window, and
            // the window buttons take the place of the step count.
            return crate::chrome::drag_area(header)
                .child(step)
                .child(trail)
                .child(div().w(px(120.)).flex_none().flex().justify_end().child(crate::chrome::window_controls(window, cx, |window, cx| {
                    cx.global_mut::<crate::app::Windows>().onboarding = None;
                    window.remove_window();
                })))
                .into_any_element();
        }
        header
            // Room for the native traffic lights.
            .child(div().w(px(120.)).flex_none())
            .child(trail)
            .child(step.text_right())
            .into_any_element()
    }

    fn footer(&self, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let f = self.footer_for(cx);
        let last = self.step == STEPS - 1;
        let mut buttons = div().flex().items_center().gap(px(8.));
        if !last {
            buttons = buttons.child(
                div().when(self.step == 0, |d| d.opacity(0.)).child(
                    Button::new("back", "Back")
                        .ghost()
                        .large()
                        .disabled(self.step == 0)
                        .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
                ),
            );
        }
        if let Some(skip) = f.skip {
            buttons = buttons.child(
                Button::new("skip", skip)
                    .ghost()
                    .large()
                    .on_click(cx.listener(|this, _, window, cx| this.go_to(this.step + 1, window, cx))),
            );
        }
        buttons = buttons.child(
            Button::new("primary", f.label)
                .primary()
                .large()
                .trailing_icon(Icon::ArrowRight)
                .disabled(!f.enabled)
                .on_click(cx.listener(|this, _, window, cx| this.next(window, cx))),
        );
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(px(76.))
            .px(px(28.))
            .border_t_1()
            .border_color(c.rule)
            .child(if last {
                div()
            } else {
                text::ui("You can quit at any time. Sayso continues where you stopped.", 13., FontWeight::NORMAL, c.graphite)
            })
            .child(buttons)
    }
}

fn trail_dot(order: std::cmp::Ordering, c: &Colors) -> Div {
    use std::cmp::Ordering::*;
    match order {
        Less => div().size(px(9.)).rounded_full().bg(c.ink_fill),
        Equal => crate::hub::settings::kit::halo_dot(11., c.ink_fill, 3.5, 0.14),
        Greater => div()
            .size(px(9.))
            .rounded_full()
            .bg(c.deboss)
            .shadow(vec![BoxShadow::new(px(0.), px(1.), c.shadow(0.3)).blur_radius(px(1.5)).inset()]),
    }
}

/// The page title and lead text used by steps 2 to 6.
fn heading(title: &str, lead: &str, c: &Colors) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_none()
        .gap(px(8.))
        .child(
            div()
                .relative()
                .child(
                    text::title(title.to_string(), 36., c)
                        .line_height(px(44.))
                        .text_color(c.highlight(0.85))
                        .absolute()
                        .top(px(1.))
                        .left_0(),
                )
                .child(text::title(title.to_string(), 36., c).line_height(px(44.))),
        )
        .child(text::body(lead.to_string(), c))
}

/// A raised card, as in the onboarding design.
fn card(c: &Colors) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(px(12.))
        .p(px(18.))
        .rounded(px(14.))
        .bg(c.sheet_raised)
        .shadow(sayso_ui::paper::raised(c))
}

/// The same card with an ink ring (the active choice).
fn ring(c: &Colors) -> Vec<BoxShadow> {
    let mut s = vec![BoxShadow::new(px(0.), px(0.), c.ink).spread_radius(px(2.))];
    s.extend(sayso_ui::paper::raised(c));
    s
}

/// A check in an ink disc and a label: "Allowed", "Works".
fn check_line(label: impl Into<SharedString>, c: &Colors) -> Div {
    check_line_w(label, FontWeight::NORMAL, c)
}

fn check_line_w(label: impl Into<SharedString>, weight: FontWeight, c: &Colors) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(16.))
                .rounded_full()
                .bg(c.ink_fill)
                .child(icon(Icon::Check, 10., c.on_ink)),
        )
        .child(text::ui(label, 13., weight, c.ink))
}

/// "1.3 GB", "626 MB".
fn size_text(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1e9)
    } else {
        format!("{} MB", (bytes as f64 / 1e6).round() as u64)
    }
}

impl Drop for OnboardingView {
    fn drop(&mut self) {
        self.stop_meter();
    }
}

impl Render for OnboardingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_meter(cx);
        let c = cx.paper().colors;
        let content: AnyElement = match self.step {
            0 => self.welcome(window, cx),
            1 => self.model_step(cx),
            2 => self.ai_step(cx),
            3 => self.permissions_step(window, cx),
            4 => self.hotkeys_step(cx),
            5 => self.practice_step(cx),
            _ => self.done_step(cx),
        };
        let top = self.top_bar(window, cx);
        let footer = self.footer(cx);
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .p(px(10.))
            .bg(c.ground)
            .font_family(sayso_ui::fonts::UI)
            .text_color(c.ink)
            .track_focus(&self.recorder.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                let model = this.model.clone();
                if recorder::on_key_down(rec, this, &model, e, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(grain(Grain::Board, px(0.), cx))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .rounded(px(12.))
                    .bg(c.sheet)
                    .shadow(sayso_ui::paper::sheet(&c))
                    .child(grain(Grain::Sheet, px(12.), cx))
                    .child(top)
                    .child(
                        div()
                            .id(("step", self.step as usize))
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .child(content)
                            .overflow_y_scrollbar(),
                    )
                    .child(footer),
            )
    }
}
