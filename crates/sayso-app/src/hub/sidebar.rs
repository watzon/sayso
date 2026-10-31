//! The Hub sidebar. In Settings it becomes the settings sidebar.

use super::{Route, SettingsPage};
use crate::layout::{RAIL_W, SIDEBAR_W};
use crate::model::AppModel;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::config::ThemeMode;
use sayso_ui::fonts::{Face, LocaleFonts as _};
use sayso_ui::ActivePaper;
use sayso_ui::a11y::{LiveStatus, Pressable, Role};
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::text;
use sayso_i18n::t;

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
    let name = if rail { t!("hub.sidebar.expand") } else { t!("hub.sidebar.collapse") };
    Pressable::new("sidebar-toggle")
        .label(name.clone())
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(if rail { 36. } else { 30. }))
        .h(px(30.))
        .rounded(px(8.))
        .cursor_pointer()
        .hover(|s| s.bg(c.deboss.opacity(0.55)))
        .tooltip(text_tooltip(name))
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
                .font_family(sayso_ui::fonts::DISPLAY).locale_fonts(Face::Serif)
                .text_size(px(24.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(sayso_ui::a11y::A11yText::new("Sayso")),
        )
}

fn go(model: &Entity<AppModel>, route: Route) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let model = model.clone();
    move |_, _, cx| model.update(cx, |m, cx| m.navigate(route, cx))
}

fn main_nav(model: &Entity<AppModel>, route: Route, rail: bool, cx: &App) -> impl IntoElement {
    let history = model.read(cx).history_visible();
    let items = [
        (Route::Home, Icon::Home, t!("hub.nav.home")),
        (Route::History, Icon::History, t!("hub.nav.history")),
        (Route::Dictionary, Icon::Dictionary, t!("hub.nav.dictionary")),
        (Route::Styles, Icon::Styles, t!("hub.nav.styles")),
        (Route::Models, Icon::Models, t!("hub.nav.models")),
        (Route::Settings(SettingsPage::General), Icon::Settings, t!("hub.nav.settings")),
    ];
    let items: Vec<_> = items.into_iter().enumerate().filter(|(_, (r, ..))| history || *r != Route::History).collect();
    let count = items.len();
    let mut nav = div().id("main-nav").role(Role::TabList).aria_label(t!("hub.sidebar.main_nav")).flex().flex_col().gap(px(2.));
    for (k, (i, (r, ic, label))) in items.into_iter().enumerate() {
        let active = route == r;
        nav = nav.child(arrow_keys(k, count, NavItem::new(("nav", i), ic, label, active).position(k, count).icon_only(rail).on_click(go(model, r))));
    }
    nav
}

/// Up and Down move the focus to the previous and next row of a tab list,
/// except at the ends. The key event goes up from the focused row to this
/// wrapper, which knows the place `index` of `count` of its row.
fn arrow_keys(index: usize, count: usize, row: impl IntoElement) -> Div {
    div().child(row).on_key_down(move |e, window, cx| match e.keystroke.key.as_str() {
        "down" if index + 1 < count => {
            cx.stop_propagation();
            window.focus_next(cx);
        }
        "up" if index > 0 => {
            cx.stop_propagation();
            window.focus_prev(cx);
        }
        _ => {}
    })
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
                    // The engine state changes, so a screen reader reads it when it does.
                    LiveStatus::new("engine-status", t!("hub.sidebar.engine_status", status = status, detail = detail))
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
            Segmented::new("theme", [t!("hub.sidebar.theme_auto"), t!("hub.sidebar.theme_light"), t!("hub.sidebar.theme_dark")], selected)
                .with_icons([Icon::Display, Icon::Sun, Icon::Moon])
                .label(t!("hub.sidebar.theme"))
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
            LiveStatus::new("engine-status", t!("hub.sidebar.engine_status", status = status, detail = detail))
                .flex()
                .items_center()
                .justify_center()
                .size(px(24.))
                .tooltip(text_tooltip(t!("hub.sidebar.engine_status", status = status, detail = detail)))
                .child(status_dot(color(&c), 8.)),
        )
        .children(update)
        .child(
            Segmented::new("theme", [t!("hub.sidebar.theme_auto"), t!("hub.sidebar.theme_light"), t!("hub.sidebar.theme_dark")], selected)
                .with_icons([Icon::Display, Icon::Sun, Icon::Moon])
                .label(t!("hub.sidebar.theme"))
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
    let back = Pressable::new("settings-back")
        .label(t!("hub.sidebar.back"))
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
        return back.w(px(36.)).px_0().justify_center().tooltip(text_tooltip(t!("hub.sidebar.back")));
    }
    back.child(
            div()
                .font_family(sayso_ui::fonts::DISPLAY).locale_fonts(Face::Serif)
                .text_size(px(22.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(t!("hub.nav.settings")),
        )
}

pub fn settings_label(page: SettingsPage) -> (String, Icon) {
    match page {
        SettingsPage::General => (t!("hub.settings_nav.general"), Icon::General),
        SettingsPage::Dictation => (t!("hub.settings_nav.dictation"), Icon::Mic),
        SettingsPage::Providers => (t!("hub.settings_nav.providers"), Icon::Sparkle),
        SettingsPage::Overlay => (t!("hub.settings_nav.overlay"), Icon::Overlay),
        SettingsPage::Audio => (t!("hub.settings_nav.audio"), Icon::Audio),
        SettingsPage::Appearance => (t!("hub.settings_nav.appearance"), Icon::Drop),
        SettingsPage::HistoryPrivacy => (t!("hub.settings_nav.history_privacy"), Icon::Shield),
        SettingsPage::Permissions => (t!("hub.settings_nav.permissions"), Icon::Lock),
        SettingsPage::ConnectedApps => (t!("hub.settings_nav.connected_apps"), Icon::Apps),
        SettingsPage::Advanced => (t!("hub.settings_nav.advanced"), Icon::Code),
    }
}

fn settings_nav(model: &Entity<AppModel>, current: SettingsPage, rail: bool) -> impl IntoElement {
    let count = SettingsPage::ALL.len();
    let mut nav = div().id("settings-nav").role(Role::TabList).aria_label(t!("hub.sidebar.settings_sections")).flex().flex_col().gap(px(2.));
    for (i, page) in SettingsPage::ALL.into_iter().enumerate() {
        let (label, ic) = settings_label(page);
        nav = nav.child(arrow_keys(
            i,
            count,
            NavItem::new(("settings-nav", i), ic, label, page == current).position(i, count).icon_only(rail).on_click(go(model, Route::Settings(page))),
        ));
    }
    nav
}

fn config_footer(model: &Entity<AppModel>, rail: bool, toggle: Option<AnyElement>, cx: &App) -> AnyElement {
    let c = cx.paper().colors;
    let path = model.read(cx).config_path_display();
    let model = model.clone();
    if rail {
        let open = Pressable::new("open-config")
            .label(t!("hub.sidebar.open_config_file_path", path = path))
            .flex()
            .items_center()
            .justify_center()
            .size(px(36.))
            .rounded(px(9.))
            .cursor_pointer()
            .hover(|s| s.bg(c.deboss.opacity(0.55)))
            .tooltip(text_tooltip(t!("hub.sidebar.open_config_file_path", path = path)))
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
            Pressable::new("open-config")
                .label(t!("hub.sidebar.open_config_file"))
                .cursor_pointer()
                .on_click(move |_, _, cx| model.read(cx).reveal_config_file())
                .child(div().font_family(sayso_ui::fonts::UI).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(c.accent).child(t!("hub.sidebar.open_config_file"))),
        )
        .child(text::mono(path, 11., c.graphite).font_weight(FontWeight::NORMAL));
    div().flex().items_end().child(file.flex_1().min_w_0()).children(toggle).into_any_element()
}
