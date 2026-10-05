//! The Hub window: sidebar plus one page at a time (plan §3, "Hub").
//!
//! Settings takes over the sidebar: the sidebar shows "‹ Settings", the
//! settings sections, and the config file link.

pub mod pages;
pub mod settings;
mod sidebar;

use crate::model::AppModel;
use gpui_kit::prelude::FluentBuilder as _;
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
    /// The sidebar width at the last render, and the width it had before
    /// its last change. The change between the two is animated.
    side: Option<(f32, f32)>,
    /// Counts the changes: a new number starts the animation again.
    side_changes: usize,
    _observe: Subscription,
}

impl HubView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pages = pages::Pages::new(model.clone(), window, cx);
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, pages, side: None, side_changes: 0, _observe: observe }
    }
}

impl Render for HubView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let route = self.model.read(cx).route;
        let page = self.pages.view(route, window, cx);
        // The custom title bar (Linux): a strip across the top that moves the
        // window and holds the window buttons. On Windows the caption bar
        // takes the same room. The content sheet starts this far down; the
        // sheets under it peek out 6 and 3 px below its top edge.
        let custom = crate::chrome::is_custom(window);
        let top = if custom { 36. } else { crate::caption::top_margin(10.) };
        let forced = crate::layout::rail_forced(window);
        let rail = crate::layout::rail(window, &self.model.read(cx).config);
        // The sidebar and the rail change places with a short slide.
        let to = if rail { crate::layout::RAIL_W } else { crate::layout::SIDEBAR_W };
        let from = match self.side {
            Some((_, last)) if last != to => {
                self.side_changes += 1;
                last
            }
            Some((from, _)) => from,
            None => to,
        };
        self.side = Some((from, to));
        let from = if cx.paper().reduce_motion { to } else { from };
        let side = div().flex_none().h_full().overflow_hidden().child(sidebar::sidebar(&self.model, route, rail, forced, cx)).with_animation(
            ("sidebar-width", self.side_changes),
            Animation::new(std::time::Duration::from_millis(220)).with_easing(ease_out_quint()),
            move |d, t| d.w(px(from + (to - from) * t)),
        );
        let hub = div()
            .relative()
            .flex()
            .flex_row()
            .bg(c.ground)
            .font_family(sayso_ui::fonts::UI)
            .text_color(c.ink)
            .child(grain(Grain::Board, px(0.), cx))
            .child(side)
            .child(
                // The content sheet, with two sheets peeking out underneath.
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pt(px(top))
                    .pr(px(10.))
                    .pb(px(10.))
                    .child(
                        div()
                            .absolute()
                            .top(px(top + 6.))
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
                            .top(px(top + 3.))
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
            .when(custom, |d| {
                d.child(
                    crate::chrome::drag_area(div().id("hub-title").absolute().top_0().left_0().right_0().h(px(top)))
                        .flex()
                        .items_center()
                        .justify_end()
                        .pr(px(12.))
                        .child(crate::chrome::window_controls(window, cx, |window, cx| {
                            cx.global_mut::<crate::app::Windows>().hub = None;
                            window.remove_window();
                        })),
                )
            })
            .children(crate::caption::caption_bar(true, cx));
        crate::layout::floor("hub-floor", crate::layout::HUB_MIN, hub)
    }
}
