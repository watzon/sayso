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
}

impl Default for General {
    fn default() -> Self {
        Self { launch_at_login: false, dock_icon_with_hub: true, name: None }
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
    pub toggle: Option<Hotkey>,
    pub push_to_talk: Option<Hotkey>,
    pub cancel: CancelMode,
    /// Two Esc presses within this window cancel.
    pub double_escape_ms: u64,
    pub paste_last: Option<Hotkey>,
    pub cycle_style: Option<Hotkey>,
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dictation {
    /// A language code such as "en" or "de", or "auto" to let the model detect
    /// the language. A model that does not support the language ignores it.
    pub language: String,
    /// A local model id, or "<provider id>:<model>" for a speech provider's model.
    pub model: ModelId,
    pub live_preview: bool,
    /// A push-to-talk press shorter than this is ignored as an accident.
    pub min_duration_ms: u64,
    /// Recording stops by itself after this long.
    pub max_duration_s: u64,
}

impl Default for Dictation {
    fn default() -> Self {
        Self { language: "en".into(), model: default_model(), live_preview: true, min_duration_ms: 300, max_duration_s: 600 }
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
}

impl Default for Insertion {
    fn default() -> Self {
        Self { restore_clipboard: true, restore_delay_ms: 450, type_in_apps: Vec::new(), smart_spacing: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Overlay {
    /// Show the small idle pill.
    pub idle_pill: bool,
    pub hide_in_fullscreen: bool,
    /// Show live preview text above the recording pill.
    pub show_preview: bool,
    /// The size of the overlay and the idle pill.
    pub size: OverlaySize,
}

impl Default for Overlay {
    fn default() -> Self {
        Self { idle_pill: true, hide_in_fullscreen: true, show_preview: true, size: OverlaySize::default() }
    }
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
    },
    ClaudeCli {
        #[serde(default)]
        path: Option<String>,
        #[serde(default = "default_claude_model")]
        model: String,
    },
    CodexCli {
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        model: Option<String>,
    },
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
        };
        model.filter(|m| !m.trim().is_empty())
    }

    pub fn set_model(&mut self, id: String) {
        match &mut self.kind {
            ProviderKind::OpenAiCompatible { model, .. } | ProviderKind::ClaudeCli { model, .. } => *model = id,
            ProviderKind::CodexCli { model, .. } => *model = Some(id),
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
    /// Days to keep text. None keeps it forever.
    pub keep_text_days: Option<u32>,
    pub save_audio: bool,
    /// Days to keep audio. When audio expires only the audio file is deleted.
    pub keep_audio_days: Option<u32>,
}

impl Default for History {
    fn default() -> Self {
        Self { keep_text_days: None, save_audio: true, keep_audio_days: Some(30) }
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
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self { theme: ThemeMode::System, ink: Ink::default(), paper_texture: true, reduce_motion: None, system_title_bar: true }
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
    pub message: String,
}

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: Config,
    pub issues: Vec<ConfigIssue>,
    /// The file did not exist. Sayso writes the defaults on first save.
    pub created: bool,
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
                        issues.push(ConfigIssue { field: "config.toml".into(), message: whole.message().to_string() });
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
                            message: format!("{} (using defaults for this section)", toml::de::Error::message(&e)),
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
        section!("advanced", advanced);
        section!("onboarding", onboarding);
        cfg
    }

    pub fn validate(&self) -> Vec<ConfigIssue> {
        let mut issues = Vec::new();
        let mut issue = |field: &str, message: String| issues.push(ConfigIssue { field: field.into(), message });
        if !(0.0..=1.0).contains(&self.sounds.volume) {
            issue("sounds.volume", "must be between 0.0 and 1.0".into());
        }
        if self.ai.http_timeout_ms < 500 || self.ai.cli_timeout_ms < 500 {
            issue("ai", "timeouts must be at least 500 ms".into());
        }
        if self.hotkeys.toggle.is_some() && self.hotkeys.toggle == self.hotkeys.push_to_talk {
            issue("hotkeys.push_to_talk", "is the same key as hotkeys.toggle".into());
        }
        if let Some(id) = &self.ai.default_provider
            && !self.ai.providers.iter().any(|p| &p.id == id) {
                issue("ai.default_provider", format!("no provider has the id \"{id}\""));
            }
        if crate::models::find_with(&self.speech, &self.dictation.model).is_none() {
            issue("dictation.model", format!("\"{}\" is not a known model", self.dictation.model));
        }
        if !crate::languages::is_valid(&self.dictation.language) {
            issue("dictation.language", format!("\"{}\" is not a language code Sayso knows. Use \"auto\" or a code such as \"en\"", self.dictation.language));
        }
        if self.speech.timeout_ms < 1000 {
            issue("speech.timeout_ms", "must be at least 1000 ms".into());
        }
        for (i, p) in self.speech.providers.iter().enumerate() {
            if p.id.is_empty() || p.id.contains(':') {
                issue("speech.providers", format!("the provider id \"{}\" must not be empty or contain a colon", p.id));
            }
            if self.speech.providers[..i].iter().any(|q| q.id == p.id) {
                issue("speech.providers", format!("two providers have the id \"{}\"", p.id));
            }
            if p.kind == SpeechProviderKind::OpenAiCompatible && p.base_url().is_none() {
                issue("speech.providers", format!("the provider \"{}\" needs a base_url", p.id));
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
    fn empty_file_gives_defaults() {
        let loaded = Config::parse("");
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.issues.is_empty());
    }

    #[test]
    fn defaults_match_the_plan() {
        let c = Config::default();
        let (toggle, paste_last) = if cfg!(target_os = "macos") { ("opt+space", "ctrl+cmd+v") } else { ("ctrl+alt+space", "ctrl+alt+v") };
        assert_eq!(c.hotkeys.toggle.unwrap().to_string(), toggle);
        assert_eq!(c.hotkeys.push_to_talk, None);
        assert_eq!(c.hotkeys.cancel, CancelMode::DoubleEscape);
        assert_eq!(c.hotkeys.paste_last.unwrap().to_string(), paste_last);
        assert_eq!(c.history.keep_text_days, None);
        assert_eq!(c.history.keep_audio_days, Some(30));
        assert!(c.overlay.idle_pill);
        assert!(!c.ai.enabled);
        assert_eq!(c.ai.http_timeout_ms, 4000);
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
            },
        });
        c.ai.default_provider = Some("openrouter".into());
        c.hotkeys.push_to_talk = Some("right_option".parse().unwrap());
        let back = Config::parse(&c.to_toml());
        assert!(back.issues.is_empty(), "{:?}", back.issues);
        assert_eq!(back.config, c);
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
        let mut codex = Provider { id: "c".into(), name: "Codex CLI".into(), kind: ProviderKind::CodexCli { path: None, model: None } };
        assert_eq!(codex.model(), None);
        assert!(!codex.needs_model(), "Codex uses its own default");
        codex.set_model("gpt-6.1-sol".into());
        assert_eq!(codex.model(), Some("gpt-6.1-sol"));
        let claude = Provider { id: "k".into(), name: "Claude CLI".into(), kind: ProviderKind::ClaudeCli { path: None, model: "haiku".into() } };
        assert_eq!(claude.model(), Some("haiku"));
        assert!(!claude.needs_model());
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
        let messages: Vec<_> = c.validate().into_iter().map(|i| i.message).collect();
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
