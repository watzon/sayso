//! Tiled rag-paper grain (`assets/textures`, made by `generate.py`).
//!
//! The board (window background, sidebar) carries the visible grain. Content
//! sheets get a much softer grain so text sits on a nearly clean page.

use crate::ActivePaper;
use gpui_kit::*;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grain {
    Board,
    Sheet,
}

struct Textures {
    board_light: Option<Arc<RenderImage>>,
    board_dark: Option<Arc<RenderImage>>,
    sheet_light: Option<Arc<RenderImage>>,
    sheet_dark: Option<Arc<RenderImage>>,
}

impl Global for Textures {}

/// Logical size of one tile.
const TILE: f32 = 512.0;

fn decode_rgba(path: &str) -> Option<image::RgbaImage> {
    let bytes = crate::assets::bytes(path)?;
    Some(image::load_from_memory(&bytes).ok()?.into_rgba8())
}

fn decode(path: &str) -> Option<Arc<RenderImage>> {
    let mut img = decode_rgba(path)?;
    // GPUI sprites are BGRA.
    for px in img.pixels_mut() {
        px.0.swap(0, 2);
    }
    let frame = image::Frame::new(img);
    Some(Arc::new(RenderImage::new([frame])))
}

pub(crate) fn init(cx: &mut App) {
    cx.set_global(Textures {
        board_light: decode("textures/board-light.png"),
        board_dark: decode("textures/board-dark.png"),
        sheet_light: decode("textures/sheet-light.png"),
        sheet_dark: decode("textures/sheet-dark.png"),
    });
}

/// An absolutely positioned layer that fills its parent with grain.
/// Put it first among the children of a `relative()` container.
pub fn grain(kind: Grain, radius: Pixels, cx: &App) -> AnyElement {
    let theme = cx.paper();
    if !theme.texture {
        return div().into_any_element();
    }
    let t = cx.global::<Textures>();
    let image = match (kind, theme.is_dark()) {
        (Grain::Board, false) => t.board_light.clone(),
        (Grain::Board, true) => t.board_dark.clone(),
        (Grain::Sheet, false) => t.sheet_light.clone(),
        (Grain::Sheet, true) => t.sheet_dark.clone(),
    };
    let Some(image) = image else { return div().into_any_element() };
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let tile = px(TILE);
            let mut y = bounds.origin.y;
            while y < bounds.origin.y + bounds.size.height {
                let mut x = bounds.origin.x;
                while x < bounds.origin.x + bounds.size.width {
                    let tile_bounds = Bounds { origin: point(x, y), size: size(tile, tile) };
                    let _ = window.paint_image(bounds, tile_bounds, Corners::all(radius), image.clone(), 0, false);
                    x += tile;
                }
                y += tile;
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: it brings in the `test` macro of gpui.
    use super::{TILE, decode_rgba};

    /// The textures are palette PNGs. A decoder that ignores the tRNS chunk
    /// would paint every pixel opaque.
    #[test]
    fn textures_decode_with_their_alpha() {
        for name in ["board-light", "board-dark", "sheet-light", "sheet-dark"] {
            let img = decode_rgba(&format!("textures/{name}.png")).unwrap_or_else(|| panic!("{name} does not decode"));
            assert_eq!(img.dimensions(), (TILE as u32, TILE as u32), "{name}");
            let alphas = img.pixels().map(|p| p.0[3]);
            assert!(alphas.clone().any(|a| a == 0), "{name} has no clear pixel");
            assert!(alphas.clone().any(|a| a > 0 && a < 255), "{name} has no partial alpha");
            assert!(alphas.max() < Some(255), "{name} has an opaque pixel");
        }
    }
}
