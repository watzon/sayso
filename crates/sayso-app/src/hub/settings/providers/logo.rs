//! The mark of a provider, on a small raised plate. The marks are in
//! `sayso-ui/assets/logos` (see its README). A one-color mark is a tinted
//! icon, so it follows the theme. A mark with colors, and an app icon, is an
//! image.

use gpui_kit::*;
use sayso_core::config::{Provider, ProviderKind, is_local_url};
use sayso_ui::ActivePaper;
use sayso_ui::assets::Icon;
use sayso_ui::components::icon;
use sayso_ui::paper;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

/// How a mark is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Mark {
    /// A one-color SVG, tinted with the ink.
    Tinted(&'static str),
    /// An SVG with its own colors.
    Colored(&'static str),
    /// An app icon: it fills the plate.
    AppIcon(&'static str),
    /// No mark of its own: an icon of Sayso.
    Glyph(Icon),
}

pub(super) const OPENAI: Mark = Mark::Tinted("logos/openai.svg");
pub(super) const CLAUDE: Mark = Mark::Colored("logos/claude-color.svg");
pub(super) const APPLE: Mark = Mark::Tinted("logos/apple.svg");
pub(super) const OPENROUTER: Mark = Mark::Tinted("logos/openrouter.svg");
pub(super) const OLLAMA: Mark = Mark::Tinted("logos/ollama.svg");
pub(super) const LM_STUDIO: Mark = Mark::Tinted("logos/lmstudio.svg");
pub(super) const SERVER: Mark = Mark::Glyph(Icon::Globe);
pub(super) const AGENT: Mark = Mark::Glyph(Icon::Terminal);

/// The mark of a cleanup model, by its publisher.
pub(super) fn cleanup(model: &str) -> Mark {
    if model.starts_with("spokenly") { Mark::AppIcon("logos/spokenly.png") } else { Mark::AppIcon("logos/superwhisper.png") }
}

/// The mark of an OpenAI-compatible server, from its address.
pub(super) fn server(base_url: &str) -> Mark {
    if base_url.contains("openrouter.ai") {
        OPENROUTER
    } else if base_url.contains("api.openai.com") {
        OPENAI
    } else if base_url.contains(":11434") {
        OLLAMA
    } else if base_url.contains(":1234") && is_local_url(base_url) {
        LM_STUDIO
    } else {
        SERVER
    }
}

/// The mark of a provider.
pub(super) fn of(p: &Provider) -> Mark {
    match &p.kind {
        ProviderKind::ChatGpt { .. } => OPENAI,
        ProviderKind::ClaudeCli { .. } => CLAUDE,
        ProviderKind::AppleIntelligence => APPLE,
        ProviderKind::OpenAiCompatible { base_url, .. } => server(base_url),
        ProviderKind::CleanupModel { model } => cleanup(model),
    }
}

/// The images of the marks, made once.
static IMAGES: LazyLock<Mutex<HashMap<&'static str, Arc<Image>>>> = LazyLock::new(Mutex::default);

fn image(path: &'static str) -> Option<Arc<Image>> {
    let mut images = IMAGES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = images.get(path) {
        return Some(found.clone());
    }
    let bytes = sayso_ui::assets::bytes(path)?;
    let format = if path.ends_with(".png") { ImageFormat::Png } else { ImageFormat::Svg };
    let made = Arc::new(Image::from_bytes(format, bytes.into_owned()));
    images.insert(path, made.clone());
    Some(made)
}

/// The mark on a plate of `size` px. `strong` draws the plate in ink, for
/// the large mark at the top of the details.
pub(super) fn tile(mark: Mark, size: f32, cx: &App) -> Div {
    let c = cx.paper().colors;
    let radius = (size * 0.27).round();
    let plate = div().flex().flex_none().items_center().justify_center().size(px(size)).rounded(px(radius)).overflow_hidden();
    let inner = (size * 0.6).round();
    match mark {
        Mark::AppIcon(path) => match image(path) {
            Some(png) => plate.shadow(paper::raised_small(&c)).child(img(png).size(px(size))),
            None => plate.bg(c.deboss),
        },
        Mark::Colored(path) => {
            let plate = plate.bg(c.sheet_raised).shadow(paper::raised_small(&c));
            match image(path) {
                Some(svg) => plate.child(img(svg).size(px(inner))),
                None => plate,
            }
        }
        Mark::Tinted(path) => plate.bg(c.sheet_raised).shadow(paper::raised_small(&c)).child(svg().path(path).size(px(inner)).text_color(c.ink)),
        Mark::Glyph(glyph) => plate.bg(c.sheet_raised).shadow(paper::raised_small(&c)).child(icon(glyph, (size * 0.5).round(), c.ink)),
    }
}

#[cfg(test)]
mod tests {
    // No glob import: `gpui_kit::*` has its own `test` attribute.
    use super::{APPLE, CLAUDE, LM_STUDIO, Mark, OLLAMA, OPENAI, OPENROUTER, SERVER, cleanup, server};

    #[test]
    fn every_mark_file_is_embedded() {
        for mark in [OPENAI, CLAUDE, APPLE, OPENROUTER, OLLAMA, LM_STUDIO, cleanup("s1-mini"), cleanup("spokenly-cleanup-1")] {
            let path = match mark {
                Mark::Tinted(p) | Mark::Colored(p) | Mark::AppIcon(p) => p,
                Mark::Glyph(_) => continue,
            };
            assert!(sayso_ui::assets::bytes(path).is_some(), "{path} is not embedded");
        }
    }

    #[test]
    fn a_server_gets_the_mark_of_its_host() {
        assert_eq!(server("https://openrouter.ai/api/v1"), OPENROUTER);
        assert_eq!(server("https://api.openai.com/v1"), OPENAI);
        assert_eq!(server("http://localhost:11434/v1"), OLLAMA);
        assert_eq!(server("http://localhost:1234/v1"), LM_STUDIO);
        assert_eq!(server("https://llm.example.com/v1"), SERVER);
    }
}
