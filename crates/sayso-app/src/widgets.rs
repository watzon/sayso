//! App-level building blocks shared by Hub pages, Settings, and onboarding.

use gpui_kit::component::input::InputState;
use gpui_kit::*;
use sayso_ui::ActivePaper;
use sayso_ui::text;

/// A Hub page title with an optional subtitle and actions on the right.
pub fn page_header(title: &str, subtitle: Option<&str>, actions: Option<AnyElement>, cx: &App) -> Div {
    let c = cx.paper().colors;
    let mut left = div().flex().flex_col().gap(px(8.)).child(text::display(title.to_string(), 44., &c));
    if let Some(s) = subtitle {
        left = left.child(text::body(s.to_string(), &c).max_w(px(560.)));
    }
    let mut row = div().flex().justify_between().items_end().w_full().child(left);
    if let Some(a) = actions {
        row = row.child(div().flex().gap(px(8.)).pb(px(4.)).child(a));
    }
    row
}

/// A section label in small caps.
pub fn section(title: &str, cx: &App) -> Div {
    text::caps(title.to_string(), &cx.paper().colors).pb(px(4.))
}

/// Create a text input state with a placeholder and an initial value.
pub fn input_state(placeholder: &str, value: &str, window: &mut Window, cx: &mut App) -> Entity<InputState> {
    let placeholder: SharedString = placeholder.to_string().into();
    let value: SharedString = value.to_string().into();
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value))
}

