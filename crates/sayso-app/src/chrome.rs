//! The custom title bar on Linux (Settings › Appearance, "Use the system
//! title bar" off). Sayso then draws its own title area and window buttons
//! (client-side decorations), and the paper goes up to the top edge of the
//! window, as it does behind the transparent title bar on macOS. The kit's
//! root window adds the shadow, the border, and the resize edges.
//!
//! On macOS, with the system title bar, and when the window manager keeps its
//! own decorations (an X11 desktop without a compositor), the functions here
//! do nothing.

use gpui_kit::*;
use sayso_core::config::Config;
use sayso_ui::ActivePaper;
use sayso_ui::assets::Icon;
use sayso_ui::components::icon;
use sayso_ui::paper::PaperStyled;

/// The decorations of the setting. None leaves the choice to GPUI (macOS).
pub fn decorations(config: &Config) -> Option<WindowDecorations> {
    cfg!(target_os = "linux").then_some(if config.appearance.system_title_bar {
        WindowDecorations::Server
    } else {
        WindowDecorations::Client
    })
}

/// The window background to ask for: clear on Linux, so the kit's shadow can
/// fade out around a window with the custom title bar.
pub fn background() -> WindowBackgroundAppearance {
    if cfg!(target_os = "linux") { WindowBackgroundAppearance::Transparent } else { WindowBackgroundAppearance::Opaque }
}

/// Apply a change of the setting to an open window.
pub fn apply(config: &Config, window: &mut Window) {
    if let Some(decorations) = decorations(config) {
        window.request_decorations(decorations);
        window.refresh();
    }
}

/// True when Sayso draws the title area of this window.
pub fn is_custom(window: &Window) -> bool {
    cfg!(target_os = "linux") && matches!(window.window_decorations(), Decorations::Client { .. })
}

/// Make `el` move the window when dragged: a press starts the move, a double
/// click maximizes, and a right click opens the window menu.
pub fn drag_area(el: Stateful<Div>) -> Stateful<Div> {
    el.on_mouse_down(MouseButton::Left, |e, window, _| {
        if e.click_count == 2 {
            window.zoom_window();
        } else {
            window.start_window_move();
        }
    })
    .on_mouse_down(MouseButton::Right, |e, window, _| window.show_window_menu(e.position))
}

/// Minimize, maximize, and close, as small paper buttons. Only the buttons
/// the window manager supports show. `on_close` runs for the close button.
pub fn window_controls(window: &Window, cx: &App, on_close: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    let c = cx.paper().colors;
    let supported = window.window_controls();
    let button = |id: &'static str| {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(24.))
            .rounded_full()
            .cursor_pointer()
            .hover(|s| s.bg(c.sheet_raised))
            .active(|s| s.opacity(0.7))
            // A press on a button must not start a window move.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    };
    let ink = c.graphite;
    let mut row = div().flex().flex_none().items_center().gap(px(6.));
    if supported.minimize {
        row = row.child(
            button("window-minimize")
                .on_click(|_, window, _| window.minimize_window())
                .child(div().w(px(9.)).h(px(1.5)).mt(px(5.)).rounded_full().bg(ink)),
        );
    }
    if supported.maximize {
        row = row.child(
            button("window-maximize")
                .on_click(|_, window, _| window.zoom_window())
                .child(div().size(px(9.)).rounded(px(2.)).border_1().border_color(ink)),
        );
    }
    row.child(
        button("window-close")
            .debossed(&c)
            .on_click(move |_, window, cx| on_close(window, cx))
            .child(icon(Icon::Close, 11., c.ink)),
    )
    .into_any_element()
}
