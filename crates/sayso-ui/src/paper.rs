//! The paper material: raised sheets, debossed wells, keycaps, ink buttons.
//!
//! Each function returns the shadow stack from the design file. Apply it with
//! `.shadow(...)` or use the [`PaperStyled`] helpers.

use crate::theme::Colors;
use gpui_kit::*;

fn s(x: f32, y: f32, blur: f32, color: Hsla) -> BoxShadow {
    BoxShadow::new(px(x), px(y), color).blur_radius(px(blur))
}

fn inset(x: f32, y: f32, blur: f32, color: Hsla) -> BoxShadow {
    s(x, y, blur, color).inset()
}

/// A card or row lifted off the page.
pub fn raised(c: &Colors) -> Vec<BoxShadow> {
    vec![inset(0., 1., 0., c.highlight(1.0)), s(0., 1., 2., c.shadow(0.14)), s(0., 8., 20., c.shadow(0.08))]
}

/// A floating sheet: the overlay pill, popovers.
pub fn floating(c: &Colors) -> Vec<BoxShadow> {
    vec![
        inset(0., 1., 0., c.highlight(1.0)),
        inset(0., -1., 0., c.shadow(0.10)),
        s(0., 1., 2., c.shadow(0.14)),
        s(0., 12., 32., c.shadow(0.16)),
    ]
}

/// The main content sheet in a window.
pub fn sheet(c: &Colors) -> Vec<BoxShadow> {
    vec![
        inset(0., 1., 0., c.highlight(0.95)),
        BoxShadow::new(px(0.), px(0.), c.shadow(0.08)).spread_radius(px(1.)),
        s(0., 1., 2., c.shadow(0.10)),
        s(0., 6., 20., c.shadow(0.10)),
    ]
}

/// A small raised control: chips, secondary buttons.
pub fn raised_small(c: &Colors) -> Vec<BoxShadow> {
    vec![inset(0., 1., 0., c.highlight(1.0)), inset(0., -1., 0., c.shadow(0.18)), s(0., 1., 1.5, c.shadow(0.16))]
}

/// A keycap: a deeper bottom edge.
pub fn keycap(c: &Colors) -> Vec<BoxShadow> {
    vec![inset(0., 1., 0., c.highlight(1.0)), inset(0., -1.5, 0., c.shadow(0.22)), s(0., 1., 1.5, c.shadow(0.18))]
}

/// A pressed impression: fields, wells, tracks, the active nav item.
pub fn debossed(c: &Colors) -> Vec<BoxShadow> {
    vec![inset(0., 1., 3., c.shadow(0.22)), inset(0., -1., 0., c.highlight(0.75))]
}

/// A large shallow well, like the stage behind the overlay.
pub fn well(c: &Colors) -> Vec<BoxShadow> {
    vec![inset(0., 2., 5., c.shadow(0.14)), inset(0., -1., 0., c.highlight(0.6))]
}

/// A primary ink button.
pub fn ink_button(c: &Colors) -> Vec<BoxShadow> {
    vec![
        inset(0., 1., 0., gpui_kit::white().opacity(0.12)),
        inset(0., -2., 3., gpui_kit::black().opacity(0.4)),
        s(0., 1., 2., c.shadow(0.3)),
    ]
}

/// An ink button pushed into the page (pressed keycap in the hotkey test).
pub fn ink_pressed(c: &Colors) -> Vec<BoxShadow> {
    vec![inset(0., 2., 4., gpui_kit::black().opacity(0.5)), s(0., 1., 0., c.highlight(0.7))]
}

/// The indigo seal: a domed stamp.
pub fn seal(_c: &Colors) -> Vec<BoxShadow> {
    vec![
        inset(0., 1., 0., gpui_kit::white().opacity(0.25)),
        inset(0., -2., 3., hsla(0.64, 0.6, 0.12, 0.35)),
        s(0., 1., 2., hsla(0.64, 0.5, 0.15, 0.3)),
    ]
}

pub trait PaperStyled: Styled + Sized {
    fn raised(self, c: &Colors) -> Self {
        self.bg(c.sheet_raised).shadow(raised(c))
    }
    fn raised_small(self, c: &Colors) -> Self {
        self.bg(c.sheet_raised).shadow(raised_small(c))
    }
    fn floating(self, c: &Colors) -> Self {
        self.bg(c.sheet_raised).shadow(floating(c))
    }
    fn debossed(self, c: &Colors) -> Self {
        self.bg(c.deboss).shadow(debossed(c))
    }
    fn well(self, c: &Colors) -> Self {
        self.bg(c.deboss).shadow(well(c))
    }
    fn keycap(self, c: &Colors) -> Self {
        self.bg(c.sheet).shadow(keycap(c))
    }
    fn ink_button(self, c: &Colors) -> Self {
        self.bg(c.ink_fill).shadow(ink_button(c))
    }
}

impl<T: Styled> PaperStyled for T {}
