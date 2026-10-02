//! The Sayso design system: tactile paper and ink on GPUI.
//!
//! - [`theme`]: the palette as a GPUI global, synced into gpui-kit.
//! - [`paper`]: material shadows (raised sheets, debossed wells, ink buttons).
//! - [`texture`]: tiled rag-paper grain.
//! - [`text`]: the type scale (Fraunces, Instrument Sans, IBM Plex Mono).
//! - [`components`]: controls built from those pieces.
//!
//! The values come from the Paper file "Sayso — v0.1 Design".

pub mod assets;
pub mod components;
pub mod fonts;
pub mod paper;
pub mod text;
pub mod texture;
pub mod theme;

pub use theme::{ActivePaper, Colors, PaperTheme};

/// Register fonts, assets, and the default theme. Call once at startup, after `gpui_kit::init`.
pub fn init(cx: &mut gpui_kit::App) {
    fonts::register(cx);
    texture::init(cx);
    theme::init(cx);
}
