//! Where the pill sits, per display, with soft snapping (plan §3, "Overlay and pill").
//!
//! Stored in `state.json` as the card center's offset from the display's
//! horizontal center and its distance from the bottom or the top of the
//! visible frame.
//!
//! The card sits at the bottom of the window in the bottom half of a display
//! and at the top of the window in the top half (see [`Anchor`]), so the
//! window never goes past the edge that the pill is near.

use super::{HEIGHT, MARGIN, WIDTH};
use gpui_kit::*;
use sayso_core::paths::Paths;
use crate::shell::{self as mac, Rect};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// The default gap between the pill and the Dock.
const DEFAULT_BOTTOM: f64 = 16.0;
/// Snap distance in points.
const SNAP: f64 = 48.0;
const EDGE: f64 = 16.0;

/// The edge of the window that holds the card. The rest of the overlay (the
/// live preview) grows away from it, into the display.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    #[default]
    Bottom,
    Top,
}

impl Anchor {
    /// The top of a card of this height, in window coordinates.
    fn card_top(self, card_height: f64) -> f64 {
        match self {
            Anchor::Top => MARGIN as f64,
            Anchor::Bottom => (HEIGHT - MARGIN) as f64 - card_height,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
struct Spot {
    /// Card center minus visible-frame center, in points.
    dx: f64,
    /// Card bottom above the visible-frame bottom, in points.
    bottom: f64,
    /// Card top below the visible-frame top, in points, for a pill in the
    /// top half of the display. `bottom` stays in the file for a version
    /// that does not know this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    top: Option<f64>,
}

impl Default for Spot {
    fn default() -> Self {
        Spot { dx: 0.0, bottom: DEFAULT_BOTTOM, top: None }
    }
}

impl Spot {
    /// The distance from the top of the visible frame, for a pill in the top
    /// half of the display. A spot from a version that had no `top` gets one.
    fn top_gap(self, vf: Rect) -> Option<f64> {
        self.top.or_else(|| (self.bottom > vf.height / 2.0).then(|| snap_to_edge(vf.height - self.bottom - IDLE_CARD, EDGE)))
    }
}

/// The height of the idle pill at the default size, for a spot that has no `top`.
const IDLE_CARD: f64 = 14.0;

/// The gap to an edge of the visible frame after a drag: `rest` when the card is near the edge.
fn snap_to_edge(gap: f64, rest: f64) -> f64 {
    if gap < rest + SNAP { rest } else { gap }
}

/// The window top and the anchor for a card that a drag wants at `card_top`.
/// `top` and `bottom` are the edges of the visible frame. All values are
/// global points that grow down.
///
/// The window stays in the visible frame on the side of the card. macOS
/// moves a window whose top goes into the menu bar, and that fights the drag.
fn dragged_window_top(top: f64, bottom: f64, card_top: f64, card_height: f64) -> (f64, Anchor) {
    let anchor = if card_top + card_height / 2.0 < (top + bottom) / 2.0 { Anchor::Top } else { Anchor::Bottom };
    let window_top = card_top - anchor.card_top(card_height);
    let window_top = match anchor {
        Anchor::Top => window_top.max(top),
        Anchor::Bottom => window_top.min(bottom - HEIGHT as f64),
    };
    (window_top, anchor)
}

/// The spot for a card after a drag, with the snaps. `center_x`, `card_top`,
/// and `card_bottom` are Cocoa coordinates.
fn snapped(vf: Rect, anchor: Anchor, center_x: f64, card_top: f64, card_bottom: f64) -> Spot {
    let mut dx = center_x - (vf.x + vf.width / 2.0);
    // Centers.
    if dx.abs() < SNAP {
        dx = 0.0;
    }
    // Edges: keep the pill a little inside the visible frame.
    let half_card = 30.0;
    let left_limit = -vf.width / 2.0 + half_card + EDGE;
    let right_limit = vf.width / 2.0 - half_card - EDGE;
    if dx < left_limit + SNAP {
        dx = left_limit;
    }
    if dx > right_limit - SNAP {
        dx = right_limit;
    }
    match anchor {
        Anchor::Bottom => Spot { dx, bottom: snap_to_edge(card_bottom - vf.y, DEFAULT_BOTTOM), top: None },
        Anchor::Top => Spot { dx, bottom: card_bottom - vf.y, top: Some(snap_to_edge(vf.y + vf.height - card_top, EDGE)) },
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StateFile {
    #[serde(default)]
    pill: HashMap<String, Spot>,
    #[serde(flatten)]
    other: serde_json::Map<String, serde_json::Value>,
}

pub struct Placement {
    path: PathBuf,
}

pub fn main_screen_height() -> f32 {
    mac::primary_screen_height() as f32
}

fn contains(frame: Rect, x: f64, y: f64) -> bool {
    x >= frame.x && x <= frame.x + frame.width && y >= frame.y && y <= frame.y + frame.height
}

fn screen_for(cocoa_x: f64, cocoa_y: f64) -> Option<mac::ScreenInfo> {
    let screens = mac::screens();
    screens.iter().find(|s| contains(s.frame, cocoa_x, cocoa_y)).or(screens.first()).copied()
}

/// The id of the display that holds a point, in Cocoa coordinates.
pub fn display_at(screens: &[mac::ScreenInfo], x: f64, y: f64) -> Option<u32> {
    screens.iter().find(|s| contains(s.frame, x, y)).map(|s| s.id)
}

/// The display that the overlay follows: the display of the mouse or of the
/// focused window, whichever went to another display last.
#[derive(Debug, Default)]
pub struct Follow {
    mouse: Option<u32>,
    focus: Option<u32>,
}

impl Follow {
    /// The display to move to, when the mouse or the focus went to another
    /// display since the last call. None means that a display is not known.
    /// When both moved, the focus wins: the text goes there.
    pub fn target(&mut self, mouse: Option<u32>, focus: Option<u32>) -> Option<u32> {
        let mouse_moved = mouse.is_some() && mouse != self.mouse;
        let focus_moved = focus.is_some() && focus != self.focus;
        self.mouse = mouse.or(self.mouse);
        self.focus = focus.or(self.focus);
        if focus_moved {
            focus
        } else if mouse_moved {
            mouse
        } else {
            None
        }
    }
}

impl Placement {
    pub fn load(paths: &Paths) -> Self {
        Placement { path: paths.state_file() }
    }

    fn read(&self) -> StateFile {
        std::fs::read_to_string(&self.path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// The window origin (GPUI coordinates) and the anchor for the primary display.
    pub fn window_origin(&self, _cx: &App) -> (Point<Pixels>, Anchor) {
        let screens = mac::screens();
        let Some(screen) = screens.first() else { return (point(px(0.), px(0.)), Anchor::default()) };
        let spot = self.read().pill.get(&screen.id.to_string()).copied().unwrap_or_default();
        origin_for(screen.visible_frame, spot)
    }

    /// The display that holds the card of the window (GPUI coordinates).
    pub fn display_of(&self, screens: &[mac::ScreenInfo], window: Bounds<Pixels>, anchor: Anchor) -> Option<u32> {
        let h = mac::primary_screen_height();
        let center_x = (window.origin.x.as_f32() + WIDTH / 2.) as f64;
        // The edge of the card that does not move when the card grows.
        let edge = match anchor {
            Anchor::Bottom => HEIGHT - MARGIN,
            Anchor::Top => MARGIN,
        };
        display_at(screens, center_x, h - (window.origin.y.as_f32() + edge) as f64)
    }

    /// The window origin (GPUI coordinates) and the anchor for the pill's spot on a display.
    pub fn origin_on(&self, screens: &[mac::ScreenInfo], display: u32) -> Option<(Point<Pixels>, Anchor)> {
        let screen = screens.iter().find(|s| s.id == display)?;
        let spot = self.read().pill.get(&display.to_string()).copied().unwrap_or_default();
        Some(origin_for(screen.visible_frame, spot))
    }

    /// The window origin and the anchor during a drag. `card` is the left of
    /// the window and the top of the card that the mouse asks for (GPUI
    /// coordinates). The display of the mouse decides the anchor, so the
    /// pill can go to a display above or below.
    pub fn drag(&self, card: Point<Pixels>, card_height: Pixels, mouse: mac::Point) -> (Point<Pixels>, Anchor) {
        let h = mac::primary_screen_height();
        let (card_top, card_height) = (card.y.as_f32() as f64, card_height.as_f32() as f64);
        let (window_top, anchor) = match screen_for(mouse.x, mouse.y) {
            Some(screen) => {
                let vf = screen.visible_frame;
                dragged_window_top(h - (vf.y + vf.height), h - vf.y, card_top, card_height)
            }
            None => (card_top - Anchor::Bottom.card_top(card_height), Anchor::Bottom),
        };
        (point(card.x, px(window_top as f32)), anchor)
    }

    /// The top of the card of a window (GPUI coordinates), where a drag starts.
    pub fn card_top(window: Point<Pixels>, anchor: Anchor, card_height: Pixels) -> Pixels {
        window.y + px(anchor.card_top(card_height.as_f32() as f64) as f32)
    }

    /// Snap the card after a drag, save the spot, and return the window origin and the anchor to use.
    pub fn snap_and_save(&self, window: Point<Pixels>, anchor: Anchor, card_height: Pixels, _cx: &App) -> (Point<Pixels>, Anchor) {
        let h = mac::primary_screen_height();
        // The card in Cocoa coordinates.
        let card_height = card_height.as_f32() as f64;
        let center_x = (window.x.as_f32() + WIDTH / 2.) as f64;
        let card_top = h - (window.y.as_f32() as f64 + anchor.card_top(card_height));
        let card_bottom = card_top - card_height;
        let Some(screen) = screen_for(center_x, (card_top + card_bottom) / 2.0) else { return (window, anchor) };
        let spot = snapped(screen.visible_frame, anchor, center_x, card_top, card_bottom);
        let mut state = self.read();
        state.pill.insert(screen.id.to_string(), spot);
        if let Ok(text) = serde_json::to_string_pretty(&state) {
            let _ = std::fs::write(&self.path, text);
        }
        origin_for(screen.visible_frame, spot)
    }
}

fn origin_for(vf: Rect, spot: Spot) -> (Point<Pixels>, Anchor) {
    let h = mac::primary_screen_height();
    let center_x = vf.x + vf.width / 2.0 + spot.dx;
    let (anchor, window_top) = match spot.top_gap(vf) {
        Some(top) => (Anchor::Top, vf.y + vf.height - top + MARGIN as f64),
        None => (Anchor::Bottom, vf.y + spot.bottom - MARGIN as f64 + HEIGHT as f64),
    };
    let x = center_x - (WIDTH / 2.) as f64;
    (point(px(x as f32), px((h - window_top) as f32)), anchor)
}

#[cfg(test)]
mod tests {
    use super::{Anchor, DEFAULT_BOTTOM, EDGE, Follow, HEIGHT, MARGIN, Rect, Spot, dragged_window_top, snapped};

    /// A display of 1440 by 900 with a menu bar of 30 and no Dock.
    const VF: Rect = Rect { x: 0.0, y: 0.0, width: 1440.0, height: 870.0 };

    #[test]
    fn a_drag_puts_the_card_at_the_window_edge_that_is_nearer_to_the_display_edge() {
        // The visible frame is 30..900 in points that grow down. The card is 40 high.
        let (window_top, anchor) = dragged_window_top(30.0, 900.0, 700.0, 40.0);
        assert_eq!(anchor, Anchor::Bottom);
        assert_eq!(window_top + (HEIGHT - MARGIN) as f64, 740.0, "the card bottom is at the window bottom");

        let (window_top, anchor) = dragged_window_top(30.0, 900.0, 200.0, 40.0);
        assert_eq!(anchor, Anchor::Top);
        assert_eq!(window_top + MARGIN as f64, 200.0, "the card top is at the window top");
    }

    #[test]
    fn a_drag_keeps_the_card_in_place_when_the_anchor_changes() {
        let card_top = |y: f64| {
            let (window_top, anchor) = dragged_window_top(30.0, 900.0, y, 40.0);
            window_top + anchor.card_top(40.0)
        };
        // The center of the visible frame is 465, so a card top of 445 is on the line.
        assert_eq!(card_top(444.0), 444.0);
        assert_eq!(card_top(446.0), 446.0);
    }

    #[test]
    fn a_drag_keeps_the_window_in_the_visible_frame() {
        // Into the menu bar: the window top stops at the top of the visible frame.
        assert_eq!(dragged_window_top(30.0, 900.0, 10.0, 40.0), (30.0, Anchor::Top));
        // Over the Dock: the window bottom stops at the bottom of the visible frame.
        assert_eq!(dragged_window_top(30.0, 900.0, 890.0, 40.0), (900.0 - HEIGHT as f64, Anchor::Bottom));
    }

    #[test]
    fn a_drop_snaps_to_the_top_and_to_the_bottom() {
        // Card top 40 below the top of the visible frame, a little off center.
        let spot = snapped(VF, Anchor::Top, 730.0, 830.0, 790.0);
        assert_eq!((spot.dx, spot.top), (0.0, Some(EDGE)));
        // Too far from the top to snap.
        assert_eq!(snapped(VF, Anchor::Top, 730.0, 700.0, 660.0).top, Some(170.0));
        // Card bottom 40 above the bottom.
        let spot = snapped(VF, Anchor::Bottom, 400.0, 80.0, 40.0);
        assert_eq!((spot.dx, spot.bottom, spot.top), (-320.0, DEFAULT_BOTTOM, None));
    }

    #[test]
    fn a_spot_without_a_top_goes_to_the_top_when_it_is_in_the_top_half() {
        // The top snap of a version that had only `bottom`.
        let old: Spot = serde_json::from_str(r#"{ "dx": 0.0, "bottom": 814.0 }"#).unwrap();
        assert_eq!(old.top_gap(VF), Some(EDGE));
        let low: Spot = serde_json::from_str(r#"{ "dx": 0.0, "bottom": 16.0 }"#).unwrap();
        assert_eq!(low.top_gap(VF), None);
    }

    #[test]
    fn follow_goes_to_the_display_that_changed_last() {
        let mut follow = Follow::default();
        assert_eq!(follow.target(Some(1), Some(2)), Some(2), "at the start the focus wins");
        assert_eq!(follow.target(Some(1), Some(2)), None, "nothing moved");
        assert_eq!(follow.target(Some(2), Some(2)), Some(2), "the mouse moved");
        assert_eq!(follow.target(Some(2), Some(1)), Some(1), "the focus moved");
        assert_eq!(follow.target(Some(1), Some(2)), Some(2), "both moved: the focus wins");
    }

    #[test]
    fn follow_ignores_a_display_that_is_not_known() {
        let mut follow = Follow::default();
        assert_eq!(follow.target(Some(1), None), Some(1), "no focused window: the mouse decides");
        assert_eq!(follow.target(None, None), None);
        assert_eq!(follow.target(Some(1), None), None, "the mouse is back on the same display");
        assert_eq!(follow.target(Some(1), Some(1)), Some(1), "the first known focus counts as a move");
    }
}
