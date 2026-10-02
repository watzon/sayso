//! Visual check of the design system: `cargo run -p sayso-ui --example gallery [dark]`.

use gpui_kit::*;
use sayso_core::ink::{Appearance, Ink};
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::texture::{Grain, grain};
use sayso_ui::{ActivePaper, PaperTheme, text};
use std::time::Instant;

struct Gallery {
    start: Instant,
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.request_animation_frame();
        let t = self.start.elapsed().as_secs_f32();
        let c = cx.paper().colors;
        let levels: Vec<f32> = (0..32).map(|i| 0.35 + 0.6 * ((i as f32 * 0.7 + t * 3.0).sin() * 0.5 + 0.5)).collect();
        div()
            .relative()
            .size_full()
            .bg(c.ground)
            .child(grain(Grain::Board, px(0.), cx))
            .child(
                div()
                    .absolute()
                    .top(px(40.))
                    .left(px(40.))
                    .right(px(40.))
                    .bottom(px(40.))
                    .flex()
                    .flex_col()
                    .gap(px(24.))
                    .p(px(40.))
                    .rounded(px(14.))
                    .bg(c.sheet)
                    .shadow(sayso_ui::paper::sheet(&c))
                    .child(text::display("Good morning, Chris.", 44., &c))
                    .child(text::caps("Thursday, October 1", &c))
                    .child(text::body("Words help Sayso hear names and terms. Replacements change the text.", &c))
                    .child(
                        div().flex().gap(px(12.)).items_center()
                            .child(Button::new("p", "Copy").primary())
                            .child(Button::new("s", "Enhance"))
                            .child(Button::new("g", "Back").ghost())
                            .child(Keycaps::new(["⌥", "Space"]))
                            .child(Switch::new("sw1", true))
                            .child(Switch::new("sw2", false))
                            .child(StyleTag::new("Message", Some(c.accent)))
                            .child(Badge::new("Cloud", BadgeTone::Accent).icon(Icon::Cloud))
                            .child(Seal::new(28.)),
                    )
                    .child(
                        div().flex().gap(px(12.)).items_center()
                            .child(Segmented::new("seg", ["Auto", "Light", "Dark"], 1).with_icons([Icon::Display, Icon::Sun, Icon::Moon]))
                            .child(Chip::new("c1", "All", true))
                            .child(Chip::new("c2", "Enhanced", false))
                            .child(Chip::new("c3", "Message", true).dot(c.accent))
                            .child(Stepper::new("st", "4 s"))
                            .child(div().w(px(160.)).child(Progress::new(0.62)))
                            .child(AppBadge::new("Slack")),
                    )
                    .child(
                        div().flex().gap(px(16.)).items_center()
                            .child(
                                div().flex().items_center().gap(px(14.)).pl(px(10.)).pr(px(18.)).py(px(10.)).rounded_full().floating(&c)
                                    .child(div().flex().items_center().justify_center().size(px(36.)).rounded_full().debossed(&c).child(status_dot(c.accent, 12.)))
                                    .child(ink_waveform(levels, t, c.ink).w(px(232.)).h(px(36.)))
                                    .child(text::mono("0:07", 13., c.graphite)),
                            )
                            .child(div().w(px(150.)).h(px(36.)).child(ink_line((t * 0.3).fract(), c.ink, c.pencil).size_full()))
                            .child(div().w(px(200.)).h(px(40.)).child(audio_bars((0..48).map(|i| ((i as f32 * 0.5).sin().abs() * 255.) as u8).collect(), 0.4, c.ink, c.deboss_shade).size_full())),
                    )
                    .child(div().w(px(220.)).child(NavItem::new("n1", Icon::Home, "Home", true)))
                    .child(div().w(px(220.)).child(NavItem::new("n2", Icon::History, "History", false)))
                    .child(Banner::new(BannerKind::Warning, "ChatGPT also uses Option+Space. Both apps will react."))
                    .child(text::serif("Let's move the standup to Thursday and invite the design team.", 17., c.ink)),
            )
    }
}

fn main() {
    let dark = std::env::args().any(|a| a == "dark");
    gpui_kit::application().with_assets(sayso_ui::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        sayso_ui::init(cx);
        sayso_ui::theme::set(cx, PaperTheme::new(Ink::default(), if dark { Appearance::Dark } else { Appearance::Light }, false, true));
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        cx.open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
            |_, cx| cx.new(|_| Gallery { start: Instant::now() }),
        )
        .unwrap();
        cx.activate(true);
    });
}
