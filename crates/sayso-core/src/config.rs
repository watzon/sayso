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
    /// Global keyboard access for push-to-talk and modifier-only toggle keys.
    pub fn needs_input_monitoring(&self) -> bool {
        self.push_to_talk.is_some() || self.toggle.and_then(Hotkey::modifier_keys).is_some()
    }

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
}

impl ProviderKind {
    /// The name of the kind, as `config.toml` has it.
    pub fn name(&self) -> &'static str {
        match self {
            ProviderKind::OpenAiCompatible { .. } => "open_ai_compatible",
            ProviderKind::ClaudeCli { .. } => "claude_cli",
            ProviderKind::CodexCli { .. } => "codex_cli",
            ProviderKind::AppleIntelligence => "apple_intelligence",
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
            ProviderKind::AppleIntelligence => None,
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
            ProviderKind::AppleIntelligence => false,
        }
    }
}

pub fn is_local_url(url: &str) -> bool {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split(['/', ':']).next().unwrap_or("");
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]" | "0.0.0.0") || host.ends_with(".local")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ai {
    /// Off until the user sets up a provider. Off means every style acts like Raw.
    pub enabled: bool,
    pub active_style: String,
    pub default_provider: Option<String>,
    pub providers: Vec<Provider>,
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
    #[error("{0} conflicts with hotkeys.toggle")]
    HotkeyConflictsWithToggle(Hotkey),
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
        if let Some(toggle) = self.hotkeys.toggle {
            for (field, hotkey) in [
                ("hotkeys.push_to_talk", self.hotkeys.push_to_talk),
                ("hotkeys.paste_last", self.hotkeys.paste_last),
                ("hotkeys.cycle_style", self.hotkeys.cycle_style),
                ("hotkeys.incognito", self.hotkeys.incognito),
            ] {
                if let Some(hotkey) = hotkey && hotkey.conflicts_with_toggle(toggle) {
                    let problem = if field == "hotkeys.push_to_talk" && hotkey == toggle {
                        ConfigProblem::PushToTalkIsToggle
                    } else {
                        ConfigProblem::HotkeyConflictsWithToggle(hotkey)
                    };
                    issue(field, problem);
                }
            }
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
        crate::style::write_atomic(path, &self.to_toml())
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
    fn validation_catches_modifier_toggle_conflicts() {
        let mut c = Config::default();
        c.hotkeys.toggle = Some("left_control+left_option".parse().unwrap());
        c.hotkeys.push_to_talk = Some("left_control".parse().unwrap());
        c.hotkeys.paste_last = c.hotkeys.toggle;
        c.hotkeys.cycle_style = c.hotkeys.toggle;
        c.hotkeys.incognito = c.hotkeys.toggle;
        let fields: Vec<_> = c.validate().into_iter().map(|i| i.field).collect();
        for field in ["hotkeys.push_to_talk", "hotkeys.paste_last", "hotkeys.cycle_style", "hotkeys.incognito"] {
            assert!(fields.iter().any(|f| f == field), "missing conflict for {field}");
        }
    }

    #[test]
    fn modifier_toggle_allows_chords_and_reports_conflicts_loaded_from_file() {
        let valid = Config::parse("[hotkeys]\ntoggle = 'left_control'\npush_to_talk = 'ctrl+opt+d'\n");
        assert!(valid.issues.is_empty());
        let invalid = Config::parse("[hotkeys]\ntoggle = 'left_control'\npush_to_talk = 'left_control+left_option'\n");
        assert_eq!(invalid.issues.len(), 1);
        assert_eq!(invalid.issues[0].field, "hotkeys.push_to_talk");
        assert!(matches!(invalid.issues[0].problem, ConfigProblem::HotkeyConflictsWithToggle(_)));
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
}
