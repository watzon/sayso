//! Where the pill sits, per display, with soft snapping (plan §3, "Overlay and pill").
//!
//! Stored in `state.json` as the card center's offset from the display's
//! horizontal center and its distance from the bottom of the visible frame.

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
struct Spot {
    /// Card center minus visible-frame center, in points.
    dx: f64,
    /// Card bottom above the visible-frame bottom, in points.
    bottom: f64,
}

impl Default for Spot {
    fn default() -> Self {
        Spot { dx: 0.0, bottom: DEFAULT_BOTTOM }
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

    /// The window origin (GPUI coordinates) for the primary display.
    pub fn window_origin(&self, _cx: &App) -> Point<Pixels> {
        let screens = mac::screens();
        let Some(screen) = screens.first() else { return point(px(0.), px(0.)) };
        let spot = self.read().pill.get(&screen.id.to_string()).copied().unwrap_or_default();
        origin_for(screen.visible_frame, spot)
    }

    /// The display that holds the card of the window (GPUI coordinates).
    pub fn display_of(&self, screens: &[mac::ScreenInfo], window: Bounds<Pixels>) -> Option<u32> {
        let h = mac::primary_screen_height();
        let center_x = (window.origin.x.as_f32() + WIDTH / 2.) as f64;
        let bottom = h - (window.origin.y.as_f32() + HEIGHT) as f64 + MARGIN as f64;
        display_at(screens, center_x, bottom)
    }

    /// The window origin (GPUI coordinates) for the pill's spot on a display.
    pub fn origin_on(&self, screens: &[mac::ScreenInfo], display: u32) -> Option<Point<Pixels>> {
        let screen = screens.iter().find(|s| s.id == display)?;
        let spot = self.read().pill.get(&display.to_string()).copied().unwrap_or_default();
        Some(origin_for(screen.visible_frame, spot))
    }

    /// Snap the card after a drag, save the spot, and return the window origin to use.
    pub fn snap_and_save(&self, window: Bounds<Pixels>, _cx: &App) -> Point<Pixels> {
        let h = mac::primary_screen_height();
        // Card center x and bottom, in Cocoa coordinates.
        let center_x = (window.origin.x.as_f32() + WIDTH / 2.) as f64;
        let bottom = h - (window.origin.y.as_f32() + HEIGHT) as f64 + MARGIN as f64;
        let Some(screen) = screen_for(center_x, bottom) else { return window.origin };
        let vf = screen.visible_frame;
        let mut dx = center_x - (vf.x + vf.width / 2.0);
        let mut from_bottom = bottom - vf.y;
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
        let top_limit = vf.height - 40.0 - EDGE;
        if from_bottom < DEFAULT_BOTTOM + SNAP {
            from_bottom = DEFAULT_BOTTOM;
        }
        if from_bottom > top_limit - SNAP {
            from_bottom = top_limit;
        }
        let spot = Spot { dx, bottom: from_bottom };
        let mut state = self.read();
        state.pill.insert(screen.id.to_string(), spot);
        if let Ok(text) = serde_json::to_string_pretty(&state) {
            let _ = std::fs::write(&self.path, text);
        }
        origin_for(vf, spot)
    }
}

fn origin_for(vf: Rect, spot: Spot) -> Point<Pixels> {
    let h = mac::primary_screen_height();
    let center_x = vf.x + vf.width / 2.0 + spot.dx;
    let card_bottom = vf.y + spot.bottom;
    let window_cocoa_y = card_bottom - MARGIN as f64;
    let x = center_x - (WIDTH / 2.) as f64;
    let y = h - (window_cocoa_y + HEIGHT as f64);
    point(px(x as f32), px(y as f32))
}

#[cfg(test)]
mod tests {
    use super::Follow;

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
