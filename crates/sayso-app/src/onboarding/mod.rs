//! Onboarding: seven steps in one window (plan §3, "Onboarding").
//!
//! `OnboardingView` owns the step. Every move is saved with
//! `set_onboarding_step`, so Sayso resumes at the same step after a quit.

mod ai;
mod all_models;
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
use sayso_i18n::t;
use sayso_core::stt::ModelStatus;
use sayso_platform::{CaptureHandle, Permission, PermissionState};
use sayso_ui::fonts::{Face, LocaleFonts as _};
use sayso_ui::ActivePaper;
use sayso_ui::a11y::{A11yExt as _, LiveStatus, Pressable, Role};
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
    all_models: all_models::AllModels,
    practice: practice::Practice,
    show_why: bool,
    /// The open language list of the welcome step.
    language_menu: Option<welcome::LanguageMenu>,
    /// The locale of the placeholders of the inputs, which keep their text.
    locale: String,
    /// The window has no room for two columns (set at each render).
    narrow: bool,
    /// The device list stays fresh on the step that shows the microphone.
    devices: crate::devices::ViewWatch,
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
    skip: Option<String>,
}

impl OnboardingView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let step = model.read(cx).onboarding_step().min(STEPS - 1);
        let ai = ai::AiForm::new(&model, window, cx);
        let practice = practice::Practice::new(window, cx);
        let all_models = all_models::AllModels::new(window, cx);
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
            all_models,
            practice,
            show_why: false,
            language_menu: None,
            locale: sayso_i18n::locale(),
            narrow: false,
            devices: Default::default(),
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
        self.devices.set(step == 3, &self.model, cx);
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
            0 => Footer { label: t!("onboarding.footer.get_started"), enabled: true, skip: None },
            // A downloaded model is required: dictation cannot work without one.
            1 => match m.status_of(&m.config.dictation.model) {
                s if s.is_on_disk() => Footer { label: t!("onboarding.footer.continue"), enabled: true, skip: None },
                ModelStatus::Downloading { fraction, .. } => {
                    Footer { label: t!("onboarding.footer.downloading", percent = format!("{:.0}", fraction * 100.0)), enabled: false, skip: None }
                }
                _ => Footer { label: t!("onboarding.footer.download"), enabled: true, skip: None },
            },
            // Continue saves the selected provider (a cleanup model starts its download). Skip leaves AI off.
            2 => Footer {
                label: if self.ai_downloads(cx) { t!("onboarding.ai.download_and_continue") } else { t!("onboarding.footer.continue") },
                enabled: self.ai_choice(cx).is_some(),
                skip: Some(t!("onboarding.footer.skip")),
            },
            3 => {
                let both = m.permission(Permission::Microphone) == PermissionState::Granted
                    && m.permission(Permission::Accessibility) == PermissionState::Granted;
                Footer { label: t!("onboarding.footer.continue"), enabled: both, skip: (!both).then(|| t!("onboarding.footer.skip")) }
            }
            5 => Footer { label: t!("onboarding.footer.finish"), enabled: true, skip: None },
            6 => Footer { label: t!("onboarding.footer.open_sayso"), enabled: true, skip: None },
            _ => Footer { label: t!("onboarding.footer.continue"), enabled: true, skip: None },
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

    /// The inputs keep their placeholder, so it changes with the language.
    fn refresh_placeholders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.locale == sayso_i18n::locale() {
            return;
        }
        self.locale = sayso_i18n::locale();
        self.ai.key.update(cx, |s, cx| s.set_placeholder(t!("onboarding.ai.key_placeholder"), window, cx));
        self.all_models.filter.update(cx, |s, cx| s.set_placeholder(t!("onboarding.all_models.filter_placeholder"), window, cx));
        self.practice.field.update(cx, |s, cx| s.set_placeholder(t!("onboarding.practice.placeholder"), window, cx));
    }

    // -----------------------------------------------------------------------
    // Pieces
    // -----------------------------------------------------------------------

    /// The side padding of a step: 72 as designed, 32 in a narrow window.
    fn side(&self) -> f32 {
        if self.narrow { 32. } else { 72. }
    }

    /// A row of cards. In a narrow window the cards go in one column.
    fn columns(&self) -> Div {
        let row = div().flex().items_start().gap(px(14.));
        if self.narrow { row.flex_col().items_stretch() } else { row }
    }

    fn top_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let mut trail = div().flex().flex_1().items_center().justify_center();
        for i in 0..STEPS {
            if i > 0 {
                trail = trail.child(div().w(px(36.)).h(px(1.5)).bg(if i <= self.step { c.ink_fill } else { c.rule }));
            }
            let dot = trail_dot(i.cmp(&self.step), &c);
            // A step that is done is a button that goes back to it. The other
            // dots only show the place.
            let dot = if i < self.step {
                Pressable::new(("trail", i as usize))
                    .label(t!("onboarding.go_to_step", step = i + 1))
                    .cursor_pointer()
                    .child(dot)
                    .on_click(cx.listener(move |this, _, window, cx| this.go_to(i, window, cx)))
                    .into_any_element()
            } else {
                dot.into_any_element()
            };
            trail = trail.child(dot);
        }
        // A live node: a screen reader says the new step when the step changes.
        let step_text = t!("onboarding.step", step = self.step + 1, total = STEPS);
        let step = LiveStatus::new("onboarding-step", step_text.clone())
            .font_family(sayso_ui::fonts::UI)
            .text_size(px(12.))
            .font_weight(FontWeight::NORMAL)
            .text_color(c.graphite)
            .w(px(120.))
            .flex_none()
            .child(step_text);
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
                    Button::new("back", t!("onboarding.footer.back"))
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
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.step == 2 {
                            this.skip_ai(cx);
                        }
                        this.go_to(this.step + 1, window, cx)
                    })),
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
            // A narrow window has room for the buttons only.
            .child(if last || self.narrow {
                div()
            } else {
                text::ui(t!("onboarding.footer.hint"), 13., FontWeight::NORMAL, c.graphite)
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
        .child(text::letterpress(title.to_string(), 36., 44., 1, c))
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

/// Text inside a control. It has no node: the control carries the name.
fn plain(text: impl Into<SharedString>, size: f32, weight: FontWeight, color: Hsla) -> Div {
    div().font_family(sayso_ui::fonts::UI).text_size(px(size)).font_weight(weight).text_color(color).child(text.into())
}

/// [`plain`] in the mono face.
fn plain_mono(text: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div().font_family(sayso_ui::fonts::MONO).text_size(px(size)).font_weight(FontWeight::NORMAL).text_color(color).child(text.into())
}

/// [`plain`] as a title in the display face.
fn plain_title(text: impl Into<SharedString>, size: f32, c: &Colors) -> Div {
    div()
        .font_family(sayso_ui::fonts::DISPLAY)
        .locale_fonts(Face::Serif)
        .text_size(px(size))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c.ink)
        .child(text.into())
}

/// Text that a screen reader reads when it shows and when it changes. The
/// node holds the text, so the text inside has none.
fn live_ui(id: impl Into<ElementId>, text: impl Into<SharedString>, size: f32, weight: FontWeight, color: Hsla, assertive: bool) -> Stateful<Div> {
    let text = text.into();
    plain(text.clone(), size, weight, color).id(id).role(Role::Status).a11y_live(text, assertive)
}

/// Text for a state that is on the page from the start. A screen reader reads
/// it when the text changes, not when it shows. The node holds the text, so
/// the text inside has none.
fn status_ui(id: impl Into<ElementId>, text: impl Into<SharedString>, size: f32, weight: FontWeight, color: Hsla) -> LiveStatus {
    let text = text.into();
    LiveStatus::new(id, text.clone())
        .font_family(sayso_ui::fonts::UI)
        .text_size(px(size))
        .font_weight(weight)
        .text_color(color)
        .child(text)
}

/// A text link in the accent color.
fn link(id: impl Into<ElementId>, label: impl Into<SharedString>, size: f32, c: &Colors) -> Pressable {
    let label = label.into();
    Pressable::new(id)
        .label(label.clone())
        .a11y_role(Role::Link)
        .flex_none()
        .cursor_pointer()
        .font_family(sayso_ui::fonts::UI)
        .text_size(px(size))
        .line_height(px(18.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(c.accent)
        .hover(|s| s.opacity(0.75))
        .child(label)
}

/// What an arrow key does in a radio group of `count` choices: the index of
/// the choice to select, and whether the focus moves forward. Right and Down
/// go forward, Left and Up go back. With no choice selected, the first key
/// picks the first or the last one.
fn arrow_target(key: &str, selected: Option<usize>, count: usize) -> Option<(usize, bool)> {
    match key {
        "right" | "down" => {
            let next = selected.map_or(0, |s| s + 1);
            (next < count).then_some((next, true))
        }
        "left" | "up" => {
            let next = match selected {
                Some(s) => s.checked_sub(1)?,
                None => count.checked_sub(1)?,
            };
            Some((next, false))
        }
        _ => None,
    }
}

/// "1.3 GB", "626 MB".
fn size_text(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        t!("onboarding.size_gb", number = sayso_i18n::numbers::decimal(bytes as f64 / 1e9, 1))
    } else {
        t!("onboarding.size_mb", number = (bytes as f64 / 1e6).round() as u64)
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
        self.refresh_placeholders(window, cx);
        self.narrow = crate::layout::onboarding_narrow(window);
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
        let all_models = (self.step == 1).then(|| self.all_models_modal(cx)).flatten();
        let view = div()
            .relative()
            .flex()
            .flex_col()
            .p(px(10.))
            .pt(px(crate::caption::top_margin(10.)))
            .bg(c.ground)
            .font_family(sayso_ui::fonts::UI)
            .locale_fonts(Face::Sans)
            .text_color(c.ink)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                let model = this.model.clone();
                if recorder::on_key_down(rec, this, &model, e, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(crate::hub::settings::kit::key_listener("onboarding-keys", &self.recorder.focus))
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
            .children(crate::caption::caption_bar(false, cx))
            .children(all_models);
        crate::layout::floor("onboarding-floor", crate::layout::ONBOARDING_FLOOR, view)
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the glob of `gpui_kit` has its own `test` macro.
    use super::arrow_target;

    #[test]
    fn arrow_keys_move_the_choice_and_stop_at_the_ends() {
        assert_eq!(arrow_target("down", Some(0), 3), Some((1, true)));
        assert_eq!(arrow_target("right", Some(1), 3), Some((2, true)));
        assert_eq!(arrow_target("down", Some(2), 3), None);
        assert_eq!(arrow_target("up", Some(2), 3), Some((1, false)));
        assert_eq!(arrow_target("left", Some(0), 3), None);
        assert_eq!(arrow_target("a", Some(0), 3), None);
    }

    #[test]
    fn with_no_choice_the_first_key_picks_an_end() {
        assert_eq!(arrow_target("down", None, 3), Some((0, true)));
        assert_eq!(arrow_target("up", None, 3), Some((2, false)));
        assert_eq!(arrow_target("up", None, 0), None);
    }
}
