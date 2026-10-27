//! `state.toml`: the installed extensions and their grants
//! (`docs/extensions.md`, "Code"). Step 8 writes it. Until then, tests and
//! developers write it by hand.

use parking_lot::Mutex;
use sayso_protocol::Capability;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;

/// The installed extensions.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct State {
    #[serde(default)]
    pub extensions: Vec<Record>,
}

/// One installed extension.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Record {
    pub id: String,
    /// The name of the extension, from its manifest. Step 8 writes it. Sayso
    /// shows the id when it is missing.
    #[serde(default)]
    pub name: Option<String>,
    pub version: String,
    /// The folder that the executable of the extension must be in.
    pub path: PathBuf,
    #[serde(default)]
    pub sideloaded: bool,
    /// The capabilities that the user granted, by name.
    #[serde(default)]
    pub granted: Vec<String>,
    /// macOS, for an extension from the registry: the code signature that
    /// install checked.
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub bundle_id: Option<String>,
}

impl Record {
    /// The name to show to the user: the name, else the id.
    pub fn display_name(&self) -> &str {
        self.name.as_deref().map(str::trim).filter(|name| !name.is_empty()).unwrap_or(&self.id)
    }

    /// The granted capabilities that this version knows. `models.read` is
    /// always granted.
    pub fn capabilities(&self) -> Vec<Capability> {
        let mut capabilities = vec![Capability::ModelsRead];
        for name in &self.granted {
            let capability = Capability::parse(name);
            if !matches!(capability, Capability::Other(_)) && !capabilities.contains(&capability) {
                capabilities.push(capability);
            }
        }
        capabilities
    }
}

impl State {
    pub fn parse(text: &str) -> Result<State, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn record(&self, id: &str) -> Option<&Record> {
        self.extensions.iter().find(|record| record.id == id)
    }
}

/// `state.toml` on disk. [`StateFile::current`] reads it again when its bytes
/// change, so a grant that the user takes back stops the next request.
pub struct StateFile {
    path: PathBuf,
    cache: Mutex<(Option<Vec<u8>>, Arc<State>)>,
}

impl StateFile {
    pub fn new(path: PathBuf) -> StateFile {
        StateFile { path, cache: Mutex::new((None, Arc::new(State::default()))) }
    }

    /// The state now. A missing file means no extensions. A file that does not
    /// parse logs a warning and means no extensions.
    pub fn current(&self) -> Arc<State> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                log::warn!("host: cannot read {}: {e}", self.path.display());
                None
            }
        };
        let mut cache = self.cache.lock();
        if cache.0 == bytes {
            return cache.1.clone();
        }
        let state = match &bytes {
            None => State::default(),
            Some(bytes) => match std::str::from_utf8(bytes).map_err(|e| e.to_string()).and_then(|text| State::parse(text).map_err(|e| e.to_string())) {
                Ok(state) => state,
                Err(e) => {
                    log::warn!("host: {} is broken, so no extension can connect: {e}", self.path.display());
                    State::default()
                }
            },
        };
        *cache = (bytes, Arc::new(state));
        cache.1.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
[[extensions]]
id = "dev.example.notetaker"
version = "1.2.0"
# The folder that the executable of the extension must be in.
path = "/Users/me/Library/Application Support/sayso/extensions/dev.example.notetaker/1.2.0"
sideloaded = false
granted = ["models.read", "session.live", "audio.mic"]
# macOS, for an extension from the registry: the code signature that install checked.
team_id = "ABCDE12345"
bundle_id = "dev.example.notetaker"
"#;

    #[test]
    fn the_record_of_the_docs_parses() {
        let state = State::parse(EXAMPLE).unwrap();
        let record = state.record("dev.example.notetaker").unwrap();
        assert_eq!(record.version, "1.2.0");
        assert_eq!(record.team_id.as_deref(), Some("ABCDE12345"));
        assert_eq!(record.bundle_id.as_deref(), Some("dev.example.notetaker"));
        assert_eq!(record.capabilities(), vec![Capability::ModelsRead, Capability::SessionLive, Capability::AudioMic]);
        assert!(state.record("dev.example.other").is_none());
    }

    #[test]
    fn models_read_is_always_granted_and_unknown_names_are_left_out() {
        let record = Record { id: "x".into(), name: None, version: "1".into(), path: "/x".into(), sideloaded: true, granted: vec!["camera.read".into(), "ai.enhance".into()], team_id: None, bundle_id: None };
        assert_eq!(record.capabilities(), vec![Capability::ModelsRead, Capability::AiEnhance]);
    }

    #[test]
    fn the_name_to_show_is_the_name_else_the_id() {
        let state = State::parse(EXAMPLE).unwrap();
        assert_eq!(state.extensions[0].name, None);
        assert_eq!(state.extensions[0].display_name(), "dev.example.notetaker");
        let state = State::parse(&EXAMPLE.replace("version = \"1.2.0\"", "name = \"Notetaker\"\nversion = \"1.2.0\"")).unwrap();
        assert_eq!(state.extensions[0].display_name(), "Notetaker");
    }

    #[test]
    fn the_file_is_read_again_when_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        let file = StateFile::new(path.clone());
        assert_eq!(*file.current(), State::default(), "a missing file means no extensions");
        std::fs::write(&path, EXAMPLE).unwrap();
        assert_eq!(file.current().extensions.len(), 1);
        std::fs::write(&path, EXAMPLE.replace(r#", "audio.mic""#, "")).unwrap();
        assert_eq!(file.current().extensions[0].capabilities(), vec![Capability::ModelsRead, Capability::SessionLive]);
        std::fs::write(&path, "[[extensions]\nid = ").unwrap();
        assert_eq!(*file.current(), State::default(), "a broken file means no extensions");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(*file.current(), State::default());
    }
}
