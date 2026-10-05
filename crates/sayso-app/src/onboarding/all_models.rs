//! The list of all local models, over the model step. The step offers three
//! models. This list is for the user who wants a different one.

use super::OnboardingView;
use crate::hub::pages::kit::{self, caps};
use crate::hub::pages::models::{LOCAL_GROUPS, size_label, speed_meter};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::languages;
use sayso_core::models::{ModelId, ModelInfo};
use sayso_core::stt::ModelStatus;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper;
use sayso_ui::{ActivePaper, text};

pub(super) struct AllModels {
    open: bool,
    filter: Entity<InputState>,
    _filter: Subscription,
}

impl AllModels {
    pub(super) fn new(window: &mut Window, cx: &mut Context<OnboardingView>) -> Self {
        let filter = crate::widgets::input_state("Filter by name or language", "", window, cx);
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
    for code in &info.languages {
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

    fn all_models_row(&self, info: &ModelInfo, active: &ModelId, cx: &mut Context<Self>) -> Stateful<Div> {
        let c = cx.paper().colors;
        let selected = info.id == *active;
        let state = match self.model.read(cx).status_of(&info.id) {
            ModelStatus::Downloading { .. } => Some("Downloading"),
            s if s.is_on_disk() => Some("Downloaded"),
            _ => None,
        };
        let id = info.id.clone();
        div()
            .id(SharedString::from(format!("all-models-{}", info.id)))
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
                            .child(text::title(info.name.clone(), 16., &c).line_height(px(20.)))
                            .when(info.recommended, |d| d.child(Badge::new("Recommended", BadgeTone::Accent)))
                            .when_some(state, |d, s| d.child(Badge::new(s, BadgeTone::Muted))),
                    )
                    .child(
                        text::ui(
                            format!("{} · {} · {}", info.vendor, info.language_label(), info.description.trim_end_matches('.')),
                            12.,
                            FontWeight::NORMAL,
                            c.graphite,
                        )
                        .line_height(px(16.))
                        .truncate(),
                    ),
            )
            .child(speed_meter(info.speed, &c))
            .child(text::mono(size_label(info), 12., c.graphite).font_weight(FontWeight::NORMAL).w(px(64.)).flex_none().text_right())
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

        let mut list = div().id("all-models-list").flex().flex_col().flex_1().min_h_0().px(px(16.)).pb(px(16.));
        if models.is_empty() {
            list = list.child(div().px(px(12.)).py(px(20.)).child(text::ui(
                "No model matches the filter. Try a different name or language.",
                13.,
                FontWeight::NORMAL,
                c.graphite,
            )));
        }
        for (title, family) in LOCAL_GROUPS {
            let group: Vec<&ModelInfo> = models.iter().filter(|i| i.engine.family() == family).collect();
            if group.is_empty() {
                continue;
            }
            list = list.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(14.))
                    .h(px(34.))
                    .px(px(12.))
                    .child(caps(title, 11., c.graphite).flex_1())
                    .child(caps("Speed", 11., c.graphite).w(px(82.)).flex_none())
                    .child(caps("Size", 11., c.graphite).w(px(64.)).flex_none().text_right()),
            );
            for info in group {
                list = list.child(self.all_models_row(info, &active, cx));
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
                    .child(text::title("All models", 24., &c).line_height(px(30.)))
                    .child(text::ui(
                        crate::shell::os_text!(
                            "All of these models run on your Mac. Choose one, and Sayso downloads it.",
                            "All of these models run on this PC. Choose one, and Sayso downloads it.",
                            "All of these models run on this computer. Choose one, and Sayso downloads it.",
                        ),
                        13.,
                        FontWeight::NORMAL,
                        c.graphite,
                    )),
            )
            .child(
                kit::icon_button("all-models-close", Icon::Close, 12., cx)
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
                .on_click(cx.listener(|this, _, window, cx| this.close_all_models(window, cx)))
                .child(
                    div()
                        .id("all-models")
                        .flex()
                        .flex_col()
                        .w(px(720.))
                        .h(px(540.))
                        .rounded(px(16.))
                        .bg(c.sheet_raised)
                        .shadow(paper::floating(&c))
                        // A click inside the sheet does not close it.
                        .on_click(|_, _, cx| cx.stop_propagation())
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
                                .child(kit::search_well(&self.all_models.filter, 38., None, cx)),
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
        let info = sayso_core::models::catalog().into_iter().find(|i| i.is_multilingual()).expect("a multilingual model");
        let language = languages::display(info.languages.iter().find(|l| *l != languages::AUTO).unwrap());
        assert!(matches(&info, ""));
        assert!(matches(&info, &format!("  {}  ", info.name.to_uppercase())));
        assert!(matches(&info, &info.vendor));
        assert!(matches(&info, &language.to_lowercase()));
        assert!(!matches(&info, "no model has this text"));
    }
}
