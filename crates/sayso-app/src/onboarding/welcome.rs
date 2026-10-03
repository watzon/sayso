//! Step 1: Welcome.

use super::OnboardingView;
use gpui_kit::*;
use sayso_ui::ActivePaper;
use sayso_ui::components::ink_waveform;
use sayso_ui::text;

/// A calm stroke for the hero: high in the middle, two smaller swells.
const HERO: [f32; 12] = [0.35, 0.55, 0.8, 0.62, 0.9, 1.0, 0.86, 0.66, 0.84, 0.6, 0.42, 0.3];

impl OnboardingView {
    pub(super) fn welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let reduce = cx.paper().reduce_motion;
        let t = if reduce { 0.0 } else { self.started.elapsed().as_secs_f32() * 0.35 };
        if !reduce {
            window.request_animation_frame();
        }
        let levels: Vec<f32> = HERO.iter().enumerate().map(|(i, l)| l * (0.92 + 0.08 * (t * 2.0 + i as f32 * 0.7).sin())).collect();
        let droplet = |x: f32, y: f32, r: f32, a: f32| div().absolute().left(px(x)).top(px(y)).size(px(r * 2.)).rounded_full().bg(c.ink.opacity(a));
        let hero = div()
            .relative()
            .w(px(520.))
            .h(px(84.))
            .child(ink_waveform(levels, t, c.ink).size_full())
            .child(droplet(484., 21., 2.6, 0.4))
            .child(droplet(86., 60., 2.1, 0.3))
            .child(droplet(313., 74., 1.6, 0.3));
        let title = "Welcome to Sayso";
        let display = |color: Hsla| {
            div()
                .font_family(sayso_ui::fonts::DISPLAY)
                .text_size(px(56.))
                .line_height(px(59.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(title)
        };
        let fact = |label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().size(px(6.)).rounded_full().bg(c.ink))
                .child(text::ui(label, 14., FontWeight::NORMAL, c.ink).line_height(px(18.)))
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(28.))
            .px(px(96.))
            .pb(px(24.))
            .child(hero)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(14.))
                    .child(div().relative().child(display(c.highlight(0.85)).absolute().top(px(1.)).left_0()).child(display(c.ink)))
                    .child(
                        text::ui(
                            format!("Dictate into any app on your {0}. Speech recognition runs on this {0}, so your voice never leaves it.", crate::os::COMPUTER),
                            17.,
                            FontWeight::NORMAL,
                            c.graphite,
                        )
                        .line_height(px(26.))
                        .text_center()
                        .w(px(520.)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(28.))
                    .pt(px(8.))
                    .child(fact(if cfg!(target_os = "macos") { "Runs on the Neural Engine" } else { "Runs on this PC" }))
                    .child(fact("Works in every app"))
                    .child(fact("Setup takes about 3 minutes")),
            )
            .into_any_element()
    }
}
