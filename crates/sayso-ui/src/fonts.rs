//! Bundled static fonts (see `assets/fonts/build.py`).

use gpui_kit::App;
use std::borrow::Cow;

/// Display serif, optical size 48. Use at 22 px and up.
pub const DISPLAY: &str = "Fraunces";
/// Reading serif, optical size 16. Use for transcripts and previews.
pub const SERIF: &str = "Fraunces Text";
pub const UI: &str = "Instrument Sans";
pub const MONO: &str = "IBM Plex Mono";

macro_rules! font {
    ($file:literal) => {
        Cow::Borrowed(include_bytes!(concat!("../../../assets/fonts/", $file)) as &'static [u8])
    };
}

pub fn register(cx: &mut App) {
    let fonts = vec![
        font!("Fraunces-Regular.ttf"),
        font!("Fraunces-Medium.ttf"),
        font!("Fraunces-SemiBold.ttf"),
        font!("Fraunces-Bold.ttf"),
        font!("Fraunces-Italic.ttf"),
        font!("FrauncesText-Regular.ttf"),
        font!("FrauncesText-Medium.ttf"),
        font!("FrauncesText-SemiBold.ttf"),
        font!("FrauncesText-Bold.ttf"),
        font!("FrauncesText-Italic.ttf"),
        font!("InstrumentSans-Regular.ttf"),
        font!("InstrumentSans-Medium.ttf"),
        font!("InstrumentSans-SemiBold.ttf"),
        font!("InstrumentSans-Bold.ttf"),
        font!("IBMPlexMono-Regular.ttf"),
        font!("IBMPlexMono-Medium.ttf"),
    ];
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        log::error!("could not register bundled fonts: {e:#}");
    }
}
