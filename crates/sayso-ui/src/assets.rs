//! Embedded assets: our icons first, gpui-kit's icon set as the fallback.

use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

#[derive(rust_embed::RustEmbed)]
#[folder = "assets"]
#[include = "icons/*.svg"]
#[include = "textures/*.png"]
#[include = "sounds/*.wav"]
struct SaysoAssets;

/// Pass to `Application::with_assets`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }
        if let Some(file) = SaysoAssets::get(path) {
            return Ok(Some(file.data));
        }
        gpui_kit::assets::Assets::new("").load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> =
            SaysoAssets::iter().filter(|name| name.starts_with(path)).map(|n| n.to_string().into()).collect();
        out.extend(gpui_kit::assets::Assets::new("").list(path)?);
        Ok(out)
    }
}

/// Raw bytes of an embedded asset, for code that needs them directly (sounds, textures).
pub fn bytes(path: &str) -> Option<Cow<'static, [u8]>> {
    SaysoAssets::get(path).map(|f| f.data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_icons_textures_and_sounds_only() {
        assert!(bytes("icons/mic.svg").is_some());
        assert!(bytes("textures/board-light.png").is_some());
        assert!(bytes("sounds/start.wav").is_some());
        // The Sayso name and the app icon belong to Watzon Ventures LLC. This crate is MIT.
        assert!(bytes("app/tray.png").is_none());
        assert!(bytes("app/icon-64.png").is_none());
        assert!(SaysoAssets::iter().all(|name| ["icons/", "textures/", "sounds/"].iter().any(|dir| name.starts_with(dir))));
    }
}

/// Icon names. Each maps to `icons/<name>.svg`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Home,
    History,
    Dictionary,
    Styles,
    Models,
    Settings,
    General,
    Mic,
    Overlay,
    Audio,
    Drop,
    Shield,
    Lock,
    Code,
    Search,
    Plus,
    Check,
    ChevronLeft,
    ChevronRight,
    ChevronDown,
    ArrowRight,
    ArrowReplace,
    Play,
    Pause,
    Cloud,
    Globe,
    Warning,
    Apps,
    ShieldCheck,
    Speaker,
    Eye,
    Terminal,
    Activity,
    Info,
    Sun,
    Moon,
    Display,
    Copy,
    Trash,
    Close,
    Undo,
    Sparkle,
    Download,
    Keyboard,
    More,
    File,
    Sidebar,
    Wordmark,
}

impl Icon {
    pub fn path(self) -> SharedString {
        let name = match self {
            Icon::Home => "home",
            Icon::History => "history",
            Icon::Dictionary => "dictionary",
            Icon::Styles => "styles",
            Icon::Models => "models",
            Icon::Settings => "settings",
            Icon::General => "general",
            Icon::Mic => "mic",
            Icon::Overlay => "overlay",
            Icon::Audio => "audio",
            Icon::Drop => "drop",
            Icon::Shield => "shield",
            Icon::Lock => "lock",
            Icon::Code => "code",
            Icon::Search => "search",
            Icon::Plus => "plus",
            Icon::Check => "check",
            Icon::ChevronLeft => "chevron-left",
            Icon::ChevronRight => "chevron-right",
            Icon::ChevronDown => "chevron-down",
            Icon::ArrowRight => "arrow-right",
            Icon::ArrowReplace => "arrow-replace",
            Icon::Play => "play",
            Icon::Pause => "pause",
            Icon::Cloud => "cloud",
            Icon::Globe => "globe",
            Icon::Warning => "warning",
            Icon::Apps => "apps",
            Icon::ShieldCheck => "shield-check",
            Icon::Speaker => "speaker",
            Icon::Eye => "eye",
            Icon::Terminal => "terminal",
            Icon::Activity => "activity",
            Icon::Info => "info",
            Icon::Sun => "sun",
            Icon::Moon => "moon",
            Icon::Display => "display",
            Icon::Copy => "copy",
            Icon::Trash => "trash",
            Icon::Close => "close",
            Icon::Undo => "undo",
            Icon::Sparkle => "sparkle",
            Icon::Download => "download",
            Icon::Keyboard => "keyboard",
            Icon::More => "more",
            Icon::File => "file",
            Icon::Sidebar => "sidebar",
            Icon::Wordmark => "wordmark",
        };
        format!("icons/{name}.svg").into()
    }
}
