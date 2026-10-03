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

fn screen_for(cocoa_x: f64, cocoa_y: f64) -> Option<mac::ScreenInfo> {
    let screens = mac::screens();
    screens
        .iter()
        .find(|s| {
            let f = s.frame;
            cocoa_x >= f.x && cocoa_x <= f.x + f.width && cocoa_y >= f.y && cocoa_y <= f.y + f.height
        })
        .or(screens.first())
        .copied()
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
