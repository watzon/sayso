//! Settings › Appearance: ink, light and dark specimens, theme, texture, motion.

use super::kit::{self, group, row};
use crate::model::AppModel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::config::ThemeMode;
use sayso_core::ink::{Appearance, Ink, NamedInk, NamedInkLabel as _, Palette, Rgb};
use sayso_i18n::t;
use sayso_ui::a11y::{A11yExt as _, Pressable, Role};
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

    /// Choose the ink at `index`. The last index is Custom.
    fn choose_ink(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        match NamedInk::ALL.get(index).copied() {
            Some(named) => {
                self.custom_open = false;
                self.model.update(cx, |m, cx| m.edit_config(cx, |c| c.appearance.ink = Ink::Named(named)));
            }
            None => {
                self.custom_open = true;
                let current = self.model.read(cx).config.appearance.ink;
                let rgb = current.color();
                let hex = rgb.to_hex();
                self.hex.update(cx, |s, cx| s.set_value(hex, window, cx));
                self.model.update(cx, |m, cx| m.edit_config(cx, |c| c.appearance.ink = Ink::Custom(rgb)));
            }
        }
    }

    fn ink_wells(&mut self, ink: Ink, cx: &mut Context<Self>) -> Stateful<Div> {
        let c = cx.paper().colors;
        // One radio group. The arrow keys change the ink, as in `Segmented`.
        let count = NamedInk::ALL.len() + 1;
        let selected_index = match ink {
            Ink::Named(n) => NamedInk::ALL.iter().position(|a| *a == n).unwrap_or(0),
            Ink::Custom(_) => count - 1,
        };
        let mut wells = div()
            .id("ink-wells")
            .role(Role::RadioGroup)
            .aria_label(t!("settings.appearance.ink"))
            .flex()
            .gap(px(20.))
            .pb(px(36.))
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| {
                let next = match e.keystroke.key.as_str() {
                    "right" | "down" if selected_index + 1 < count => selected_index + 1,
                    "left" | "up" if selected_index > 0 => selected_index - 1,
                    _ => return,
                };
                cx.stop_propagation();
                this.choose_ink(next, window, cx);
                if next > selected_index { window.focus_next(cx) } else { window.focus_prev(cx) }
            }));
        for (i, named) in NamedInk::ALL.into_iter().enumerate() {
            let selected = ink == Ink::Named(named);
            wells = wells.child(
                well_column(("ink", i), named.label(), selected, i, count)
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
                    .child(well_label(&named.label(), selected, &c))
                    .on_click(cx.listener(move |this, _, window, cx| this.choose_ink(i, window, cx))),
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
            well_column("ink-custom", t!("settings.appearance.custom"), custom_selected, count - 1, count)
                .child(custom_well)
                .child(well_label(&t!("settings.appearance.custom"), custom_selected, &c))
                .on_click(cx.listener(move |this, _, window, cx| this.choose_ink(count - 1, window, cx))),
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
                .child(text::ui(t!("settings.appearance.custom_ink"), 15., FontWeight::SEMIBOLD, c.ink))
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
                        .child(Input::new(&self.hex).aria_label(t!("settings.appearance.custom_ink")).appearance(false).w_full()),
                )
                .child(text::ui(t!("settings.appearance.hex_hint"), 13., FontWeight::NORMAL, c.graphite)),
        );
        if self.hex_error {
            col = col.child(super::kit::notice(
                BannerKind::Warning,
                t!("settings.appearance.hex_error"), cx));
        } else if !report.passes() {
            col = col.child(super::kit::notice(
                BannerKind::Warning,
                t!(
                    "settings.appearance.low_contrast",
                    light = sayso_i18n::numbers::decimal(report.light_ratio, 1),
                    dark = sayso_i18n::numbers::decimal(report.dark_ratio, 1)
                ), cx));
        }
        Some(col)
    }
}

/// One ink in the group: a radio button named for the ink.
fn well_column(id: impl Into<ElementId>, name: impl Into<SharedString>, selected: bool, index: usize, count: usize) -> Pressable {
    Pressable::new(id).a11y_radio(name, selected, index, count).flex().flex_col().flex_none().items_center().gap(px(10.)).w(px(76.)).cursor_pointer()
}

/// The text of a well. Plain text: the radio button carries the name.
fn well_label(name: &str, selected: bool, c: &Colors) -> Div {
    div()
        .font_family(sayso_ui::fonts::UI)
        .text_size(px(13.))
        .font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
        .text_color(if selected { c.ink } else { c.graphite })
        .child(name.to_string())
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
fn specimen(appearance: Appearance, ink: Ink, cx: &App) -> Stateful<Div> {
    let c = cx.paper().colors_for(appearance);
    let palette = Palette::new(ink, appearance);
    let ratio = palette.ink.contrast(palette.sheet);
    let pass = ratio >= 4.5;
    let ratio_text = if pass {
        t!("settings.appearance.specimen_pass", ratio = sayso_i18n::numbers::decimal(ratio, 1))
    } else {
        t!("settings.appearance.specimen_fail", ratio = sayso_i18n::numbers::decimal(ratio, 1))
    };
    // Each specimen has an id, so the text inside has unique ids.
    let (id, name) = match appearance {
        Appearance::Light => ("specimen-light", t!("settings.appearance.theme_light")),
        Appearance::Dark => ("specimen-dark", t!("settings.appearance.theme_dark")),
    };
    div()
        .id(id)
        .role(Role::Group)
        .aria_label(name)
        .flex()
        .flex_col()
        .flex_1()
        .gap(px(14.))
        .py(px(20.))
        .px(px(22.))
        .rounded(px(14.))
        .bg(c.sheet)
        .shadow(paper::raised(&c))
        .child(text::title(t!("settings.appearance.specimen_title"), 22., &c).line_height(px(28.)))
        .child(text::ui(t!("settings.appearance.specimen_body"), 13., FontWeight::NORMAL, c.graphite).line_height(px(16.)))
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
                        .child(text::ui(t!("settings.appearance.specimen_primary"), 12., FontWeight::SEMIBOLD, c.on_ink)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(kit::halo_dot(10., c.accent, 3., 0.18))
                        .child(text::ui(t!("settings.appearance.specimen_recording"), 12., FontWeight::SEMIBOLD, c.accent)),
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
        let theme = Segmented::new("theme-mode", [t!("settings.appearance.theme_auto"), t!("settings.appearance.theme_light"), t!("settings.appearance.theme_dark")], theme_index)
            .label(t!("settings.appearance.theme"))
            .on_select(move |i, _, cx| {
            let mode = [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark][i];
            m1.update(cx, |m, cx| m.edit_config(cx, |c| c.appearance.theme = mode));
        });
        let texture = Switch::new("paper-texture", a.paper_texture).label(t!("settings.appearance.texture")).on_toggle(move |on, _, cx| {
            m2.update(cx, |m, cx| m.edit_config(cx, |c| c.appearance.paper_texture = on));
        });
        let motion = Segmented::new("reduce-motion", [t!("settings.appearance.motion_auto"), t!("settings.appearance.motion_on"), t!("settings.appearance.motion_off")], motion_index)
            .label(t!("settings.appearance.reduce_motion"))
            .on_select(move |i, _, cx| {
            let v = [None, Some(true), Some(false)][i];
            m3.update(cx, |m, cx| m.edit_config(cx, |c| c.appearance.reduce_motion = v));
        });

        let ink_group = div()
            .flex()
            .flex_col()
            .pt(px(8.))
            .child(crate::widgets::section(&t!("settings.appearance.ink"), cx).pb(px(12.)))
            .child(
                text::ui(
                    t!("settings.appearance.ink_detail"),
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
        let title_bar = Switch::new("system-title-bar", a.system_title_bar).label(t!("settings.appearance.system_title_bar")).on_toggle(move |on, window, cx| {
            m3.update(cx, |m, cx| m.edit_config(cx, |c| c.appearance.system_title_bar = on));
            // The Hub is open now, so apply the change at once.
            crate::chrome::apply(&m3.read(cx).config, window);
        });

        let paper_group = group(&t!("settings.appearance.paper"), cx)
            .child(row(
                &t!("settings.appearance.theme"),
                &t!(crate::shell::os_text!("settings.appearance.theme_detail_mac", "settings.appearance.theme_detail_windows", "settings.appearance.theme_detail_other")),
                theme,
                cx,
            ))
            .child(row(&t!("settings.appearance.texture"), &t!("settings.appearance.texture_detail"), texture, cx))
            .child(row(
                &t!("settings.appearance.reduce_motion"),
                &t!(crate::shell::os_text!("settings.appearance.motion_detail_mac", "settings.appearance.motion_detail_windows", "settings.appearance.motion_detail_other")),
                motion,
                cx,
            ));

        // Only Linux lets an app choose who draws the title bar.
        let paper_group = paper_group.when(cfg!(target_os = "linux"), |g| {
            g.child(row(
                &t!("settings.appearance.system_title_bar"),
                &t!("settings.appearance.system_title_bar_detail"),
                title_bar,
                cx,
            ))
        });

        let body = kit::body().child(ink_group).child(paper_group);
        kit::page("appearance-page", &t!("settings.appearance.title"), &t!("settings.appearance.subtitle"), body, cx)
    }
}
