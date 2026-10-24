//! Step 7: done, with a short tour. "Open Sayso" finishes onboarding.

use super::{OnboardingView, status_ui};
use gpui_kit::*;
use sayso_i18n::t;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper;
use sayso_ui::{ActivePaper, Colors, text};

fn tour_card(art: Div, title: String, body: String, c: &Colors) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap(px(12.))
        .py(px(18.))
        .px(px(16.))
        .rounded(px(14.))
        .bg(c.sheet_raised)
        .shadow(paper::raised(c))
        .child(div().flex().items_center().justify_center().h(px(40.)).child(art))
        .child(text::ui(title, 14., FontWeight::SEMIBOLD, c.ink).line_height(px(18.)))
        .child(text::ui(body, 13., FontWeight::NORMAL, c.graphite).line_height(px(19.)).text_center())
}

impl OnboardingView {
    pub(super) fn done_step(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let lead = match (m.config.hotkeys.toggle, m.config.hotkeys.push_to_talk) {
            (Some(hk), _) => t!("onboarding.done.lead_toggle", keys = hk.keycaps().join(" ")),
            (None, Some(hk)) => t!("onboarding.done.lead_push", keys = hk.keycaps().join(" ")),
            (None, None) => t!("onboarding.done.lead_none"),
        };
        let paste = m.config.hotkeys.paste_last;
        // Shown only on a Mac that has Pindrop data.
        let pindrop = m.pindrop_found.then(|| {
            let state = m.pindrop_import.clone();
            let running = state == crate::pindrop_import::PindropImport::Running;
            let message = match &state {
                crate::pindrop_import::PindropImport::Idle => t!("onboarding.done.pindrop_found"),
                other => other.message(),
            };
            (message, running)
        });

        let pill = div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(52.))
            .h(px(14.))
            .rounded_full()
            .bg(c.sheet)
            .shadow(vec![
                BoxShadow::new(px(0.), px(1.), c.highlight(1.0)).inset(),
                BoxShadow::new(px(0.), px(1.), c.shadow(0.2)).blur_radius(px(2.)),
                BoxShadow::new(px(0.), px(4.), c.shadow(0.14)).blur_radius(px(12.)),
            ])
            .child(div().w(px(18.)).h(px(2.)).rounded(px(2.)).bg(c.ink.opacity(0.55)));
        let menu = div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(34.))
            .h(px(24.))
            .rounded(px(6.))
            .bg(c.ink.opacity(0.1))
            .child(icon(Icon::Wordmark, 18., c.ink));
        let keys = match paste {
            Some(hk) => div().child(crate::widgets::hotkey_keycaps(&hk).size(KeySize::Small)),
            None => div().child(text::ui(t!("onboarding.done.not_set"), 12., FontWeight::NORMAL, c.graphite)),
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(26.))
            .px(px(self.side()))
            .pb(px(16.))
            .child(Seal::new(92.).ringed())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.))
                    .child(text::letterpress(t!("onboarding.done.title"), 48., 58., 1, &c))
                    .child(text::ui(lead, 16., FontWeight::NORMAL, c.graphite).line_height(px(24.)).text_center().w(px(480.))),
            )
            .child(
                div()
                    .flex()
                    .w_full()
                    .gap(px(14.))
                    .child(tour_card(pill, t!("onboarding.done.pill_title"), t!("onboarding.done.pill_body"), &c))
                    .child(tour_card(menu, t!(crate::shell::os_text!("onboarding.done.menu_title_mac", "onboarding.done.menu_title_other")), t!("onboarding.done.menu_body"), &c))
                    .child(tour_card(keys, t!("onboarding.done.paste_title"), t!("onboarding.done.paste_body"), &c)),
            )
            .children(pindrop.map(|(message, running)| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .w_full()
                    // The import state is live: a screen reader reads when it ends.
                    .child(status_ui("pindrop-status", message, 13., FontWeight::NORMAL, c.graphite).line_height(px(19.)).flex_1())
                    .child(Button::new("import-pindrop", t!("onboarding.done.import_pindrop")).small().disabled(running).on_click(cx.listener(|this, _, _, cx| {
                        this.model.update(cx, |m, cx| m.import_pindrop(cx));
                    })))
            }))
            .into_any_element()
    }
}
