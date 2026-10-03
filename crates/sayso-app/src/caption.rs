//! Window controls where Sayso draws its own title bar.
//!
//! On macOS the Hub and onboarding keep the native traffic lights. Elsewhere
//! GPUI hides the whole title bar for `appears_transparent`, so the window
//! needs its own drag area and minimize, maximize, and close buttons. GPUI
//! maps the areas to the native hit-test codes, so Windows still snaps,
//! shows snap layouts on the maximize button, and double-click maximizes.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_ui::ActivePaper;
use sayso_ui::assets::Icon;
use sayso_ui::components::icon;

/// Height of the strip at the top of the window.
pub const HEIGHT: f32 = 36.;

/// The top margin of a window's main sheet: room for the caption bar where
/// Sayso draws one, else `mac` (the space the design has above the sheet).
pub fn top_margin(mac: f32) -> f32 {
    if cfg!(target_os = "macos") { mac } else { HEIGHT }
}
const BUTTON_W: f32 = 46.;
/// The red of a close button under the mouse, as Windows draws it.
const CLOSE_HOVER: u32 = 0xc42b1c;

/// The drag strip and window buttons across the top of the window, above
/// its main sheet (see [`top_margin`]). None on macOS.
pub fn caption_bar(resizable: bool, cx: &App) -> Option<AnyElement> {
    if cfg!(target_os = "macos") {
        return None;
    }
    let c = cx.paper().colors;
    let button = |id: &'static str, area: WindowControlArea, close: bool, glyph: AnyElement| {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(BUTTON_W))
            .h(px(HEIGHT))
            .window_control_area(area)
            .when(close, |d| d.hover(|s| s.bg(rgb(CLOSE_HOVER))))
            .when(!close, |d| d.hover(|s| s.bg(c.deboss)))
            .child(glyph)
    };
    let line = |w: f32, h: f32| div().w(px(w)).h(px(h)).bg(c.graphite).into_any_element();
    let square = div().size(px(10.)).border_1().border_color(c.graphite).into_any_element();
    let buttons = div()
        .flex()
        .flex_none()
        .child(button("caption-min", WindowControlArea::Min, false, line(10., 1.)))
        .when(resizable, |d| d.child(button("caption-max", WindowControlArea::Max, false, square)))
        .child(button("caption-close", WindowControlArea::Close, true, icon(Icon::Close, 12., c.graphite).into_any_element()));
    let drag = div().id("caption-drag").h(px(HEIGHT)).window_control_area(WindowControlArea::Drag);
    Some(div().absolute().top_0().left_0().right_0().h(px(HEIGHT)).flex().child(drag.flex_1()).child(buttons).into_any_element())
}
