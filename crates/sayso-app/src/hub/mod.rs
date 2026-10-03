//! The Hub window: sidebar plus one page at a time (plan §3, "Hub").
//!
//! Settings takes over the sidebar: the sidebar shows "‹ Settings", the
//! settings sections, and the config file link.

pub mod pages;
pub mod settings;
mod sidebar;

use crate::model::AppModel;
use gpui_kit::*;
use sayso_ui::ActivePaper;
use sayso_ui::texture::{Grain, grain};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Route {
    Home,
    History,
    Dictionary,
    Styles,
    Models,
    Settings(SettingsPage),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    General,
    Dictation,
    Overlay,
    Audio,
    Appearance,
    HistoryPrivacy,
    Permissions,
    Advanced,
}

impl SettingsPage {
    pub const ALL: [SettingsPage; 8] = [
        SettingsPage::General,
        SettingsPage::Dictation,
        SettingsPage::Overlay,
        SettingsPage::Audio,
        SettingsPage::Appearance,
        SettingsPage::HistoryPrivacy,
        SettingsPage::Permissions,
        SettingsPage::Advanced,
    ];
}

pub struct HubView {
    model: Entity<AppModel>,
    pages: pages::Pages,
    _observe: Subscription,
}

impl HubView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pages = pages::Pages::new(model.clone(), window, cx);
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, pages, _observe: observe }
    }
}

impl Render for HubView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let route = self.model.read(cx).route;
        let page = self.pages.view(route, window, cx);
        div()
            .relative()
            .size_full()
            .flex()
            .flex_row()
            .bg(c.ground)
            .font_family(sayso_ui::fonts::UI)
            .text_color(c.ink)
            .child(grain(Grain::Board, px(0.), cx))
            .child(sidebar::sidebar(&self.model, route, cx))
            .child(
                // The content sheet, with two sheets peeking out underneath.
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pt(px(crate::caption::top_margin(10.)))
                    .pr(px(10.))
                    .pb(px(10.))
                    .child(
                        div()
                            .absolute()
                            .top(px(16.))
                            .right(px(6.))
                            .bottom(px(4.))
                            .left(px(6.))
                            .rounded(px(14.))
                            .bg(c.sheet.blend(c.ground.opacity(0.45)))
                            .shadow(vec![BoxShadow::new(px(0.), px(1.), c.shadow(0.12)).blur_radius(px(2.))]),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(13.))
                            .right(px(8.))
                            .bottom(px(6.))
                            .left(px(2.))
                            .rounded(px(14.))
                            .bg(c.sheet.blend(c.ground.opacity(0.25)))
                            .shadow(vec![BoxShadow::new(px(0.), px(1.), c.shadow(0.12)).blur_radius(px(2.))]),
                    )
                    .child(
                        div()
                            .relative()
                            .size_full()
                            .rounded(px(14.))
                            .bg(c.sheet)
                            .shadow(sayso_ui::paper::sheet(&c))
                            .overflow_hidden()
                            .child(grain(Grain::Sheet, px(14.), cx))
                            .child(div().relative().size_full().child(page)),
                    ),
            )
            .children(crate::caption::caption_bar(true, cx))
    }
}
