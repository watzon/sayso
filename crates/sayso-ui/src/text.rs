//! The type scale from the design file.

use crate::fonts::{DISPLAY, MONO, SERIF, UI};
use crate::theme::Colors;
use gpui_kit::*;

/// A display heading in Fraunces with a letterpress highlight below it.
///
/// GPUI has no text shadow, so the highlight is a second copy of the text,
/// 1 px lower, in the highlight color.
pub fn display(text: impl Into<SharedString>, size: f32, c: &Colors) -> Div {
    let text: SharedString = text.into();
    let base = |color: Hsla| {
        div()
            .font_family(DISPLAY)
            .text_size(px(size))
            .line_height(px((size * 1.08).round()))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color)
            .child(text.clone())
    };
    div()
        .relative()
        .child(base(c.highlight(0.85)).absolute().top(px(1.)).left_0())
        .child(base(c.ink))
}

/// Small caps label: "TODAY", "HOTKEYS".
pub fn caps(text: impl Into<SharedString>, c: &Colors) -> Div {
    let s: SharedString = text.into();
    div()
        .font_family(UI)
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(c.graphite)
        .child(SharedString::from(s.to_uppercase()))
}

/// UI text in Instrument Sans.
pub fn ui(text: impl Into<SharedString>, size: f32, weight: FontWeight, color: Hsla) -> Div {
    div().font_family(UI).text_size(px(size)).font_weight(weight).text_color(color).child(text.into())
}

/// Body text: 15 px, 22 px line height, graphite.
pub fn body(text: impl Into<SharedString>, c: &Colors) -> Div {
    ui(text, 15., FontWeight::NORMAL, c.graphite).line_height(px(22.))
}

/// Reading serif for transcripts.
pub fn serif(text: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div().font_family(SERIF).text_size(px(size)).line_height(px((size * 1.42).round())).text_color(color).child(text.into())
}

pub fn mono(text: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div().font_family(MONO).text_size(px(size)).font_weight(FontWeight::MEDIUM).text_color(color).child(text.into())
}

/// A display serif title smaller than [`display`], without the highlight.
pub fn title(text: impl Into<SharedString>, size: f32, c: &Colors) -> Div {
    div()
        .font_family(DISPLAY)
        .text_size(px(size))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c.ink)
        .child(text.into())
}
