//! The Hub sidebar. In Settings it becomes the settings sidebar.

use super::{Route, SettingsPage};
use crate::layout::{RAIL_W, SIDEBAR_W};
use crate::model::AppModel;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::config::ThemeMode;
use sayso_ui::ActivePaper;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::text;

/// The sidebar, or the rail of icons when `rail` is true. `forced` says that
/// the window is too narrow for the sidebar, so there is no button to expand it.
pub fn sidebar(model: &Entity<AppModel>, route: Route, rail: bool, forced: bool, cx: &App) -> impl IntoElement {
    let toggle = (!forced).then(|| collapse_button(model, rail, cx));
    let c = cx.paper().colors;
    let col = div()
        .relative()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(if rail { RAIL_W } else { SIDEBAR_W }))
        .h_full()
        // Room for the native traffic lights.
        .pt(px(52.))
        .px(px(14.))
        .pb(px(16.))
        .gap(px(24.))
        .when(rail, |d| d.px_0().items_center());
    match route {
        Route::Settings(page) => col
            .child(back_button(model, rail, cx))
            .child(settings_nav(model, page, rail))
            .child(div().flex_1())
            .child(config_footer(model, rail, toggle, cx)),
        _ => col
            .child(wordmark(rail, cx))
            .child(main_nav(model, route, rail, cx))
            .child(div().flex_1())
            .child(if rail { rail_footer(model, toggle, cx).into_any_element() } else { main_footer(model, toggle, cx).into_any_element() }),
    }
    .text_color(c.ink)
}

/// The button that collapses the sidebar to the rail, or expands the rail.
fn collapse_button(model: &Entity<AppModel>, rail: bool, cx: &App) -> AnyElement {
    let c = cx.paper().colors;
    let model = model.clone();
    div()
        .id("sidebar-toggle")
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(if rail { 36. } else { 30. }))
        .h(px(30.))
        .rounded(px(8.))
        .cursor_pointer()
        .hover(|s| s.bg(c.deboss.opacity(0.55)))
        .tooltip(text_tooltip(if rail { "Expand sidebar" } else { "Collapse sidebar" }))
        .on_click(move |_, _, cx| model.update(cx, |m, cx| m.edit_config(cx, |c| c.appearance.sidebar_collapsed = !rail)))
        .child(icon(Icon::Sidebar, 18., c.graphite))
        .into_any_element()
}

fn wordmark(rail: bool, cx: &App) -> impl IntoElement {
    let c = cx.paper().colors;
    let mark = div().flex().items_center().gap(px(10.)).h(px(30.)).px(px(8.)).child(icon(Icon::Wordmark, 22., c.ink));
    if rail {
        return mark;
    }
    mark.child(
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

fn main_nav(model: &Entity<AppModel>, route: Route, rail: bool, cx: &App) -> impl IntoElement {
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
        nav = nav.child(NavItem::new(("nav", i), ic, label, active).icon_only(rail).on_click(go(model, r)));
    }
    nav
}

fn main_footer(model: &Entity<AppModel>, toggle: Option<AnyElement>, cx: &App) -> impl IntoElement {
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
                        .flex_1()
                        .min_w_0()
                        .gap(px(1.))
                        .child(text::ui(status, 13., FontWeight::SEMIBOLD, c.ink))
                        .child(text::ui(detail, 12., FontWeight::NORMAL, c.graphite)),
                )
                .when_some(toggle, |d, t| d.pr_0().child(t)),
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

/// The theme switch and the engine status of the rail.
fn rail_footer(model: &Entity<AppModel>, toggle: Option<AnyElement>, cx: &App) -> impl IntoElement {
    let c = cx.paper().colors;
    let m = model.read(cx);
    let (status, detail, color) = m.engine_summary();
    let selected = match m.config.appearance.theme {
        ThemeMode::System => 0,
        ThemeMode::Light => 1,
        ThemeMode::Dark => 2,
    };
    let update = crate::update_ui::rail_slip(model, cx);
    let model = model.clone();
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(16.))
        .child(
            div()
                .id("engine-status")
                .flex()
                .items_center()
                .justify_center()
                .size(px(24.))
                .tooltip(text_tooltip(format!("{status}. {detail}")))
                .child(status_dot(color(&c), 8.)),
        )
        .children(update)
        .child(
            Segmented::new("theme", ["Auto", "Light", "Dark"], selected)
                .with_icons([Icon::Display, Icon::Sun, Icon::Moon])
                .rail()
                .on_select(move |i, _, cx| {
                    let mode = [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark][i];
                    model.update(cx, |m, cx| m.edit_config(cx, |c| c.appearance.theme = mode));
                }),
        )
        .children(toggle)
}

fn back_button(model: &Entity<AppModel>, rail: bool, cx: &App) -> impl IntoElement {
    let c = cx.paper().colors;
    let model = model.clone();
    let back = div()
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
        .child(icon(Icon::ChevronLeft, 16., c.ink));
    if rail {
        return back.w(px(36.)).px_0().justify_center().tooltip(text_tooltip("Back"));
    }
    back.child(
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

fn settings_nav(model: &Entity<AppModel>, current: SettingsPage, rail: bool) -> impl IntoElement {
    let mut nav = div().flex().flex_col().gap(px(2.));
    for (i, page) in SettingsPage::ALL.into_iter().enumerate() {
        let (label, ic) = settings_label(page);
        nav = nav.child(NavItem::new(("settings-nav", i), ic, label, page == current).icon_only(rail).on_click(go(model, Route::Settings(page))));
    }
    nav
}

fn config_footer(model: &Entity<AppModel>, rail: bool, toggle: Option<AnyElement>, cx: &App) -> AnyElement {
    let c = cx.paper().colors;
    let path = model.read(cx).config_path_display();
    let model = model.clone();
    if rail {
        let open = div()
            .id("open-config")
            .flex()
            .items_center()
            .justify_center()
            .size(px(36.))
            .rounded(px(9.))
            .cursor_pointer()
            .hover(|s| s.bg(c.deboss.opacity(0.55)))
            .tooltip(text_tooltip(format!("Open config file: {path}")))
            .on_click(move |_, _, cx| model.read(cx).reveal_config_file())
            .child(icon(Icon::File, 18., c.accent));
        return div().flex().flex_col().items_center().gap(px(16.)).child(open).children(toggle).into_any_element();
    }
    let file = div()
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
        .child(text::mono(path, 11., c.graphite).font_weight(FontWeight::NORMAL));
    div().flex().items_end().child(file.flex_1().min_w_0()).children(toggle).into_any_element()
}
