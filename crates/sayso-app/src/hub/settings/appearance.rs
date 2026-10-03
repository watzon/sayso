//! Settings › Appearance: ink, light and dark specimens, theme, texture, motion.

use super::kit::{self, group, row};
use crate::model::AppModel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::config::ThemeMode;
use sayso_core::ink::{Appearance, Ink, NamedInk, Palette, Rgb};
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::{self, PaperStyled};
use sayso_ui::{ActivePaper, Colors, text};

pub struct AppearanceSettings {
    model: Entity<AppModel>,
    hex: Entity<InputState>,
    /// The custom field is open (the user chose Custom).
    custom_open: bool,
    hex_error: bool,
    _hex_sub: Subscription,
}

impl AppearanceSettings {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ink = model.read(cx).config.appearance.ink;
        let initial = match ink {
            Ink::Custom(c) => c.to_hex(),
            Ink::Named(_) => String::new(),
        };
        let hex = crate::widgets::input_state("#2D3C8C", &initial, window, cx);
        let sub = cx.subscribe_in(&hex, window, |this, state, ev: &InputEvent, _window, cx| {
            if !matches!(ev, InputEvent::Change | InputEvent::PressEnter { .. } | InputEvent::Blur) {
                return;
            }
            let value = state.read(cx).value().to_string();
            match Rgb::parse(&value) {
                Some(rgb) => {
                    this.hex_error = false;
                    this.model.update(cx, |m, cx| m.edit_config(cx, |c| c.appearance.ink = Ink::Custom(rgb)));
                }
                None => {
                    // Only complain once the value is long enough to be a color.
                    this.hex_error = value.trim().trim_start_matches('#').len() >= 6 || matches!(ev, InputEvent::Blur | InputEvent::PressEnter { .. }) && !value.trim().is_empty();
                    cx.notify();
                }
            }
        });
        Self { model, hex, custom_open: matches!(ink, Ink::Custom(_)), hex_error: false, _hex_sub: sub }
    }

    fn ink_wells(&mut self, ink: Ink, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let mut wells = div().flex().gap(px(20.)).pb(px(36.));
        for (i, named) in NamedInk::ALL.into_iter().enumerate() {
            let selected = ink == Ink::Named(named);
            wells = wells.child(
                well_column(("ink", i), named.name(), selected, &c)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(60.))
                            .rounded_full()
                            .bg(c.deboss)
                            .shadow(well_shadow(selected, &c))
                            .child(ink_drop(named.color())),
                    )
                    .child(well_label(named.name(), selected, &c))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.custom_open = false;
                        this.model.update(cx, |m, cx| m.edit_config(cx, |c| c.appearance.ink = Ink::Named(named)));
                    })),
            );
        }
        let custom_selected = matches!(ink, Ink::Custom(_));
        let custom_well = match ink {
            Ink::Custom(rgb) => div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(60.))
                .rounded_full()
                .bg(c.deboss)
                .shadow(well_shadow(true, &c))
                .child(ink_drop(rgb)),
            Ink::Named(_) => div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(60.))
                .rounded_full()
                .border(px(1.5))
                .border_dashed()
                .border_color(c.deboss_shade)
                .child(icon(Icon::Plus, 16., c.graphite)),
        };
        wells.child(
            well_column("ink-custom", "Custom", custom_selected, &c)
                .child(custom_well)
                .child(well_label("Custom", custom_selected, &c))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.custom_open = true;
                    let current = this.model.read(cx).config.appearance.ink;
                    let rgb = current.color();
                    let hex = rgb.to_hex();
                    this.hex.update(cx, |s, cx| s.set_value(hex, window, cx));
                    this.model.update(cx, |m, cx| m.edit_config(cx, |c| c.appearance.ink = Ink::Custom(rgb)));
                })),
        )
    }

    fn custom_field(&self, ink: Ink, cx: &mut Context<Self>) -> Option<Div> {
        if !self.custom_open {
            return None;
        }
        let c = cx.paper().colors;
        let report = Palette::contrast_report(ink);
        let mut col = div().flex().flex_col().gap(px(10.)).pb(px(28.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(text::ui("Custom ink", 15., FontWeight::SEMIBOLD, c.ink))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .w(px(150.))
                        .h(px(34.))
                        .px(px(12.))
                        .rounded(px(9.))
                        .debossed(&c)
                        .font_family(sayso_ui::fonts::MONO)
                        .text_size(px(13.))
                        .child(Input::new(&self.hex).appearance(false).w_full()),
                )
                .child(text::ui("A hex color, for example #2D3C8C.", 13., FontWeight::NORMAL, c.graphite)),
        );
        if self.hex_error {
            col = col.child(super::kit::notice(
                BannerKind::Warning,
                "This is not a color. Enter six hex digits, for example #2D3C8C.", cx));
        } else if !report.passes() {
            col = col.child(super::kit::notice(
                BannerKind::Warning,
                format!(
                    "Text in this ink is hard to read: {:.1}:1 on light paper and {:.1}:1 on dark paper. Text needs at least 4.5:1. Choose a darker or stronger color.",
                    report.light_ratio, report.dark_ratio
                ), cx));
        }
        Some(col)
    }
}

fn well_column(id: impl Into<ElementId>, _name: &str, _selected: bool, _c: &Colors) -> Stateful<Div> {
    div().id(id).flex().flex_col().flex_none().items_center().gap(px(10.)).w(px(76.)).cursor_pointer()
}

fn well_label(name: &str, selected: bool, c: &Colors) -> Div {
    text::ui(
        name.to_string(),
        13.,
        if selected { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM },
        if selected { c.ink } else { c.graphite },
    )
}

fn well_shadow(selected: bool, c: &Colors) -> Vec<BoxShadow> {
    let mut s = vec![
        BoxShadow::new(px(0.), px(2.), c.shadow(0.28)).blur_radius(px(4.)).inset(),
        BoxShadow::new(px(0.), px(-1.), c.highlight(0.7)).inset(),
    ];
    if selected {
        s.push(BoxShadow::new(px(0.), px(0.), c.ink).spread_radius(px(2.)));
    }
    s
}

fn ink_drop(color: Rgb) -> Div {
    div().size(px(42.)).rounded_full().bg(sayso_ui::theme::hsla(color)).shadow(vec![
        BoxShadow::new(px(0.), px(3.), gpui_kit::white().opacity(0.15)).blur_radius(px(6.)).inset(),
        BoxShadow::new(px(0.), px(-3.), gpui_kit::black().opacity(0.42)).blur_radius(px(6.)).inset(),
    ])
}

/// A sheet of paper in one appearance with real colors and the ink contrast.
fn specimen(appearance: Appearance, ink: Ink, cx: &App) -> Div {
    let c = cx.paper().colors_for(appearance);
    let palette = Palette::new(ink, appearance);
    let ratio = palette.ink.contrast(palette.sheet);
    let pass = ratio >= 4.5;
    let ratio_text = if pass { format!("AA · {ratio:.1}:1") } else { format!("Below AA · {ratio:.1}:1") };
    div()
        .flex()
        .flex_col()
        .flex_1()
        .gap(px(14.))
        .py(px(20.))
        .px(px(22.))
        .rounded(px(14.))
        .bg(c.sheet)
        .shadow(paper::raised(&c))
        .child(text::title("The quick brown fox", 22., &c).line_height(px(28.)))
        .child(text::ui("Body text, secondary text, and a focus ring.", 13., FontWeight::NORMAL, c.graphite).line_height(px(16.)))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .h(px(28.))
                        .px(px(12.))
                        .rounded(px(7.))
                        .bg(c.ink_fill)
                        .child(text::ui("Primary", 12., FontWeight::SEMIBOLD, c.on_ink)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(kit::halo_dot(10., c.accent, 3., 0.18))
                        .child(text::ui("Recording", 12., FontWeight::SEMIBOLD, c.accent)),
                )
                .child(div().flex_1())
                .child(text::mono(ratio_text, 11., if pass { c.success } else { c.danger }).font_weight(FontWeight::NORMAL)),
        )
}

impl Render for AppearanceSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let a = self.model.read(cx).config.appearance.clone();
        let theme_index = match a.theme {
            ThemeMode::System => 0,
            ThemeMode::Light => 1,
            ThemeMode::Dark => 2,
        };
        let motion_index = match a.reduce_motion {
            None => 0,
            Some(true) => 1,
            Some(false) => 2,
        };
        let (m1, m2, m3) = (self.model.clone(), self.model.clone(), self.model.clone());
        let theme = Segmented::new("theme-mode", ["Auto", "Light", "Dark"], theme_index).on_select(move |i, _, cx| {
            let mode = [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark][i];
            m1.update(cx, |m, cx| m.edit_config(cx, |c| c.appearance.theme = mode));
        });
        let texture = Switch::new("paper-texture", a.paper_texture).on_toggle(move |on, _, cx| {
            m2.update(cx, |m, cx| m.edit_config(cx, |c| c.appearance.paper_texture = on));
        });
        let motion = Segmented::new("reduce-motion", ["Auto", "On", "Off"], motion_index).on_select(move |i, _, cx| {
            let v = [None, Some(true), Some(false)][i];
            m3.update(cx, |m, cx| m.edit_config(cx, |c| c.appearance.reduce_motion = v));
        });

        let ink_group = div()
            .flex()
            .flex_col()
            .pt(px(8.))
            .child(crate::widgets::section("Ink", cx).pb(px(12.)))
            .child(
                text::ui(
                    "Your ink colors text, the waveform, and buttons. Sayso picks the accent and the recording color to match, and checks contrast in light and dark.",
                    14.,
                    FontWeight::NORMAL,
                    c.graphite,
                )
                .line_height(px(20.))
                .w(px(560.))
                .pb(px(26.)),
            )
            .child(self.ink_wells(a.ink, cx))
            .when_some(self.custom_field(a.ink, cx), |d, f| d.child(f))
            .child(
                div()
                    .flex()
                    .gap(px(16.))
                    .child(specimen(Appearance::Light, a.ink, cx))
                    .child(specimen(Appearance::Dark, a.ink, cx)),
            );

        let m3 = self.model.clone();
        let title_bar = Switch::new("system-title-bar", a.system_title_bar).on_toggle(move |on, window, cx| {
            m3.update(cx, |m, cx| m.edit_config(cx, |c| c.appearance.system_title_bar = on));
            // The Hub is open now, so apply the change at once.
            crate::chrome::apply(&m3.read(cx).config, window);
        });

        let paper_group = group("Paper", cx)
            .child(row("Appearance", crate::shell::os_text!("Auto follows macOS.", "Auto follows the system."), theme, cx))
            .child(row("Paper texture", "Grain on the window background. Content stays almost clean.", texture, cx))
            .child(row("Reduce motion", crate::shell::os_text!("Ink appears without spreading. Auto follows macOS.", "Ink appears without spreading. Auto follows the system."), motion, cx));

        // Only Linux lets an app choose who draws the title bar.
        let paper_group = paper_group.when(cfg!(target_os = "linux"), |g| {
            g.child(row(
                "Use the system title bar",
                "Off: Sayso draws its own title bar and window buttons, and the paper goes to the top edge.",
                title_bar,
                cx,
            ))
        });

        let body = kit::body().child(ink_group).child(paper_group);
        kit::page("appearance-page", "Appearance", "Your ink, light and dark paper, texture, and motion.", body, cx)
    }
}
