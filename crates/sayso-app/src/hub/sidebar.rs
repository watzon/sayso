//! The Hub sidebar. In Settings it becomes the settings sidebar.

use super::{Route, SettingsPage};
use crate::model::AppModel;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::config::ThemeMode;
use sayso_ui::ActivePaper;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::text;

pub fn sidebar(model: &Entity<AppModel>, route: Route, cx: &App) -> impl IntoElement {
    let c = cx.paper().colors;
    let col = div()
        .relative()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(232.))
        .h_full()
        // Room for the native traffic lights.
        .pt(px(52.))
        .px(px(14.))
        .pb(px(16.))
        .gap(px(24.));
    match route {
        Route::Settings(page) => col
            .child(back_button(model, cx))
            .child(settings_nav(model, page))
            .child(div().flex_1())
            .child(config_footer(model, cx)),
        _ => col
            .child(wordmark(cx))
            .child(main_nav(model, route, cx))
            .child(div().flex_1())
            .child(main_footer(model, cx)),
    }
    .text_color(c.ink)
}

fn wordmark(cx: &App) -> impl IntoElement {
    let c = cx.paper().colors;
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(8.))
        .child(icon(Icon::Wordmark, 22., c.ink))
        .child(
            div()
                .font_family(sayso_ui::fonts::DISPLAY)
                .text_size(px(24.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Sayso"),
        )
}

fn go(model: &Entity<AppModel>, route: Route) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let model = model.clone();
    move |_, _, cx| model.update(cx, |m, cx| m.navigate(route, cx))
}

fn main_nav(model: &Entity<AppModel>, route: Route, cx: &App) -> impl IntoElement {
    let history = model.read(cx).history_visible();
    let items = [
        (Route::Home, Icon::Home, "Home"),
        (Route::History, Icon::History, "History"),
        (Route::Dictionary, Icon::Dictionary, "Dictionary"),
        (Route::Styles, Icon::Styles, "Styles"),
        (Route::Models, Icon::Models, "Models"),
        (Route::Settings(SettingsPage::Dictation), Icon::Settings, "Settings"),
    ];
    let mut nav = div().flex().flex_col().gap(px(2.));
    for (i, (r, ic, label)) in items.into_iter().enumerate().filter(|(_, (r, ..))| history || *r != Route::History) {
        let active = route == r;
        nav = nav.child(NavItem::new(("nav", i), ic, label, active).on_click(go(model, r)));
    }
    nav
}

fn main_footer(model: &Entity<AppModel>, cx: &App) -> impl IntoElement {
    let c = cx.paper().colors;
    let m = model.read(cx);
    let (status, detail, color) = m.engine_summary();
    let selected = match m.config.appearance.theme {
        ThemeMode::System => 0,
        ThemeMode::Light => 1,
        ThemeMode::Dark => 2,
    };
    let update = crate::update_ui::sidebar_slip(model, cx);
    let model = model.clone();
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .px(px(4.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(6.))
                .child(status_dot(color(&c), 8.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(text::ui(status, 13., FontWeight::SEMIBOLD, c.ink))
                        .child(text::ui(detail, 12., FontWeight::NORMAL, c.graphite)),
                ),
        )
        .children(update)
        .child(
            Segmented::new("theme", ["Auto", "Light", "Dark"], selected)
                .with_icons([Icon::Display, Icon::Sun, Icon::Moon])
                .compact()
                .on_select(move |i, _, cx| {
                    let mode = [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark][i];
                    model.update(cx, |m, cx| m.edit_config(cx, |c| c.appearance.theme = mode));
                }),
        )
}

fn back_button(model: &Entity<AppModel>, cx: &App) -> impl IntoElement {
    let c = cx.paper().colors;
    let model = model.clone();
    div()
        .id("settings-back")
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(32.))
        .px(px(8.))
        .rounded(px(8.))
        .cursor_pointer()
        .hover(|s| s.bg(c.deboss.opacity(0.55)))
        .on_click(move |_, _, cx| model.update(cx, |m, cx| m.navigate(Route::Home, cx)))
        .child(icon(Icon::ChevronLeft, 16., c.ink))
        .child(
            div()
                .font_family(sayso_ui::fonts::DISPLAY)
                .text_size(px(22.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Settings"),
        )
}

pub fn settings_label(page: SettingsPage) -> (&'static str, Icon) {
    match page {
        SettingsPage::General => ("General", Icon::General),
        SettingsPage::Dictation => ("Dictation", Icon::Mic),
        SettingsPage::Overlay => ("Overlay", Icon::Overlay),
        SettingsPage::Audio => ("Audio", Icon::Audio),
        SettingsPage::Appearance => ("Appearance", Icon::Drop),
        SettingsPage::HistoryPrivacy => ("History and privacy", Icon::Shield),
        SettingsPage::Permissions => ("Permissions", Icon::Lock),
        SettingsPage::Advanced => ("Advanced", Icon::Code),
    }
}

fn settings_nav(model: &Entity<AppModel>, current: SettingsPage) -> impl IntoElement {
    let mut nav = div().flex().flex_col().gap(px(2.));
    for (i, page) in SettingsPage::ALL.into_iter().enumerate() {
        let (label, ic) = settings_label(page);
        nav = nav.child(NavItem::new(("settings-nav", i), ic, label, page == current).on_click(go(model, Route::Settings(page))));
    }
    nav
}

fn config_footer(model: &Entity<AppModel>, cx: &App) -> impl IntoElement {
    let c = cx.paper().colors;
    let path = model.read(cx).config_path_display();
    let model = model.clone();
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .px(px(10.))
        .child(
            div()
                .id("open-config")
                .cursor_pointer()
                .on_click(move |_, _, cx| model.read(cx).reveal_config_file())
                .child(text::ui("Open config file", 13., FontWeight::SEMIBOLD, c.accent)),
        )
        .child(text::mono(path, 11., c.graphite).font_weight(FontWeight::NORMAL))
        .when(false, |d| d)
}
