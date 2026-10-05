//! Window size limits, and the widths at which the Hub and onboarding change
//! their layout.
//!
//! A window manager can ignore the minimum size of a window: a tiling window
//! manager (Hyprland, sway) gives a tiled window the size of its tile. So the
//! minimum size is not enough. Each window also has a floor ([`floor`]): below
//! it the content keeps its size and the window scrolls.

use gpui_kit::*;
use sayso_core::config::Config;

/// The smallest Hub. The Hub shows the rail and the compact pages at this size.
pub const HUB_MIN: (f32, f32) = (760., 560.);
/// The size of the onboarding window, which is also its minimum size.
pub const ONBOARDING_SIZE: (f32, f32) = (960., 640.);
/// The smallest onboarding content, for a window manager that ignores the
/// minimum size.
pub const ONBOARDING_FLOOR: (f32, f32) = (600., 480.);

/// Below this window width the Hub always shows the rail: the pages have no
/// room for the sidebar.
const RAIL_BELOW: f32 = 920.;
/// Below this window width onboarding puts its columns in one column.
const ONBOARDING_NARROW_BELOW: f32 = 860.;

/// The width of the Hub sidebar.
pub const SIDEBAR_W: f32 = 232.;
/// The width of the rail. The traffic lights on macOS end at 78.
pub const RAIL_W: f32 = 88.;

pub fn min_size(min: (f32, f32)) -> Size<Pixels> {
    size(px(min.0), px(min.1))
}

/// True when the window is too narrow for the sidebar, so the user has no choice.
pub fn rail_forced(window: &Window) -> bool {
    window.viewport_size().width < px(RAIL_BELOW)
}

/// True when the Hub shows the rail in place of the sidebar: the user
/// collapsed the sidebar, or the window is narrow.
pub fn rail(window: &Window, config: &Config) -> bool {
    config.appearance.sidebar_collapsed || rail_forced(window)
}

/// The width of a Hub page: the window without the sidebar and the margin.
pub fn page_width(window: &Window, config: &Config) -> f32 {
    let side = if rail(window, config) { RAIL_W } else { SIDEBAR_W };
    (f32::from(window.viewport_size().width).max(HUB_MIN.0) - side - 10.).max(0.)
}

/// True when a Hub page has no room for its columns side by side.
pub fn page_narrow(window: &Window, config: &Config) -> bool {
    page_width(window, config) < 820.
}

/// The side padding of a Hub page: 52 as designed, 32 on a narrow page.
pub fn page_pad(window: &Window, config: &Config) -> f32 {
    if page_narrow(window, config) { 32. } else { 52. }
}

/// True when onboarding has no room for two columns.
pub fn onboarding_narrow(window: &Window) -> bool {
    window.viewport_size().width < px(ONBOARDING_NARROW_BELOW)
}

/// Keep `content` at `min` or larger. In a smaller window the content does
/// not shrink: the window scrolls.
pub fn floor(id: &'static str, min: (f32, f32), content: Div) -> Stateful<Div> {
    div().id(id).size_full().overflow_scroll().child(content.size_full().min_w(px(min.0)).min_h(px(min.1)))
}
