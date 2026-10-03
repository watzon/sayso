//! Step 7: done, with a short tour. "Open Sayso" finishes onboarding.

use super::OnboardingView;
use gpui_kit::*;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper;
use sayso_ui::{ActivePaper, Colors, text};

fn tour_card(art: Div, title: &'static str, body: &'static str, c: &Colors) -> Div {
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
            (Some(t), _) => format!("Press {} in any app to dictate. Here is where Sayso lives.", t.keycaps().join(" ")),
            (None, Some(p)) => format!("Hold {} in any app to dictate. Here is where Sayso lives.", p.keycaps().join(" ")),
            (None, None) => "Set a hotkey in Settings › Dictation to dictate in any app. Here is where Sayso lives.".to_string(),
        };
        let paste = m.config.hotkeys.paste_last;
        // Shown only on a Mac that has Pindrop data.
        let pindrop = m.pindrop_found.then(|| {
            let state = m.pindrop_import.clone();
            let running = state == crate::pindrop_import::PindropImport::Running;
            let message = match &state {
                crate::pindrop_import::PindropImport::Idle => "You used Pindrop on this computer. Sayso can copy your dictations, dictionary, and prompt presets.".to_string(),
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
            Some(hk) => div().child(Keycaps::hotkey(&hk).size(KeySize::Small)),
            None => div().child(text::ui("Not set", 12., FontWeight::NORMAL, c.graphite)),
        };

        let title = |color: Hsla| {
            div()
                .font_family(sayso_ui::fonts::DISPLAY)
                .text_size(px(48.))
                .line_height(px(58.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child("You are ready")
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(26.))
            .px(px(72.))
            .pb(px(16.))
            .child(Seal::new(92.).ringed())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.))
                    .child(div().relative().child(title(c.highlight(0.85)).absolute().top(px(1.)).left_0()).child(title(c.ink)))
                    .child(text::ui(lead, 16., FontWeight::NORMAL, c.graphite).line_height(px(24.)).text_center().w(px(480.))),
            )
            .child(
                div()
                    .flex()
                    .w_full()
                    .gap(px(14.))
                    .child(tour_card(pill, "The pill", "Bottom of your screen. Click it, or drag it where you like.", &c))
                    .child(tour_card(menu, crate::shell::os_text!("The menu bar", "The tray icon"), "Switch style, model, or microphone, and copy your last text.", &c))
                    .child(tour_card(keys, "Paste last text", "If text did not land where you wanted, paste it again.", &c)),
            )
            .children(pindrop.map(|(message, running)| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .w_full()
                    .child(text::ui(message, 13., FontWeight::NORMAL, c.graphite).line_height(px(19.)).flex_1())
                    .child(Button::new("import-pindrop", "Import from Pindrop").small().disabled(running).on_click(cx.listener(|this, _, _, cx| {
                        this.model.update(cx, |m, cx| m.import_pindrop(cx));
                    })))
            }))
            .into_any_element()
    }
}
