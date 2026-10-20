//! The list of all local models, over the model step. The step offers three
//! models. This list is for the user who wants a different one.

use super::{OnboardingView, arrow_target, plain, plain_mono, plain_title};
use crate::hub::pages::kit::caps;
use crate::hub::pages::models::{LOCAL_GROUPS, group_title, size_label, speed_meter};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_i18n::t;
use sayso_core::languages;
use sayso_core::models::{ModelId, ModelInfo};
use sayso_core::stt::ModelStatus;
use sayso_ui::a11y::{A11yExt as _, A11yText, Pressable, Role};
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::{self, PaperStyled};
use sayso_ui::{ActivePaper, text};

pub(super) struct AllModels {
    open: bool,
    pub(super) filter: Entity<InputState>,
    _filter: Subscription,
}

impl AllModels {
    pub(super) fn new(window: &mut Window, cx: &mut Context<OnboardingView>) -> Self {
        let filter = crate::widgets::input_state(&t!("onboarding.all_models.filter_placeholder"), "", window, cx);
        let sub = cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify());
        Self { open: false, filter, _filter: sub }
    }
}

/// True when the name, the vendor, the description, or a language of the
/// model contains `query`.
fn matches(info: &ModelInfo, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let mut text = format!("{} {} {}", info.name, info.vendor, info.description);
    for code in info.languages {
        text.push(' ');
        text.push_str(&languages::display(code));
    }
    text.to_lowercase().contains(&query)
}

impl OnboardingView {
    /// The models the user can make active here: local, and able to do the final pass.
    pub(super) fn local_models(&self, cx: &App) -> Vec<ModelInfo> {
        self.model.read(cx).catalog().into_iter().filter(|i| !i.is_remote() && i.final_pass).collect()
    }

    pub(super) fn open_all_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.all_models.open = true;
        self.all_models.filter.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn close_all_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.all_models.open = false;
        self.all_models.filter.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    /// One model of the list: a radio button. `index` of `count` is its place
    /// among the models that the filter shows.
    fn all_models_row(&self, info: &ModelInfo, active: &ModelId, index: usize, count: usize, cx: &mut Context<Self>) -> Pressable {
        let c = cx.paper().colors;
        let selected = info.id == *active;
        let recommended = info.id == super::model_step::suggested(&self.model.read(cx).config.dictation.language);
        let state = match self.model.read(cx).status_of(&info.id) {
            ModelStatus::Downloading { .. } => Some(t!("onboarding.all_models.downloading")),
            s if s.is_on_disk() => Some(t!("onboarding.all_models.downloaded")),
            _ => None,
        };
        let id = info.id.clone();
        let details = format!("{} · {} · {}", info.vendor, info.language_label(), info.description.trim_end_matches('.'));
        // What the badges say, for a screen reader.
        let spoken = [recommended.then(|| t!("onboarding.model.recommended")), state.clone(), Some(details.clone()), Some(size_label(info))]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(". ");
        Pressable::new(SharedString::from(format!("all-models-{}", info.id)))
            .a11y_radio(info.name.clone(), selected, index, count)
            .aria_description(spoken)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(14.))
            .h(px(58.))
            .px(px(12.))
            .rounded(px(10.))
            .cursor_pointer()
            .when(selected, |d| d.bg(c.deboss))
            .hover(|s| s.bg(c.deboss))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.choose_model(id.clone(), cx);
                this.close_all_models(window, cx);
            }))
            .child(radio_dot(selected, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(plain_title(info.name.clone(), 16., &c).line_height(px(20.)))
                            .when(recommended, |d| d.child(Badge::new(t!("onboarding.model.recommended"), BadgeTone::Accent)))
                            .when_some(state, |d, s| d.child(Badge::new(s, BadgeTone::Muted))),
                    )
                    .child(plain(details, 12., FontWeight::NORMAL, c.graphite).line_height(px(16.)).truncate()),
            )
            .child(speed_meter(info.speed, &c))
            .child(plain_mono(size_label(info), 12., c.graphite).w(px(64.)).flex_none().text_right())
    }

    /// The list over the step. Esc, the backdrop, or the close button dismiss it.
    pub(super) fn all_models_modal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.all_models.open {
            return None;
        }
        let c = cx.paper().colors;
        let active = self.model.read(cx).config.dictation.model.clone();
        let query = self.all_models.filter.read(cx).value().to_string();
        let models: Vec<ModelInfo> = self.local_models(cx).into_iter().filter(|i| matches(i, &query)).collect();

        let count = models.len();
        let ids: Vec<ModelId> = models.iter().map(|i| i.id.clone()).collect();
        let selected_index = ids.iter().position(|id| *id == active);
        // One choice of a set: the rows are radio buttons, and the arrow keys
        // move the choice.
        let mut list = div()
            .id("all-models-list")
            .role(Role::RadioGroup)
            .aria_label(t!("onboarding.all_models.title"))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px(px(16.))
            .pb(px(16.))
            .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
                let Some((next, forward)) = arrow_target(&ev.keystroke.key, selected_index, count) else { return };
                cx.stop_propagation();
                this.choose_model(ids[next].clone(), cx);
                if forward { window.focus_next(cx) } else { window.focus_prev(cx) }
            }));
        if models.is_empty() {
            list = list.child(div().px(px(12.)).py(px(20.)).child(text::ui(
                t!("onboarding.all_models.no_match"),
                13.,
                FontWeight::NORMAL,
                c.graphite,
            )));
        }
        let mut index = 0;
        for family in LOCAL_GROUPS {
            let title = group_title(family);
            let group: Vec<&ModelInfo> = models.iter().filter(|i| i.engine.family() == family).collect();
            if group.is_empty() {
                continue;
            }
            // The family is a heading. The column captions only help the eye.
            let heading = div()
                .flex_1()
                .font_family(sayso_ui::fonts::UI)
                .text_size(px(11.))
                .line_height(px(16.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(c.graphite)
                .child(A11yText::new(title.to_uppercase()).heading(3).with_id(SharedString::from(format!("all-models-family-{title}"))));
            list = list.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(14.))
                    .h(px(34.))
                    .px(px(12.))
                    .child(heading)
                    .child(caps(t!("onboarding.all_models.speed"), 11., c.graphite).w(px(82.)).flex_none())
                    .child(caps(t!("onboarding.all_models.size"), 11., c.graphite).w(px(64.)).flex_none().text_right()),
            );
            for info in group {
                list = list.child(self.all_models_row(info, &active, index, count, cx));
                index += 1;
            }
        }

        let head = div()
            .flex()
            .flex_none()
            .items_start()
            .justify_between()
            .gap(px(16.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(text::title(t!("onboarding.all_models.title"), 24., &c).line_height(px(30.)))
                    .child(text::ui(
                        t!(crate::shell::os_text!("onboarding.all_models.lead_mac", "onboarding.all_models.lead_windows", "onboarding.all_models.lead_other")),
                        13.,
                        FontWeight::NORMAL,
                        c.graphite,
                    )),
            )
            .child(
                Pressable::new("all-models-close")
                    .label(t!("onboarding.all_models.close"))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(22.))
                    .rounded_full()
                    .cursor_pointer()
                    .hover(|s| s.bg(c.deboss))
                    .child(icon(Icon::Close, 12., c.graphite))
                    .on_click(cx.listener(|this, _, window, cx| this.close_all_models(window, cx))),
            );

        Some(
            div()
                .id("all-models-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(c.shadow(0.28))
                .occlude()
                .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    if ev.keystroke.key == "escape" {
                        cx.stop_propagation();
                        this.close_all_models(window, cx);
                    }
                }))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_all_models(window, cx)))
                .child(
                    div()
                        .id("all-models")
                        .role(Role::Dialog)
                        .aria_label(t!("onboarding.all_models.title"))
                        .flex()
                        .flex_col()
                        .w(px(720.))
                        .h(px(540.))
                        .rounded(px(16.))
                        .bg(c.sheet_raised)
                        .shadow(paper::floating(&c))
                        // A click inside the sheet does not close it.
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_none()
                                .gap(px(16.))
                                .pt(px(24.))
                                .px(px(28.))
                                .pb(px(8.))
                                .child(head)
                                .child(
                                    div()
                                        .flex()
                                        .flex_none()
                                        .items_center()
                                        .gap(px(10.))
                                        .h(px(38.))
                                        .px(px(12.))
                                        .rounded(px(10.))
                                        .debossed(&c)
                                        .text_size(px(14.))
                                        .child(icon(Icon::Search, 15., c.graphite))
                                        .child(
                                            div().flex_1().min_w_0().child(
                                                Input::new(&self.all_models.filter)
                                                    .appearance(false)
                                                    .px_0()
                                                    .py_0()
                                                    .h(px(26.))
                                                    .text_size(px(14.))
                                                    .line_height(px(22.))
                                                    .aria_label(t!("onboarding.all_models.filter_label")),
                                            ),
                                        ),
                                ),
                        )
                        .child(list.overflow_y_scrollbar()),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the glob of `gpui_kit` has its own `test` macro.
    use super::matches;
    use sayso_core::languages;

    #[test]
    fn filter_matches_name_vendor_and_language() {
        let info = sayso_core::models::catalog().iter().find(|i| i.is_multilingual()).expect("a multilingual model");
        let language = languages::display(info.languages.iter().find(|l| **l != languages::AUTO).unwrap());
        assert!(matches(info, ""));
        assert!(matches(info, &format!("  {}  ", info.name.to_uppercase())));
        assert!(matches(info, &info.vendor));
        assert!(matches(info, &language.to_lowercase()));
        assert!(!matches(info, "no model has this text"));
    }
}
