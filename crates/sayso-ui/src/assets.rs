//! Embedded assets: our icons first, gpui-kit's icon set as the fallback.

use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

#[derive(rust_embed::RustEmbed)]
#[folder = "../../assets"]
#[include = "icons/*.svg"]
#[include = "textures/*.png"]
#[include = "sounds/*.wav"]
#[include = "app/*.png"]
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
    Warning,
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
            Icon::Warning => "warning",
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
            Icon::Wordmark => "wordmark",
        };
        format!("icons/{name}.svg").into()
    }
}
