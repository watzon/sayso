//! Bundled static fonts (see `assets/fonts/build.py` in this crate).

use gpui_kit::{App, FontFallbacks, Styled};
use std::borrow::Cow;
use std::sync::LazyLock;

/// Display serif, optical size 48. Use at 22 px and up.
pub const DISPLAY: &str = "Fraunces";
/// Reading serif, optical size 16. Use for transcripts and previews.
pub const SERIF: &str = "Fraunces Text";
pub const UI: &str = "Instrument Sans";
pub const MONO: &str = "IBM Plex Mono";

macro_rules! font {
    ($file:literal) => {
        Cow::Borrowed(include_bytes!(concat!("../assets/fonts/", $file)) as &'static [u8])
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

/// The kind of face that a text uses, for its fallback fonts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// Instrument Sans and IBM Plex Mono.
    Sans,
    /// Fraunces and Fraunces Text.
    Serif,
}

/// The fonts for the characters that the bundled fonts do not have, for a
/// locale. Chinese and Japanese use the same characters with different
/// shapes, so the system must not choose: Japanese text gets a Japanese font
/// and Chinese text a Chinese font. Korean gets Korean fonts for the same
/// reason. The bundled fonts have no Cyrillic, and the system would show
/// Russian serif text in a sans face, so Russian serif text gets Georgia.
/// None leaves the choice to the system, which is right for the Latin
/// locales and for Russian sans text. The Linux text system ignores the list
/// and uses fontconfig.
pub fn fallbacks(locale: &str, face: Face) -> Option<FontFallbacks> {
    static JA_SANS: LazyLock<FontFallbacks> = LazyLock::new(|| list(&["Hiragino Sans", "Yu Gothic UI", "Noto Sans CJK JP"]));
    static JA_SERIF: LazyLock<FontFallbacks> = LazyLock::new(|| list(&["Hiragino Mincho ProN", "Yu Mincho", "Noto Serif CJK JP"]));
    static ZH_SANS: LazyLock<FontFallbacks> = LazyLock::new(|| list(&["PingFang SC", "Microsoft YaHei UI", "Noto Sans CJK SC"]));
    static ZH_SERIF: LazyLock<FontFallbacks> = LazyLock::new(|| list(&["Songti SC", "SimSun", "Noto Serif CJK SC"]));
    static KO_SANS: LazyLock<FontFallbacks> = LazyLock::new(|| list(&["Apple SD Gothic Neo", "Malgun Gothic", "Noto Sans CJK KR"]));
    static KO_SERIF: LazyLock<FontFallbacks> = LazyLock::new(|| list(&["AppleMyungjo", "Batang", "Noto Serif CJK KR"]));
    static RU_SERIF: LazyLock<FontFallbacks> = LazyLock::new(|| list(&["Georgia", "Noto Serif"]));
    let language = locale.split(['-', '_']).next().unwrap_or(locale);
    match (language, face) {
        ("ja", Face::Sans) => Some(JA_SANS.clone()),
        ("ja", Face::Serif) => Some(JA_SERIF.clone()),
        ("zh", Face::Sans) => Some(ZH_SANS.clone()),
        ("zh", Face::Serif) => Some(ZH_SERIF.clone()),
        ("ko", Face::Sans) => Some(KO_SANS.clone()),
        ("ko", Face::Serif) => Some(KO_SERIF.clone()),
        ("ru", Face::Serif) => Some(RU_SERIF.clone()),
        _ => None,
    }
}

fn list(fonts: &[&str]) -> FontFallbacks {
    FontFallbacks::from_fonts(fonts.iter().map(|f| f.to_string()).collect())
}

/// Fallback fonts on an element. Its children inherit them.
pub trait LocaleFonts: Styled + Sized {
    /// The fallbacks of the interface language for a face. Each window root
    /// sets the sans fallbacks, and each serif text sets the serif ones.
    fn locale_fonts(self, face: Face) -> Self {
        self.fonts_for(&crate::l10n::locale(), face)
    }

    /// The fallbacks of a given locale, for text in that language whatever
    /// the interface language is: the names in the Language menu.
    fn fonts_for(mut self, locale: &str, face: Face) -> Self {
        self.text_style().font_fallbacks = fallbacks(locale, face);
        self
    }
}

impl<T: Styled> LocaleFonts for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_and_russian_serif_get_their_own_fonts() {
        let first = |locale, face| fallbacks(locale, face).map(|f| f.fallback_list()[0].clone());
        assert_eq!(first("ja", Face::Sans).as_deref(), Some("Hiragino Sans"));
        assert_eq!(first("ja", Face::Serif).as_deref(), Some("Hiragino Mincho ProN"));
        assert_eq!(first("zh-CN", Face::Sans).as_deref(), Some("PingFang SC"));
        assert_eq!(first("zh-CN", Face::Serif).as_deref(), Some("Songti SC"));
        assert_eq!(first("ko", Face::Sans).as_deref(), Some("Apple SD Gothic Neo"));
        assert_eq!(first("ko", Face::Serif).as_deref(), Some("AppleMyungjo"));
        assert_eq!(first("ru", Face::Serif).as_deref(), Some("Georgia"));
        assert!(fallbacks("ru", Face::Sans).is_none());
        assert!(fallbacks("es", Face::Sans).is_none());
        assert!(fallbacks("en", Face::Serif).is_none());
    }
}
