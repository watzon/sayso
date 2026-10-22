//! `config.toml`: the single source of truth for settings (plan §3).
//!
//! Every field has a default, so a missing or partial file works. Invalid
//! values produce a [`ConfigIssue`] that Settings shows. Sayso never falls back
//! silently.

use crate::hotkey::Hotkey;
use crate::ink::Ink;
use crate::models::{ModelId, default_model};
use crate::speech::{Speech, SpeechProviderKind};
use crate::style::DEFAULT_STYLE_ID;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub hotkeys: Hotkeys,
    pub dictation: Dictation,
    pub insertion: Insertion,
    pub overlay: Overlay,
    pub audio: Audio,
    pub sounds: Sounds,
    pub ai: Ai,
    /// Cloud speech providers for the final pass.
    pub speech: Speech,
    pub history: History,
    pub appearance: AppearanceConfig,
    pub updates: Updates,
    pub advanced: Advanced,
    /// Set when onboarding finished. Onboarding resumes from `onboarding_step` otherwise.
    pub onboarding: Onboarding,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub launch_at_login: bool,
    /// Show the Dock icon while the Hub is open.
    pub dock_icon_with_hub: bool,
    /// The name in the Home greeting. None uses the first name of the
    /// macOS account.
    pub name: Option<String>,
    /// The interface language: "system" or a locale code such as "es" or
    /// "zh-CN". See `docs/i18n.md`.
    pub language: String,
}

impl Default for General {
    fn default() -> Self {
        Self { launch_at_login: false, dock_icon_with_hub: true, name: None, language: sayso_i18n::SYSTEM.into() }
    }
}

impl General {
    /// The name the user set, without spaces around it. None when empty.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref().map(str::trim).filter(|n| !n.is_empty())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CancelMode {
    #[default]
    DoubleEscape,
    SingleEscape,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    #[serde(with = "optional_hotkey")]
    pub toggle: Option<Hotkey>,
    #[serde(with = "optional_hotkey")]
    pub push_to_talk: Option<Hotkey>,
    pub cancel: CancelMode,
    /// Two Esc presses within this window cancel.
    pub double_escape_ms: u64,
    #[serde(with = "optional_hotkey")]
    pub paste_last: Option<Hotkey>,
    #[serde(with = "optional_hotkey")]
    pub cycle_style: Option<Hotkey>,
    /// Turns incognito on and off. It has no default.
    #[serde(with = "optional_hotkey")]
    pub incognito: Option<Hotkey>,
    /// Enter while recording stops the dictation, and Sayso presses Enter
    /// after the insert. Sayso then takes that Enter from the app. macOS and
    /// Windows; off by default.
    pub enter_sends: bool,
    /// While recording, Sayso takes every plain Esc from the app, so the Esc
    /// that cancels does not also close a dialog or clear a field. macOS and
    /// Windows; on by default.
    pub take_escape: bool,
}

/// A hotkey with no key is the empty string in the file. TOML has no null, and
/// a missing line means the default, so a cleared hotkey needs its own value.
mod optional_hotkey {
    use super::Hotkey;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(hotkey: &Option<Hotkey>, s: S) -> Result<S::Ok, S::Error> {
        match hotkey {
            Some(hk) => s.collect_str(hk),
            None => s.serialize_str(""),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Hotkey>, D::Error> {
        let s = String::deserialize(d)?;
        if s.trim().is_empty() {
            return Ok(None);
        }
        s.parse().map(Some).map_err(serde::de::Error::custom)
    }
}

impl Hotkeys {
    /// Whether Send with Enter can work: it is on, and the toggle and
    /// push-to-talk hotkeys do not use Return, which it takes while recording.
    pub fn enter_sends_usable(&self) -> bool {
        self.enter_sends && !self.uses_key(crate::hotkey::Key::Return)
    }

    /// Whether Sayso can take Esc while recording: the setting is on, and the
    /// toggle and push-to-talk hotkeys do not use Esc.
    pub fn take_escape_usable(&self) -> bool {
        self.take_escape && !self.uses_key(crate::hotkey::Key::Escape)
    }

    /// The toggle or push-to-talk hotkey uses `key`.
    fn uses_key(&self, key: crate::hotkey::Key) -> bool {
        let uses = |h: &Option<Hotkey>| matches!(h, Some(Hotkey::Chord { key: k, .. }) if *k == key);
        uses(&self.toggle) || uses(&self.push_to_talk)
    }
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            toggle: Some(Hotkey::toggle_default()),
            push_to_talk: None,
            cancel: CancelMode::DoubleEscape,
            double_escape_ms: 400,
            paste_last: Some(Hotkey::paste_last_default()),
            cycle_style: "ctrl+opt+s".parse().ok(),
            incognito: None,
            enter_sends: false,
            take_escape: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dictation {
    /// A language code such as "en" or "de", or "auto" to let the model detect
    /// the language. A model that does not support the language ignores it.
    pub language: String,
    /// The final model: a local model id, or "<provider id>:<model>" for a
    /// speech provider's model.
    pub model: ModelId,
    /// The preview model: "auto", or the id of a streaming local model.
    pub preview_model: PreviewChoice,
    pub live_preview: bool,
    /// Change spoken punctuation ("comma", "new line") into marks and line
    /// breaks, after the replacements. The commands are English.
    pub spoken_punctuation: bool,
    /// A push-to-talk press shorter than this is ignored as an accident.
    pub min_duration_ms: u64,
    /// Recording stops by itself after this long.
    pub max_duration_s: u64,
    /// Minutes the active local model stays in memory after the last
    /// dictation. None keeps it in memory always. The live-preview model
    /// always stays in memory.
    pub keep_model_minutes: Option<u32>,
    /// Load the active local model when Sayso starts. When false, the first
    /// dictation loads it.
    pub preload_model: bool,
}

impl Default for Dictation {
    fn default() -> Self {
        Self {
            language: "en".into(),
            model: default_model(),
            preview_model: PreviewChoice::Auto,
            live_preview: true,
            spoken_punctuation: false,
            min_duration_ms: 300,
            max_duration_s: 600,
            keep_model_minutes: None,
            preload_model: true,
        }
    }
}

/// Which model gives the live preview.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum PreviewChoice {
    /// The final model when it streams. Otherwise a streaming model on disk
    /// that understands the dictation language (`models::preview_for`).
    #[default]
    Auto,
    /// This streaming model, also when another model would fit better.
    Model(ModelId),
}

impl From<String> for PreviewChoice {
    fn from(s: String) -> Self {
        if s == "auto" { PreviewChoice::Auto } else { PreviewChoice::Model(ModelId(s)) }
    }
}

impl From<PreviewChoice> for String {
    fn from(c: PreviewChoice) -> Self {
        match c {
            PreviewChoice::Auto => "auto".into(),
            PreviewChoice::Model(id) => id.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Insertion {
    pub restore_clipboard: bool,
    /// Wait this long after the app read the pasted text before the clipboard
    /// is restored. Some apps read the clipboard more than one time for a paste.
    pub restore_delay_ms: u64,
    /// Bundle ids where Sayso types instead of pasting.
    pub type_in_apps: Vec<String>,
    /// Add a space before the text when the cursor follows a word. Reserved.
    pub smart_spacing: bool,
    /// Add a space after the text, so the next dictation does not join the
    /// last word.
    pub trailing_space: bool,
}

impl Default for Insertion {
    fn default() -> Self {
        Self { restore_clipboard: true, restore_delay_ms: 450, type_in_apps: Vec::new(), smart_spacing: false, trailing_space: true }
    }
}

impl Insertion {
    /// The text that goes into the app. Text that is empty or that ends with
    /// a space or a line break gets no trailing space.
    pub fn text_to_insert(&self, text: &str) -> String {
        let ends_open = text.chars().next_back().is_some_and(|c| !c.is_whitespace());
        if self.trailing_space && ends_open { format!("{text} ") } else { text.to_string() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Overlay {
    /// How the overlay shows a dictation: the pill or the notch.
    pub style: OverlayStyle,
    /// With the notch style, draw a notch on a display that has no camera
    /// notch. When false, such a display uses the pill. Only macOS has
    /// displays with a notch, so the other systems always draw one.
    pub notch_on_other_displays: bool,
    /// Show the small idle pill.
    pub idle_pill: bool,
    pub hide_in_fullscreen: bool,
    /// Move the overlay to the display that the user works on.
    pub follow_display: bool,
    /// Show live preview text above the recording pill.
    pub show_preview: bool,
    /// The size of the overlay and the idle pill.
    pub size: OverlaySize,
}

impl Default for Overlay {
    fn default() -> Self {
        Self {
            style: OverlayStyle::default(),
            notch_on_other_displays: true,
            idle_pill: true,
            hide_in_fullscreen: true,
            follow_display: true,
            show_preview: true,
            size: OverlaySize::default(),
        }
    }
}

/// The look of the overlay (Settings › Overlay › Style).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayStyle {
    /// A slip of paper that waits on the screen and that the user can drag.
    #[default]
    Pill,
    /// The camera notch at the top center of the display, which opens only
    /// while a dictation runs.
    Notch,
}

impl OverlayStyle {
    pub const ALL: [OverlayStyle; 2] = [OverlayStyle::Pill, OverlayStyle::Notch];
}

/// Large is the size of the design.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlaySize {
    Small,
    Medium,
    #[default]
    Large,
}

impl OverlaySize {
    pub const ALL: [OverlaySize; 3] = [OverlaySize::Small, OverlaySize::Medium, OverlaySize::Large];

    /// The factor for shapes, gaps, and icons.
    pub fn scale(self) -> f32 {
        match self {
            OverlaySize::Small => 0.75,
            OverlaySize::Medium => 0.87,
            OverlaySize::Large => 1.0,
        }
    }

    /// The factor for text. Text shrinks half as much as shapes, so it stays
    /// easy to read at the small size.
    pub fn text_scale(self) -> f32 {
        0.5 + 0.5 * self.scale()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Audio {
    /// Device name. None uses the system default input.
    pub input_device: Option<String>,
    /// Pause the media that plays while Sayso records, and play it again after.
    pub pause_media: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sounds {
    pub enabled: bool,
    /// 0.0 to 1.0.
    pub volume: f32,
    pub start: bool,
    pub stop: bool,
    pub cancel: bool,
    pub insert: bool,
}

impl Default for Sounds {
    fn default() -> Self {
        Self { enabled: true, volume: 0.35, start: true, stop: true, cancel: true, insert: false }
    }
}

/// The thinking level that a CLI provider gets when the user chose none.
/// All models of the two tools that have levels take it.
pub const DEFAULT_CLI_THINKING: &str = "low";

/// The thinking levels that Sayso knows, from the least thinking to the most.
/// A provider can report other names; they sort after these.
const THINKING_ORDER: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Put thinking levels in order, from the least thinking to the most, without duplicates.
pub fn sort_thinking(mut levels: Vec<String>) -> Vec<String> {
    let rank = |level: &String| THINKING_ORDER.iter().position(|known| known == level).unwrap_or(THINKING_ORDER.len());
    levels.sort_by_key(rank);
    levels.dedup();
    levels
}

/// The name of a thinking level for the UI.
pub fn thinking_label(level: &str) -> String {
    match level {
        "xhigh" => "Extra high".into(),
        other => {
            let mut chars = other.chars();
            chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderKind {
    /// OpenRouter, OpenAI, Ollama, LM Studio, llama.cpp server.
    OpenAiCompatible {
        base_url: String,
        /// Empty until the user chooses one from the provider's model list.
        #[serde(default)]
        model: String,
        /// Keychain account name. The key itself is never in this file.
        #[serde(default)]
        api_key_account: Option<String>,
        /// OpenRouter only: route to zero-data-retention endpoints.
        #[serde(default)]
        zero_data_retention: bool,
        /// The thinking level. None sends no level, and the model uses its own default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking: Option<String>,
    },
    ClaudeCli {
        #[serde(default)]
        path: Option<String>,
        #[serde(default = "default_claude_model")]
        model: String,
        /// The folder that holds the login of the tool (`CLAUDE_CONFIG_DIR`). None uses the tool's own default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config_dir: Option<String>,
        /// The thinking level. None uses [`DEFAULT_CLI_THINKING`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking: Option<String>,
    },
    CodexCli {
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        model: Option<String>,
        /// The folder that holds the login of the tool (`CODEX_HOME`). None uses the tool's own default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config_dir: Option<String>,
        /// The thinking level. None uses [`DEFAULT_CLI_THINKING`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking: Option<String>,
    },
    /// The language model of macOS, which the engine runs on this Mac.
    AppleIntelligence,
    /// A local model that cleans the text and reads no prompt. `model` is the
    /// id of the model in the catalog. In `config.toml` it is in
    /// `[[ai.cleanup_models]]`, not in `[[ai.providers]]`.
    CleanupModel { model: String },
}

impl ProviderKind {
    /// The name of the kind, as `config.toml` has it.
    pub fn name(&self) -> &'static str {
        match self {
            ProviderKind::OpenAiCompatible { .. } => "open_ai_compatible",
            ProviderKind::ClaudeCli { .. } => "claude_cli",
            ProviderKind::CodexCli { .. } => "codex_cli",
            ProviderKind::AppleIntelligence => "apple_intelligence",
            ProviderKind::CleanupModel { .. } => "cleanup_model",
        }
    }
}

fn default_claude_model() -> String {
    "haiku".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub kind: ProviderKind,
}

impl Provider {
    /// The chosen model. None when no model is chosen yet, or when a CLI uses
    /// its own default.
    pub fn model(&self) -> Option<&str> {
        let model = match &self.kind {
            ProviderKind::OpenAiCompatible { model, .. } | ProviderKind::ClaudeCli { model, .. } => Some(model.as_str()),
            ProviderKind::CodexCli { model, .. } => model.as_deref(),
            ProviderKind::AppleIntelligence => None,
            ProviderKind::CleanupModel { model } => Some(model.as_str()),
        };
        model.filter(|m| !m.trim().is_empty())
    }

    /// The folder with the login of a CLI provider. None for the default folder of the tool, and for kinds without a login.
    pub fn config_dir(&self) -> Option<&str> {
        match &self.kind {
            ProviderKind::ClaudeCli { config_dir, .. } | ProviderKind::CodexCli { config_dir, .. } => {
                config_dir.as_deref().filter(|dir| !dir.trim().is_empty())
            }
            _ => None,
        }
    }

    /// The thinking level that the user chose. None when the user chose
    /// none, and for a kind without levels.
    pub fn thinking(&self) -> Option<&str> {
        match &self.kind {
            ProviderKind::OpenAiCompatible { thinking, .. }
            | ProviderKind::ClaudeCli { thinking, .. }
            | ProviderKind::CodexCli { thinking, .. } => thinking.as_deref().filter(|level| !level.trim().is_empty()),
            ProviderKind::AppleIntelligence | ProviderKind::CleanupModel { .. } => None,
        }
    }

    /// The thinking level that goes to the provider. None sends no level.
    pub fn thinking_sent(&self) -> Option<&str> {
        let default = matches!(self.kind, ProviderKind::ClaudeCli { .. } | ProviderKind::CodexCli { .. }).then_some(DEFAULT_CLI_THINKING);
        self.thinking().or(default)
    }

    pub fn set_thinking(&mut self, level: Option<String>) {
        if let ProviderKind::OpenAiCompatible { thinking, .. }
        | ProviderKind::ClaudeCli { thinking, .. }
        | ProviderKind::CodexCli { thinking, .. } = &mut self.kind
        {
            *thinking = level;
        }
    }

    /// Make the thinking level fit a model that offers `levels`, from the
    /// least thinking to the most. A level that the model does not have
    /// becomes its lowest level. A model without levels gets none.
    pub fn fit_thinking(&mut self, levels: &[String]) {
        let fits = self.thinking_sent().is_some_and(|sent| levels.iter().any(|level| level == sent));
        if levels.is_empty() {
            self.set_thinking(None);
        } else if !fits {
            self.set_thinking(levels.first().cloned());
        }
    }

    pub fn set_model(&mut self, id: String) {
        match &mut self.kind {
            ProviderKind::OpenAiCompatible { model, .. } | ProviderKind::ClaudeCli { model, .. } => *model = id,
            ProviderKind::CodexCli { model, .. } => *model = Some(id),
            // macOS chooses the model.
            ProviderKind::AppleIntelligence => {}
            // The catalog fixes the model.
            ProviderKind::CleanupModel { .. } => {}
        }
    }

    /// True when the provider cannot run until a model is chosen. The CLIs
    /// have their own default; an HTTP server has none.
    pub fn needs_model(&self) -> bool {
        matches!(self.kind, ProviderKind::OpenAiCompatible { .. }) && self.model().is_none()
    }

    /// True when the provider sends text off this Mac.
    pub fn is_cloud(&self) -> bool {
        match &self.kind {
            ProviderKind::OpenAiCompatible { base_url, .. } => !is_local_url(base_url),
            ProviderKind::ClaudeCli { .. } | ProviderKind::CodexCli { .. } => true,
            ProviderKind::AppleIntelligence | ProviderKind::CleanupModel { .. } => false,
        }
    }
}

pub fn is_local_url(url: &str) -> bool {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split(['/', ':']).next().unwrap_or("");
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]" | "0.0.0.0") || host.ends_with(".local")
}

/// The `[ai]` section. In memory `providers` holds every provider, also the
/// cleanup models. In the file the cleanup models are in their own list (see
/// [`AiFile`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "AiFile", into = "AiFile")]
pub struct Ai {
    /// Off until the user sets up a provider. Off means every style acts like Raw.
    pub enabled: bool,
    pub active_style: String,
    pub default_provider: Option<String>,
    pub providers: Vec<Provider>,
    /// Entries of a provider kind that this version does not know. They stay in the file and never run.
    pub unknown_providers: Vec<toml::Value>,
    pub http_timeout_ms: u64,
    pub cli_timeout_ms: u64,
    /// The apps and sites that choose a style.
    pub style_rules: crate::style_rules::StyleRules,
}

impl Default for Ai {
    fn default() -> Self {
        Self {
            enabled: false,
            active_style: DEFAULT_STYLE_ID.into(),
            default_provider: None,
            providers: Vec::new(),
            unknown_providers: Vec::new(),
            http_timeout_ms: 4000,
            cli_timeout_ms: 8000,
            style_rules: Default::default(),
        }
    }
}

impl Ai {
    pub fn provider(&self, id: Option<&str>) -> Option<&Provider> {
        let id = id.or(self.default_provider.as_deref())?;
        self.providers.iter().find(|p| p.id == id)
    }
}

/// The `[ai]` section as `config.toml` has it.
///
/// An older Sayso reads `[ai]` in one piece, and a provider of a kind that it
/// does not know makes it use the default for the whole section. So a cleanup
/// model is not in `providers`. It is in `cleanup_models`, which an older
/// Sayso ignores.
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct AiFile {
    enabled: bool,
    active_style: String,
    default_provider: Option<String>,
    providers: Vec<ProviderEntry>,
    http_timeout_ms: u64,
    cli_timeout_ms: u64,
    style_rules: crate::style_rules::StyleRules,
    /// Not written when empty, so a file without cleanup models stays as it was.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    cleanup_models: Vec<CleanupModelEntry>,
}

impl Default for AiFile {
    fn default() -> Self {
        Ai::default().into()
    }
}

/// An entry of `[[ai.providers]]`.
enum ProviderEntry {
    Known(Provider),
    /// A kind that this version does not know, as it is in the file.
    Unknown(toml::Value),
}

impl Serialize for ProviderEntry {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            ProviderEntry::Known(provider) => provider.serialize(serializer),
            ProviderEntry::Unknown(value) => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ProviderEntry {
    /// Read each entry on its own, so one bad entry does not fail the section.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = toml::Value::deserialize(deserializer)?;
        Ok(match value.clone().try_into::<Provider>() {
            Ok(provider) => ProviderEntry::Known(provider),
            Err(error) => {
                let field = |name: &str| value.get(name).and_then(toml::Value::as_str).unwrap_or("?").to_string();
                log::warn!("keeping the provider {:?} of kind {:?} as it is, it will not run: {}", field("id"), field("kind"), error);
                ProviderEntry::Unknown(value)
            }
        })
    }
}

/// An entry of `[[ai.cleanup_models]]`.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct CleanupModelEntry {
    id: String,
    name: String,
    /// The id of the model in the catalog.
    model: String,
}

impl From<AiFile> for Ai {
    fn from(file: AiFile) -> Self {
        let mut providers = Vec::new();
        let mut unknown_providers = Vec::new();
        for entry in file.providers {
            match entry {
                ProviderEntry::Known(provider) => providers.push(provider),
                ProviderEntry::Unknown(value) => unknown_providers.push(value),
            }
        }
        for entry in file.cleanup_models {
            if entry.id.is_empty() || entry.model.is_empty() {
                log::warn!("skipped a cleanup model with an empty id or model");
                continue;
            }
            providers.push(Provider { id: entry.id, name: entry.name, kind: ProviderKind::CleanupModel { model: entry.model } });
        }
        Ai {
            enabled: file.enabled,
            active_style: file.active_style,
            default_provider: file.default_provider,
            providers,
            unknown_providers,
            http_timeout_ms: file.http_timeout_ms,
            cli_timeout_ms: file.cli_timeout_ms,
            style_rules: file.style_rules,
        }
    }
}

impl From<Ai> for AiFile {
    fn from(ai: Ai) -> Self {
        let mut providers = Vec::new();
        let mut cleanup_models = Vec::new();
        for provider in ai.providers {
            match provider.kind {
                ProviderKind::CleanupModel { model } => cleanup_models.push(CleanupModelEntry { id: provider.id, name: provider.name, model }),
                kind => providers.push(ProviderEntry::Known(Provider { id: provider.id, name: provider.name, kind })),
            }
        }
        providers.extend(ai.unknown_providers.into_iter().map(ProviderEntry::Unknown));
        AiFile {
            enabled: ai.enabled,
            active_style: ai.active_style,
            default_provider: ai.default_provider,
            providers,
            http_timeout_ms: ai.http_timeout_ms,
            cli_timeout_ms: ai.cli_timeout_ms,
            style_rules: ai.style_rules,
            cleanup_models,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    /// Save each dictation as a history entry. When false, Sayso saves no
    /// text and no audio. Entries that exist stay until the user clears them.
    pub enabled: bool,
    /// Days to keep text. None keeps it forever.
    pub keep_text_days: Option<u32>,
    pub save_audio: bool,
    /// Days to keep audio. When audio expires only the audio file is deleted.
    pub keep_audio_days: Option<u32>,
    /// Show the icon of the site on a history entry. Sayso gets the icon
    /// from the site, or from the icon service of DuckDuckGo when the site
    /// gives none.
    pub site_icons: bool,
}

impl Default for History {
    fn default() -> Self {
        Self { enabled: true, keep_text_days: None, save_audio: true, keep_audio_days: Some(30), site_icons: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    pub theme: ThemeMode,
    pub ink: Ink,
    pub paper_texture: bool,
    /// None follows the macOS setting.
    pub reduce_motion: Option<bool>,
    /// Linux only. False gives Sayso's own title bar and window buttons.
    pub system_title_bar: bool,
    /// True shows the Hub sidebar as a rail of icons. A narrow window shows
    /// the rail also when this is false.
    pub sidebar_collapsed: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self { theme: ThemeMode::System, ink: Ink::default(), paper_texture: true, reduce_motion: None, system_title_bar: true, sidebar_collapsed: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Updates {
    /// Look for a new version when Sayso starts and one time each day.
    /// "Check for updates" in Settings works also when this is off.
    pub check: bool,
    /// Download and prepare a new version without a question. It takes the
    /// place of this version at the next start of Sayso.
    pub automatic: bool,
}

impl Default for Updates {
    fn default() -> Self {
        Self { check: true, automatic: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Advanced {
    /// Path to a custom `SaysoEngine` binary. None uses the bundled one.
    pub engine_path: Option<String>,
    /// "error", "warn", "info", "debug", or "trace".
    pub log_level: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Onboarding {
    pub completed: bool,
    /// 0 to 6. The step to show when Sayso starts before onboarding is done.
    pub step: u8,
}

/// A problem in config.toml. The app keeps running with the default for that value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigIssue {
    pub field: String,
    pub problem: ConfigProblem,
}

/// What is wrong with a value. `Display` gives the English text for the log;
/// the app shows the text of the interface language.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigProblem {
    /// The file is there, but Sayso cannot read it. `error` is the text of the system.
    #[error("could not read {path}: {error}")]
    Unreadable { path: String, error: String },
    /// The TOML syntax is wrong. The text of the TOML parser.
    #[error("{0}")]
    Syntax(String),
    /// A section has a bad value, so it uses its defaults. The text of the TOML parser.
    #[error("{0} (using defaults for this section)")]
    BadSection(String),
    #[error("must be between 0.0 and 1.0")]
    VolumeRange,
    #[error("timeouts must be at least 500 ms")]
    AiTimeoutTooShort,
    #[error("is the same key as hotkeys.toggle")]
    PushToTalkIsToggle,
    #[error("no provider has the id \"{0}\"")]
    UnknownProvider(String),
    #[error("\"{0}\" is not a known model")]
    UnknownModel(String),
    #[error("\"{0}\" cannot show a live preview. Use \"auto\" or a streaming model")]
    NotAStreamingModel(String),
    #[error("\"{0}\" is not an interface language Sayso has. Sayso uses the language of the system")]
    UnknownInterfaceLanguage(String),
    #[error("\"{0}\" is not a language code Sayso knows. Use \"auto\" or a code such as \"en\"")]
    UnknownDictationLanguage(String),
    #[error("must be at least 1000 ms")]
    SpeechTimeoutTooShort,
    #[error("the provider id \"{0}\" must not be empty or contain a colon")]
    BadProviderId(String),
    #[error("two providers have the id \"{0}\"")]
    DuplicateProviderId(String),
    #[error("the provider \"{0}\" needs a base_url")]
    NeedsBaseUrl(String),
}

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: Config,
    pub issues: Vec<ConfigIssue>,
    /// The file did not exist. Sayso writes the defaults on first save.
    pub created: bool,
}

impl LoadedConfig {
    /// The defaults for a file that is there but cannot be read. Not
    /// `created`, so the app does not write the defaults over the file.
    pub fn unreadable(error: &ConfigError) -> Self {
        let ConfigError::Io(path, e) = error;
        let issue = ConfigIssue { field: "config.toml".into(), problem: ConfigProblem::Unreadable { path: path.clone(), error: e.to_string() } };
        Self { config: Config::default(), issues: vec![issue], created: false }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read {0}: {1}")]
    Io(String, std::io::Error),
}

impl Config {
    /// Load the config. A syntax error keeps all defaults and reports the problem.
    /// A bad value in one field keeps the rest of the file.
    pub fn load(path: &Path) -> Result<LoadedConfig, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LoadedConfig { config: Config::default(), issues: Vec::new(), created: true });
            }
            Err(e) => return Err(ConfigError::Io(path.display().to_string(), e)),
        };
        Ok(Self::parse(&text))
    }

    pub fn parse(text: &str) -> LoadedConfig {
        match toml::from_str::<Config>(text) {
            Ok(config) => {
                let issues = config.validate();
                LoadedConfig { config, issues, created: false }
            }
            Err(whole) => {
                // Retry section by section, so one bad value does not reset everything.
                let mut issues = Vec::new();
                let config = match toml::from_str::<toml::Table>(text) {
                    Ok(table) => Self::from_sections(table, &mut issues),
                    Err(_) => {
                        issues.push(ConfigIssue { field: "config.toml".into(), problem: ConfigProblem::Syntax(whole.message().to_string()) });
                        Config::default()
                    }
                };
                let mut more = config.validate();
                issues.append(&mut more);
                LoadedConfig { config, issues, created: false }
            }
        }
    }

    fn from_sections(table: toml::Table, issues: &mut Vec<ConfigIssue>) -> Config {
        let mut cfg = Config::default();
        macro_rules! section {
            ($name:literal, $field:ident) => {
                if let Some(v) = table.get($name) {
                    match v.clone().try_into() {
                        Ok(parsed) => cfg.$field = parsed,
                        Err(e) => issues.push(ConfigIssue {
                            field: $name.into(),
                            problem: ConfigProblem::BadSection(toml::de::Error::message(&e).to_string()),
                        }),
                    }
                }
            };
        }
        section!("general", general);
        section!("hotkeys", hotkeys);
        section!("dictation", dictation);
        section!("insertion", insertion);
        section!("overlay", overlay);
        section!("audio", audio);
        section!("sounds", sounds);
        section!("ai", ai);
        section!("speech", speech);
        section!("history", history);
        section!("appearance", appearance);
        section!("updates", updates);
        section!("advanced", advanced);
        section!("onboarding", onboarding);
        cfg
    }

    pub fn validate(&self) -> Vec<ConfigIssue> {
        let mut issues = Vec::new();
        let mut issue = |field: &str, problem: ConfigProblem| issues.push(ConfigIssue { field: field.into(), problem });
        if !(0.0..=1.0).contains(&self.sounds.volume) {
            issue("sounds.volume", ConfigProblem::VolumeRange);
        }
        if self.ai.http_timeout_ms < 500 || self.ai.cli_timeout_ms < 500 {
            issue("ai", ConfigProblem::AiTimeoutTooShort);
        }
        if self.hotkeys.toggle.is_some() && self.hotkeys.toggle == self.hotkeys.push_to_talk {
            issue("hotkeys.push_to_talk", ConfigProblem::PushToTalkIsToggle);
        }
        if let Some(id) = &self.ai.default_provider
            && !self.ai.providers.iter().any(|p| &p.id == id) {
                issue("ai.default_provider", ConfigProblem::UnknownProvider(id.clone()));
            }
        if crate::models::find_with(&self.speech, &self.dictation.model).is_none() {
            issue("dictation.model", ConfigProblem::UnknownModel(self.dictation.model.to_string()));
        }
        if let PreviewChoice::Model(id) = &self.dictation.preview_model {
            match crate::models::find(id) {
                None => issue("dictation.preview_model", ConfigProblem::UnknownModel(id.to_string())),
                Some(m) if !m.live_preview => issue("dictation.preview_model", ConfigProblem::NotAStreamingModel(id.to_string())),
                Some(_) => {}
            }
        }
        if !sayso_i18n::is_valid_setting(&self.general.language) {
            issue("general.language", ConfigProblem::UnknownInterfaceLanguage(self.general.language.clone()));
        }
        if !crate::languages::is_valid(&self.dictation.language) {
            issue("dictation.language", ConfigProblem::UnknownDictationLanguage(self.dictation.language.clone()));
        }
        if self.speech.timeout_ms < 1000 {
            issue("speech.timeout_ms", ConfigProblem::SpeechTimeoutTooShort);
        }
        for (i, p) in self.speech.providers.iter().enumerate() {
            if p.id.is_empty() || p.id.contains(':') {
                issue("speech.providers", ConfigProblem::BadProviderId(p.id.clone()));
            }
            if self.speech.providers[..i].iter().any(|q| q.id == p.id) {
                issue("speech.providers", ConfigProblem::DuplicateProviderId(p.id.clone()));
            }
            if p.kind == SpeechProviderKind::OpenAiCompatible && p.base_url().is_none() {
                issue("speech.providers", ConfigProblem::NeedsBaseUrl(p.id.clone()));
            }
        }
        issues
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config always serializes")
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let toml = self.to_toml();
        if toml.contains("[[ai.cleanup_models]]") {
            back_up_before_cleanup_models(path);
        }
        crate::style::write_atomic(path, &toml)
    }
}

/// The name that the backup of the file before the first cleanup model has.
const BACKUP_SUFFIX: &str = ".before-cleanup-models";

/// Copy the file at `path` to `<name>.before-cleanup-models`, one time. An
/// older Sayso loses `[[ai.cleanup_models]]` when it saves, so the user can
/// get the file back. It does nothing when the file is not there, when it
/// already has cleanup models, or when the copy is there. A failure is logged
/// and does not stop the save.
fn back_up_before_cleanup_models(path: &Path) {
    let Some(name) = path.file_name() else { return };
    let Ok(old) = std::fs::read(path) else { return };
    if String::from_utf8_lossy(&old).contains("cleanup_models") {
        return;
    }
    let mut backup_name = name.to_os_string();
    backup_name.push(BACKUP_SUFFIX);
    let backup = path.with_file_name(backup_name);
    if backup.exists() {
        return;
    }
    if let Err(error) = std::fs::copy(path, &backup) {
        log::warn!("could not copy {} to {}: {}", path.display(), backup.display(), error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_that_cannot_be_read_is_reported_and_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // Not UTF-8.
        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        let error = Config::load(&path).unwrap_err();
        let loaded = LoadedConfig::unreadable(&error);
        assert!(!loaded.created);
        assert_eq!(loaded.issues.len(), 1);
        assert!(loaded.issues[0].problem.to_string().contains("config.toml"), "{}", loaded.issues[0].problem);
        assert_eq!(loaded.config, Config::default());
    }

    #[test]
    fn empty_file_gives_defaults() {
        let loaded = Config::parse("");
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.issues.is_empty());
    }

    #[test]
    fn a_cleared_hotkey_stays_cleared() {
        let mut c = Config::default();
        c.hotkeys.toggle = None;
        c.hotkeys.paste_last = None;
        let text = c.to_toml();
        assert!(text.contains("toggle = \"\""), "{text}");
        let loaded = Config::parse(&text);
        assert!(loaded.issues.is_empty(), "{:?}", loaded.issues);
        assert_eq!(loaded.config, c);
    }

    #[test]
    fn a_missing_hotkey_line_gives_the_default() {
        let loaded = Config::parse("[hotkeys]\npush_to_talk = \"right_option\"\n");
        assert!(loaded.issues.is_empty(), "{:?}", loaded.issues);
        assert_eq!(loaded.config.hotkeys.toggle, Some(Hotkey::toggle_default()));
        assert_eq!(loaded.config.hotkeys.push_to_talk, Some("right_option".parse().unwrap()));
    }

    #[test]
    fn the_model_stays_in_memory_unless_the_file_gives_a_time() {
        // A config file from before the setting.
        let old = Config::parse("[dictation]\nlanguage = \"de\"\n");
        assert!(old.issues.is_empty(), "{:?}", old.issues);
        assert_eq!(old.config.dictation.keep_model_minutes, None);
        let mut c = Config::default();
        c.dictation.keep_model_minutes = Some(15);
        let back = Config::parse(&c.to_toml());
        assert!(back.issues.is_empty(), "{:?}", back.issues);
        assert_eq!(back.config.dictation.keep_model_minutes, Some(15));
    }

    #[test]
    fn the_preview_model_is_automatic_unless_the_file_names_one() {
        // A config file from before the setting.
        let old = Config::parse("[dictation]\nlanguage = \"de\"\n");
        assert!(old.issues.is_empty(), "{:?}", old.issues);
        assert_eq!(old.config.dictation.preview_model, PreviewChoice::Auto);
        let streaming = crate::models::catalog().iter().find(|m| m.live_preview).unwrap().id.clone();
        let mut c = Config::default();
        c.dictation.preview_model = PreviewChoice::Model(streaming.clone());
        let back = Config::parse(&c.to_toml());
        assert!(back.issues.is_empty(), "{:?}", back.issues);
        assert_eq!(back.config.dictation.preview_model, PreviewChoice::Model(streaming));
    }

    #[test]
    fn the_preview_model_must_stream() {
        let mut c = Config::default();
        c.dictation.preview_model = PreviewChoice::Model(ModelId::new("no-such-model"));
        assert_eq!(c.validate()[0].problem, ConfigProblem::UnknownModel("no-such-model".into()));
        let batch = crate::models::catalog().iter().find(|m| !m.live_preview).unwrap().id.clone();
        c.dictation.preview_model = PreviewChoice::Model(batch.clone());
        assert_eq!(c.validate()[0].problem, ConfigProblem::NotAStreamingModel(batch.to_string()));
        assert_eq!(c.validate()[0].field, "dictation.preview_model");
    }

    #[test]
    fn the_model_loads_at_the_start_unless_the_file_says_no() {
        // A config file from before the setting.
        let old = Config::parse("[dictation]\nlanguage = \"de\"\n");
        assert!(old.issues.is_empty(), "{:?}", old.issues);
        assert!(old.config.dictation.preload_model);
        let off = Config::parse("[dictation]\npreload_model = false\n");
        assert!(off.issues.is_empty(), "{:?}", off.issues);
        assert!(!off.config.dictation.preload_model);
    }

    #[test]
    fn send_with_enter_steps_aside_for_a_hotkey_on_return() {
        let mut h = Hotkeys { enter_sends: true, ..Hotkeys::default() };
        assert!(h.enter_sends_usable());
        h.push_to_talk = Some("ctrl+return".parse().unwrap());
        assert!(!h.enter_sends_usable());
        h.push_to_talk = Some("right_option".parse().unwrap());
        h.toggle = Some("cmd+return".parse().unwrap());
        assert!(!h.enter_sends_usable());
        assert!(!Hotkeys::default().enter_sends_usable(), "off by default");
    }

    #[test]
    fn esc_is_taken_by_default_unless_a_hotkey_uses_it() {
        let mut h = Hotkeys::default();
        assert!(h.take_escape_usable(), "on by default");
        h.toggle = Some("shift+esc".parse().unwrap());
        assert!(!h.take_escape_usable());
        h.toggle = Hotkeys::default().toggle;
        h.take_escape = false;
        assert!(!h.take_escape_usable());
        // A config file from before the setting.
        let old = Config::parse("[hotkeys]\nenter_sends = true\n");
        assert!(old.issues.is_empty(), "{:?}", old.issues);
        assert!(old.config.hotkeys.take_escape);
    }

    #[test]
    fn defaults_match_the_plan() {
        let c = Config::default();
        assert!(c.updates.check);
        assert!(!c.updates.automatic);
        let (toggle, paste_last) = if cfg!(target_os = "macos") {
            ("opt+space", "ctrl+cmd+v")
        } else if cfg!(windows) {
            ("ctrl+space", "alt+shift+v")
        } else {
            ("ctrl+alt+space", "ctrl+alt+v")
        };
        assert_eq!(c.hotkeys.toggle.unwrap().to_string(), toggle);
        assert_eq!(c.hotkeys.push_to_talk, None);
        assert_eq!(c.hotkeys.incognito, None);
        assert!(!c.hotkeys.enter_sends);
        assert_eq!(c.hotkeys.cancel, CancelMode::DoubleEscape);
        assert_eq!(c.hotkeys.paste_last.unwrap().to_string(), paste_last);
        assert!(c.history.enabled);
        assert_eq!(c.history.keep_text_days, None);
        assert_eq!(c.history.keep_audio_days, Some(30));
        assert!(c.overlay.idle_pill);
        assert!(!c.ai.enabled);
        assert_eq!(c.ai.http_timeout_ms, 4000);
    }

    #[test]
    fn insertion_adds_one_trailing_space_by_default() {
        let on = Insertion::default();
        assert!(on.trailing_space);
        assert_eq!(on.text_to_insert("Hello there."), "Hello there. ");
        assert_eq!(on.text_to_insert("Hello "), "Hello ");
        assert_eq!(on.text_to_insert("First line\n"), "First line\n");
        assert_eq!(on.text_to_insert(""), "");
        let off = Insertion { trailing_space: false, ..Insertion::default() };
        assert_eq!(off.text_to_insert("Hello there."), "Hello there.");
        let read = Config::parse("[insertion]\ntrailing_space = false\n");
        assert!(read.issues.is_empty(), "{:?}", read.issues);
        assert!(!read.config.insertion.trailing_space);
    }

    #[test]
    fn round_trips_through_toml() {
        let mut c = Config::default();
        c.ai.providers.push(Provider {
            id: "openrouter".into(),
            name: "OpenRouter".into(),
            kind: ProviderKind::OpenAiCompatible {
                base_url: "https://openrouter.ai/api/v1".into(),
                model: "meta-llama/llama-3.3-8b-instruct".into(),
                api_key_account: Some("openrouter".into()),
                zero_data_retention: true,
                thinking: None,
            },
        });
        c.ai.default_provider = Some("openrouter".into());
        c.hotkeys.push_to_talk = Some("right_option".parse().unwrap());
        let back = Config::parse(&c.to_toml());
        assert!(back.issues.is_empty(), "{:?}", back.issues);
        assert_eq!(back.config, c);
    }

    #[test]
    fn the_thinking_level_is_low_until_the_user_changes_it() {
        // A config from before the setting has no level.
        let old = "[[ai.providers]]\nid = \"claude\"\nname = \"Claude CLI\"\nkind = \"claude_cli\"\nmodel = \"haiku\"\n";
        let read = Config::parse(old);
        assert!(read.issues.is_empty(), "{:?}", read.issues);
        let claude = &read.config.ai.providers[0];
        assert_eq!((claude.thinking(), claude.thinking_sent()), (None, Some("low")));
        assert!(!read.config.to_toml().contains("thinking"), "no level is written until the user chooses one");

        let mut c = read.config;
        c.ai.providers[0].set_thinking(Some("xhigh".into()));
        assert!(c.to_toml().contains("thinking = \"xhigh\""));
        let back = Config::parse(&c.to_toml());
        assert!(back.issues.is_empty(), "{:?}", back.issues);
        assert_eq!(back.config.ai.providers[0].thinking_sent(), Some("xhigh"));

        // An HTTP provider sends no level until the user chooses one.
        let http = Provider {
            id: "o".into(),
            name: "O".into(),
            kind: ProviderKind::OpenAiCompatible { base_url: "http://x".into(), model: "m".into(), api_key_account: None, zero_data_retention: false, thinking: None },
        };
        assert_eq!(http.thinking_sent(), None);
    }

    #[test]
    fn a_new_model_gets_a_level_that_it_has() {
        let levels = |names: &[&str]| sort_thinking(names.iter().map(|n| n.to_string()).collect());
        assert_eq!(levels(&["high", "odd", "none", "low", "high"]), ["none", "low", "high", "odd"]);
        assert_eq!((thinking_label("xhigh"), thinking_label("none")), ("Extra high".to_string(), "None".to_string()));

        let mut claude = Provider { id: "k".into(), name: "K".into(), kind: ProviderKind::ClaudeCli { path: None, model: "haiku".into(), config_dir: None, thinking: None } };
        claude.fit_thinking(&levels(&["low", "medium", "high"]));
        assert_eq!(claude.thinking(), None, "the default level fits, so nothing is written");
        claude.set_thinking(Some("max".into()));
        claude.fit_thinking(&levels(&["low", "medium", "high"]));
        assert_eq!(claude.thinking(), Some("low"), "a level that the model does not have becomes its lowest");
        claude.fit_thinking(&[]);
        assert_eq!(claude.thinking(), None, "a model without levels has none");

        let mut http = Provider {
            id: "o".into(),
            name: "O".into(),
            kind: ProviderKind::OpenAiCompatible { base_url: "http://x".into(), model: "m".into(), api_key_account: None, zero_data_retention: false, thinking: None },
        };
        http.fit_thinking(&levels(&["high", "minimal", "low"]));
        assert_eq!(http.thinking_sent(), Some("minimal"));
    }

    #[test]
    fn a_provider_without_a_model_needs_one() {
        let text = "[ai]\nexperimental_codex = true\n[[ai.providers]]\nid = \"or\"\nname = \"OpenRouter\"\nkind = \"open_ai_compatible\"\nbase_url = \"https://openrouter.ai/api/v1\"\n";
        let loaded = Config::parse(text);
        assert!(loaded.issues.is_empty(), "an old key and a missing model still load: {:?}", loaded.issues);
        let mut p = loaded.config.ai.providers[0].clone();
        assert_eq!(p.model(), None);
        assert!(p.needs_model());
        p.set_model("openai/gpt-5-mini".into());
        assert_eq!(p.model(), Some("openai/gpt-5-mini"));
        assert!(!p.needs_model());
    }

    #[test]
    fn cli_providers_never_need_a_model() {
        let mut codex = Provider { id: "c".into(), name: "Codex CLI".into(), kind: ProviderKind::CodexCli { path: None, model: None, config_dir: None, thinking: Default::default() } };
        assert_eq!(codex.model(), None);
        assert!(!codex.needs_model(), "Codex uses its own default");
        codex.set_model("gpt-6.1-sol".into());
        assert_eq!(codex.model(), Some("gpt-6.1-sol"));
        let claude = Provider { id: "k".into(), name: "Claude CLI".into(), kind: ProviderKind::ClaudeCli { path: None, model: "haiku".into(), config_dir: None, thinking: Default::default() } };
        assert_eq!(claude.model(), Some("haiku"));
        assert!(!claude.needs_model());
    }

    #[test]
    fn cli_config_dir_is_optional_and_not_written_when_unset() {
        // An old file has no config_dir.
        let old = "[[ai.providers]]\nid = \"k\"\nname = \"Claude CLI\"\nkind = \"claude_cli\"\nmodel = \"haiku\"\n";
        let parsed = Config::parse(old);
        assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
        let provider = parsed.config.ai.providers.iter().find(|p| p.id == "k").unwrap();
        assert_eq!(provider.config_dir(), None);

        let mut claude = Provider { id: "k".into(), name: "Claude CLI".into(), kind: ProviderKind::ClaudeCli { path: None, model: "haiku".into(), config_dir: None, thinking: Default::default() } };
        let unset = toml::to_string(&claude).unwrap();
        assert!(!unset.contains("config_dir"), "{unset}");

        let ProviderKind::ClaudeCli { config_dir, .. } = &mut claude.kind else { unreachable!() };
        *config_dir = Some("~/.claude-work".into());
        assert_eq!(claude.config_dir(), Some("~/.claude-work"));
        let set = toml::to_string(&claude).unwrap();
        assert!(set.contains("config_dir = \"~/.claude-work\""), "{set}");
        assert_eq!(toml::from_str::<Provider>(&set).unwrap(), claude);

        let ProviderKind::ClaudeCli { config_dir, .. } = &mut claude.kind else { unreachable!() };
        *config_dir = Some("  ".into());
        assert_eq!(claude.config_dir(), None, "a blank folder counts as unset");
        let codex = Provider { id: "c".into(), name: "C".into(), kind: ProviderKind::CodexCli { path: None, model: None, config_dir: Some("/x".into()), thinking: Default::default() } };
        assert_eq!(codex.config_dir(), Some("/x"));
        let apple = Provider { id: "a".into(), name: "A".into(), kind: ProviderKind::AppleIntelligence };
        assert_eq!(apple.config_dir(), None);
    }

    #[test]
    fn apple_intelligence_is_local_and_reads_from_the_config_file() {
        let parsed = Config::parse("[[ai.providers]]\nid = \"apple\"\nname = \"Apple Intelligence\"\nkind = \"apple_intelligence\"\n");
        assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
        let mut apple = parsed.config.ai.providers[0].clone();
        assert_eq!(apple.kind, ProviderKind::AppleIntelligence);
        assert!(!apple.is_cloud());
        assert!(!apple.needs_model(), "macOS chooses the model");
        apple.set_model("other".into());
        assert_eq!(apple.model(), None);
        let saved = parsed.config.to_toml();
        assert_eq!(Config::parse(&saved).config.ai.providers, parsed.config.ai.providers);
    }

    #[test]
    fn the_greeting_name_is_optional_and_trimmed() {
        assert_eq!(Config::default().general.name(), None);
        let set = Config::parse("[general]\nname = \"  Chris \"\n").config;
        assert_eq!(set.general.name(), Some("Chris"));
        let blank = Config::parse("[general]\nname = \" \"\n").config;
        assert_eq!(blank.general.name(), None, "a blank name falls back to the Mac account");
    }

    #[test]
    fn overlay_size_defaults_to_large_and_reads_from_toml() {
        assert_eq!(Config::default().overlay.size, OverlaySize::Large);
        let loaded = Config::parse("[overlay]\nsize = \"small\"\n");
        assert!(loaded.issues.is_empty(), "{:?}", loaded.issues);
        assert_eq!(loaded.config.overlay.size, OverlaySize::Small);
        assert!(loaded.config.overlay.idle_pill, "other keys keep their defaults");
        let small = OverlaySize::Small;
        assert!(small.scale() < OverlaySize::Medium.scale() && OverlaySize::Medium.scale() < OverlaySize::Large.scale());
        assert!(small.text_scale() > small.scale(), "text shrinks less than shapes");
    }

    #[test]
    fn overlay_style_defaults_to_the_pill_and_reads_from_toml() {
        let default = Config::default().overlay;
        assert_eq!(default.style, OverlayStyle::Pill);
        assert!(default.notch_on_other_displays);
        let loaded = Config::parse("[overlay]\nstyle = \"notch\"\nnotch_on_other_displays = false\n");
        assert!(loaded.issues.is_empty(), "{:?}", loaded.issues);
        assert_eq!(loaded.config.overlay.style, OverlayStyle::Notch);
        assert!(!loaded.config.overlay.notch_on_other_displays);
    }

    #[test]
    fn history_can_be_turned_off_and_keeps_the_other_keys() {
        let loaded = Config::parse("[history]\nenabled = false\n");
        assert!(loaded.issues.is_empty(), "{:?}", loaded.issues);
        assert!(!loaded.config.history.enabled);
        assert!(loaded.config.history.save_audio, "other keys keep their defaults");
        let old_file = Config::parse("[history]\nkeep_audio_days = 7\n");
        assert!(old_file.config.history.enabled, "a config from before the key keeps history on");
    }

    #[test]
    fn one_bad_section_keeps_the_others() {
        let text = "[sounds]\nvolume = \"loud\"\n[history]\nkeep_audio_days = 7\n";
        let loaded = Config::parse(text);
        assert_eq!(loaded.config.history.keep_audio_days, Some(7));
        assert_eq!(loaded.config.sounds, Sounds::default());
        assert_eq!(loaded.issues.len(), 1);
        assert_eq!(loaded.issues[0].field, "sounds");
    }

    #[test]
    fn syntax_error_reports_and_uses_defaults() {
        let loaded = Config::parse("[sounds\nvolume = 1");
        assert_eq!(loaded.config, Config::default());
        assert_eq!(loaded.issues[0].field, "config.toml");
    }

    #[test]
    fn validation_catches_bad_values() {
        let mut c = Config::default();
        c.sounds.volume = 3.0;
        c.ai.default_provider = Some("missing".into());
        c.hotkeys.push_to_talk = c.hotkeys.toggle;
        let fields: Vec<_> = c.validate().into_iter().map(|i| i.field).collect();
        assert!(fields.contains(&"sounds.volume".to_string()));
        assert!(fields.contains(&"ai.default_provider".to_string()));
        assert!(fields.contains(&"hotkeys.push_to_talk".to_string()));
    }

    #[test]
    fn language_and_cloud_model_are_validated() {
        use crate::speech::SpeechProvider;
        let mut c = Config::default();
        c.dictation.language = "de".into();
        assert!(c.validate().is_empty(), "a known language code is fine");
        c.dictation.language = "auto".into();
        assert!(c.validate().is_empty());
        c.dictation.language = "klingon".into();
        assert_eq!(c.validate()[0].field, "dictation.language");

        let mut c = Config::default();
        c.dictation.model = ModelId::new("groq:whisper-large-v3-turbo");
        assert_eq!(c.validate()[0].field, "dictation.model", "no such provider yet");
        c.speech.providers.push(SpeechProvider {
            id: "groq".into(),
            name: "Groq".into(),
            kind: SpeechProviderKind::Groq,
            base_url: None,
            api_key_account: Some("speech.groq".into()),
            models: vec![],
        });
        assert!(c.validate().is_empty(), "{:?}", c.validate());
        let back = Config::parse(&c.to_toml());
        assert!(back.issues.is_empty(), "{:?}", back.issues);
        assert_eq!(back.config, c);

        c.speech.providers.push(SpeechProvider {
            id: "groq".into(),
            name: "Mine".into(),
            kind: SpeechProviderKind::OpenAiCompatible,
            base_url: None,
            api_key_account: None,
            models: vec![],
        });
        let messages: Vec<_> = c.validate().into_iter().map(|i| i.problem.to_string()).collect();
        assert!(messages.iter().any(|m| m.contains("two providers")));
        assert!(messages.iter().any(|m| m.contains("base_url")));
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = Config::load(&dir.path().join("config.toml")).unwrap();
        assert!(loaded.created);
    }

    #[test]
    fn local_urls_are_not_cloud() {
        assert!(is_local_url("http://localhost:11434/v1"));
        assert!(is_local_url("http://127.0.0.1:1234/v1"));
        assert!(!is_local_url("https://openrouter.ai/api/v1"));
    }

    // Tests for cleanup models in the file.

    fn cleanup_model(id: &str, name: &str) -> Provider {
        Provider { id: id.into(), name: name.into(), kind: ProviderKind::CleanupModel { model: id.into() } }
    }

    fn config_with_cleanup_model() -> Config {
        let mut config = Config::default();
        config.ai.enabled = true;
        config.ai.default_provider = Some("s1-mini".into());
        config.ai.providers = vec![
            Provider { id: "apple-intelligence".into(), name: "Apple Intelligence".into(), kind: ProviderKind::AppleIntelligence },
            cleanup_model("s1-mini", "S1-mini"),
        ];
        config.ai.style_rules.enabled = true;
        config.ai.style_rules.rules = vec![crate::style_rules::StyleRule { style: "email".into(), apps: vec!["com.apple.mail".into()], sites: vec![] }];
        config
    }

    fn ai_table(toml_text: &str) -> toml::Table {
        let mut table: toml::Table = toml::from_str(toml_text).unwrap();
        match table.remove("ai") {
            Some(toml::Value::Table(ai)) => ai,
            other => panic!("no [ai] table: {other:?}"),
        }
    }

    fn kinds_of(array: Option<&toml::Value>) -> Vec<String> {
        array
            .and_then(toml::Value::as_array)
            .map(|entries| entries.iter().filter_map(|e| e.get("kind")).filter_map(toml::Value::as_str).map(String::from).collect())
            .unwrap_or_default()
    }

    #[test]
    fn a_config_with_a_cleanup_model_round_trips_and_keeps_it_out_of_the_provider_list() {
        let config = config_with_cleanup_model();
        let text = config.to_toml();

        let loaded = Config::parse(&text);
        assert!(loaded.issues.is_empty(), "{:?}", loaded.issues);
        assert_eq!(loaded.config, config);

        let ai = ai_table(&text);
        assert_eq!(kinds_of(ai.get("providers")), ["apple_intelligence"]);
        let cleanup = ai.get("cleanup_models").and_then(toml::Value::as_array).expect("cleanup_models is written");
        assert_eq!(cleanup.len(), 1);
        assert_eq!(cleanup[0].get("id").and_then(toml::Value::as_str), Some("s1-mini"));
        assert_eq!(cleanup[0].get("name").and_then(toml::Value::as_str), Some("S1-mini"));
        assert_eq!(cleanup[0].get("model").and_then(toml::Value::as_str), Some("s1-mini"));
    }

    /// The `[ai]` section of the version before cleanup models. Its provider
    /// kinds are the four old ones only, and it ignores keys that it does not know.
    #[derive(Debug, Deserialize)]
    #[serde(default)]
    struct OldAi {
        enabled: bool,
        active_style: String,
        default_provider: Option<String>,
        providers: Vec<OldProvider>,
        http_timeout_ms: u64,
        cli_timeout_ms: u64,
        style_rules: crate::style_rules::StyleRules,
    }

    impl Default for OldAi {
        fn default() -> Self {
            Self {
                enabled: false,
                active_style: DEFAULT_STYLE_ID.into(),
                default_provider: None,
                providers: Vec::new(),
                http_timeout_ms: 4000,
                cli_timeout_ms: 8000,
                style_rules: Default::default(),
            }
        }
    }

    #[derive(Debug, Deserialize)]
    struct OldProvider {
        id: String,
        #[allow(dead_code)]
        name: String,
        #[serde(flatten)]
        #[allow(dead_code)]
        kind: OldProviderKind,
    }

    #[derive(Debug, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum OldProviderKind {
        OpenAiCompatible {
            #[allow(dead_code)]
            base_url: String,
        },
        ClaudeCli {},
        CodexCli {},
        AppleIntelligence,
    }

    #[test]
    fn an_older_reader_keeps_the_other_providers_and_the_style_rules() {
        let config = config_with_cleanup_model();
        let text = config.to_toml();

        let old: OldAi = ai_table(&text).try_into().expect("the older version parses [ai]");
        assert!(old.enabled);
        assert_eq!(old.default_provider.as_deref(), Some("s1-mini"));
        assert_eq!(old.providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["apple-intelligence"]);
        assert_eq!(old.style_rules, config.ai.style_rules);
        assert_eq!((old.http_timeout_ms, old.cli_timeout_ms), (config.ai.http_timeout_ms, config.ai.cli_timeout_ms));
        assert_eq!(old.active_style, config.ai.active_style);
    }

    const TODAYS_FILE: &str = r#"
[general]
language = "en"

[ai]
enabled = true
active_style = "clean"
default_provider = "openrouter"
http_timeout_ms = 6000
cli_timeout_ms = 9000

[[ai.providers]]
id = "openrouter"
name = "OpenRouter"
kind = "open_ai_compatible"
base_url = "https://openrouter.ai/api/v1"
model = "openai/gpt-5-mini"
api_key_account = "openrouter"
zero_data_retention = true
thinking = "low"

[[ai.providers]]
id = "claude"
name = "Claude CLI"
kind = "claude_cli"
path = "/usr/local/bin/claude"
model = "haiku"
config_dir = "/Users/me/.claude-work"

[[ai.providers]]
id = "apple-intelligence"
name = "Apple Intelligence"
kind = "apple_intelligence"

[ai.style_rules]
enabled = true
seeded = true

[[ai.style_rules.rules]]
style = "email"
apps = ["com.apple.mail"]

[[ai.style_rules.rules]]
style = "message"
apps = ["com.tinyspeck.slackmacgap"]
sites = ["facebook.com/messages"]
"#;

    #[test]
    fn a_config_file_from_before_cleanup_models_loads_and_saves_the_same_config() {
        let loaded = Config::parse(TODAYS_FILE);
        assert!(loaded.issues.is_empty(), "{:?}", loaded.issues);
        let ai = &loaded.config.ai;
        assert!(ai.enabled);
        assert_eq!(ai.default_provider.as_deref(), Some("openrouter"));
        assert_eq!(ai.providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["openrouter", "claude", "apple-intelligence"]);
        assert!(ai.unknown_providers.is_empty());
        assert_eq!(ai.style_rules.rules.len(), 2);
        assert_eq!((ai.http_timeout_ms, ai.cli_timeout_ms), (6000, 9000));

        let saved = loaded.config.to_toml();
        assert!(!saved.contains("cleanup_models"), "{saved}");
        let again = Config::parse(&saved);
        assert!(again.issues.is_empty(), "{:?}", again.issues);
        assert_eq!(again.config, loaded.config);
    }

    #[test]
    fn a_provider_of_an_unknown_kind_is_kept_and_does_not_reset_the_ai_section() {
        let text = format!(
            "{TODAYS_FILE}\n[[ai.providers]]\nid = \"later\"\nname = \"Later\"\nkind = \"future_thing\"\nendpoint = \"x://y\"\nretries = 3\n"
        );
        let loaded = Config::parse(&text);
        assert!(loaded.issues.is_empty(), "{:?}", loaded.issues);
        let ai = &loaded.config.ai;
        assert!(ai.enabled, "the section did not fall back to the default");
        assert_eq!(ai.providers.len(), 3);
        assert_eq!(ai.style_rules.rules.len(), 2);
        assert_eq!(ai.unknown_providers.len(), 1);
        assert_eq!(ai.unknown_providers[0].get("kind").and_then(toml::Value::as_str), Some("future_thing"));
        assert_eq!(ai.unknown_providers[0].get("retries").and_then(toml::Value::as_integer), Some(3));

        let saved = loaded.config.to_toml();
        let providers = ai_table(&saved);
        let kinds = kinds_of(providers.get("providers"));
        assert_eq!(kinds, ["open_ai_compatible", "claude_cli", "apple_intelligence", "future_thing"]);
        let again = Config::parse(&saved);
        assert!(again.issues.is_empty(), "{:?}", again.issues);
        assert_eq!(again.config, loaded.config);
    }

    #[test]
    fn a_cleanup_model_with_an_empty_id_or_model_is_skipped() {
        let text = r#"
[ai]
enabled = true

[[ai.cleanup_models]]
id = ""
name = "No id"
model = "s1-mini"

[[ai.cleanup_models]]
id = "no-model"
name = "No model"

[[ai.cleanup_models]]
id = "cleanup-1"
name = "Cleanup 1"
model = "spokenly-cleanup-1"
"#;
        let loaded = Config::parse(text);
        assert_eq!(loaded.config.ai.providers.len(), 1);
        assert_eq!(loaded.config.ai.providers[0].kind, ProviderKind::CleanupModel { model: "spokenly-cleanup-1".into() });
        assert_eq!(loaded.config.ai.providers[0].model(), Some("spokenly-cleanup-1"));
    }

    #[test]
    fn the_file_before_the_first_cleanup_model_is_copied_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let backup = dir.path().join("config.toml.before-cleanup-models");
        std::fs::write(&path, TODAYS_FILE).unwrap();

        let config = config_with_cleanup_model();
        config.save(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), TODAYS_FILE);
        assert!(std::fs::read_to_string(&path).unwrap().contains("[[ai.cleanup_models]]"));

        // The file has cleanup models now, and a second save leaves the copy as it is.
        let mut changed = config.clone();
        changed.ai.http_timeout_ms = 7000;
        changed.save(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), TODAYS_FILE);
    }

    #[test]
    fn the_copy_is_not_replaced_when_a_later_file_has_no_cleanup_models_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let backup = dir.path().join("config.toml.before-cleanup-models");
        std::fs::write(&path, TODAYS_FILE).unwrap();
        let config = config_with_cleanup_model();
        config.save(&path).unwrap();

        // An older Sayso saved the file and dropped the cleanup models.
        std::fs::write(&path, "[ai]\nenabled = true\n").unwrap();
        config.save(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), TODAYS_FILE);
    }

    #[test]
    fn a_save_without_cleanup_models_makes_no_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, TODAYS_FILE).unwrap();
        let loaded = Config::parse(TODAYS_FILE);
        loaded.config.save(&path).unwrap();
        assert!(!dir.path().join("config.toml.before-cleanup-models").exists());
    }

    #[test]
    fn a_first_save_with_cleanup_models_and_no_old_file_makes_no_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        config_with_cleanup_model().save(&path).unwrap();
        assert!(path.exists());
        assert!(!dir.path().join("config.toml.before-cleanup-models").exists());
    }

    #[test]
    fn a_cleanup_model_has_no_thinking_no_model_choice_and_stays_on_this_computer() {
        let mut provider = cleanup_model("s1-mini", "S1-mini");
        assert_eq!(provider.kind.name(), "cleanup_model");
        assert_eq!(provider.thinking(), None);
        assert_eq!(provider.thinking_sent(), None);
        assert_eq!(provider.config_dir(), None);
        assert!(!provider.is_cloud());
        assert!(!provider.needs_model());
        provider.set_thinking(Some("high".into()));
        provider.set_model("other".into());
        provider.fit_thinking(&["low".to_string()]);
        assert_eq!(provider, cleanup_model("s1-mini", "S1-mini"));
    }
}
