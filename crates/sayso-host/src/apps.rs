//! `apps.toml`: the answers of the user to the approval question
//! (`docs/sdk.md`, "The question"), and the changes of Settings. It replaces
//! `extensions/state.toml`.

use crate::identity::{Caller, Identity, clean_name};
use parking_lot::Mutex;
use sayso_protocol::Capability;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use toml::value::Datetime;

/// The apps that have an answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Apps {
    #[serde(default)]
    pub apps: Vec<AppRecord>,
}

/// The answer of the user for one identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppRecord {
    /// The name from the system, which Sayso shows.
    pub name: String,
    /// macOS, signed apps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    /// The organization on the certificate, which Sayso shows for a signed app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>,
    /// Other identities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// **Allow**. A record without it is a **Don't allow**.
    #[serde(default)]
    pub allowed: bool,
    #[serde(default)]
    pub granted: Vec<String>,
    /// Capabilities that the user refused. The host does not ask for them again.
    #[serde(default)]
    pub denied: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seen: Option<Datetime>,
}

impl AppRecord {
    /// The identity of this record. None for a record that names no program:
    /// it matches nothing.
    pub fn identity(&self) -> Option<Identity> {
        match (&self.team_id, &self.identifier, &self.path) {
            (Some(team_id), Some(identifier), None) => Some(Identity::Signed { team_id: team_id.clone(), identifier: identifier.clone() }),
            (None, None, Some(path)) => Some(Identity::Path(path.clone())),
            _ => None,
        }
    }

    /// The key of the app in the host, as [`Identity::id`].
    pub fn id(&self) -> Option<String> {
        self.identity().map(|identity| identity.id())
    }

    /// The name to show. It comes from a file that the user can edit, so it
    /// gets the same cleaning as the name from the system.
    pub fn shown_name(&self) -> String {
        clean_name(&self.name).unwrap_or_else(|| "?".into())
    }

    /// The capabilities that the app has now. `models.read` needs no grant.
    /// Unknown names are left out. A capability that is also in `denied` is denied.
    pub fn capabilities(&self) -> Vec<Capability> {
        let mut capabilities = vec![Capability::ModelsRead];
        let denied = self.denied_capabilities();
        for capability in known(&self.granted) {
            if !capabilities.contains(&capability) && !denied.contains(&capability) {
                capabilities.push(capability);
            }
        }
        capabilities
    }

    /// The capabilities that the user refused.
    pub fn denied_capabilities(&self) -> Vec<Capability> {
        known(&self.denied)
    }

    fn has_answer(&self, capability: &Capability) -> bool {
        self.granted.iter().chain(&self.denied).any(|name| name == capability.as_str())
    }
}

fn known(names: &[String]) -> Vec<Capability> {
    names.iter().map(|name| Capability::parse(name)).filter(|capability| !matches!(capability, Capability::Other(_) | Capability::ModelsRead)).collect()
}

/// The capabilities of `asked` that need an answer of the user: known names,
/// without `models.read`, once each.
pub(crate) fn askable(asked: &[Capability]) -> Vec<Capability> {
    let mut capabilities: Vec<Capability> = Vec::new();
    for capability in asked {
        if !matches!(capability, Capability::Other(_) | Capability::ModelsRead) && !capabilities.contains(capability) {
            capabilities.push(capability.clone());
        }
    }
    capabilities
}

/// The capabilities of `asked` that `record` has no answer for.
pub(crate) fn unanswered(record: &AppRecord, asked: &[Capability]) -> Vec<Capability> {
    askable(asked).into_iter().filter(|capability| !record.has_answer(capability)).collect()
}

impl Apps {
    pub fn parse(text: &str) -> Result<Apps, toml::de::Error> {
        toml::from_str(text)
    }

    /// The record of `identity`.
    pub fn find(&self, identity: &Identity) -> Option<&AppRecord> {
        self.record(&identity.id())
    }

    /// The record with this key.
    pub fn record(&self, id: &str) -> Option<&AppRecord> {
        self.apps.iter().find(|record| record.id().as_deref() == Some(id))
    }

    /// At least one app is approved: the Models page shows **Voice detection** and **Speakers**.
    pub fn any_approved(&self) -> bool {
        self.apps.iter().any(|record| record.allowed && record.identity().is_some())
    }

    fn record_mut(&mut self, id: &str) -> Option<&mut AppRecord> {
        self.apps.iter_mut().find(|record| record.id().as_deref() == Some(id))
    }
}

type Observer = Arc<dyn Fn() + Send + Sync>;

struct Cache {
    bytes: Option<Vec<u8>>,
    apps: Arc<Apps>,
    broken: bool,
    /// The last read failed, so `bytes` says nothing about the file.
    unread: bool,
}

/// `apps.toml` on disk. [`AppsFile::current`] reads it again when its bytes
/// change, so a grant that the user takes back stops the next request. The
/// host and Settings write through this one value.
pub struct AppsFile {
    path: PathBuf,
    cache: Mutex<Cache>,
    /// One writer at a time, from reading the file to the rename.
    write: Mutex<()>,
    observer: Mutex<Option<Observer>>,
}

impl AppsFile {
    pub fn new(path: PathBuf) -> AppsFile {
        AppsFile { path, cache: Mutex::new(Cache { bytes: None, apps: Arc::new(Apps::default()), broken: false, unread: false }), write: Mutex::new(()), observer: Mutex::new(None) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The answers now. A missing file means no answers yet. A file that does
    /// not parse logs a warning and means no apps ([`AppsFile::is_broken`]).
    pub fn current(&self) -> Arc<Apps> {
        self.load().0
    }

    /// True when the file does not parse. The host then lets no app connect
    /// and asks nothing, because an answer could not be kept, and Sayso never
    /// writes over a file that it cannot read. Settings says so.
    pub fn is_broken(&self) -> bool {
        self.load().1
    }

    fn load(&self) -> (Arc<Apps>, bool) {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => {
                log::warn!("host: cannot read {}: {e}", self.path.display());
                let mut cache = self.cache.lock();
                *cache = Cache { bytes: None, apps: Arc::new(Apps::default()), broken: true, unread: true };
                return (cache.apps.clone(), true);
            }
        };
        let mut cache = self.cache.lock();
        if cache.bytes == bytes && !cache.unread {
            return (cache.apps.clone(), cache.broken);
        }
        let (apps, broken) = match &bytes {
            None => (Apps::default(), false),
            Some(bytes) => match std::str::from_utf8(bytes).map_err(|e| e.to_string()).and_then(|text| Apps::parse(text).map_err(|e| e.to_string())) {
                Ok(apps) => (apps, false),
                Err(e) => {
                    log::warn!("host: {} is broken, so no app can connect: {e}", self.path.display());
                    (Apps::default(), true)
                }
            },
        };
        *cache = Cache { bytes, apps: Arc::new(apps), broken, unread: false };
        (cache.apps.clone(), broken)
    }

    /// Call `observer` after each write. It runs on the thread of the writer and must not block.
    pub fn set_observer(&self, observer: Observer) {
        *self.observer.lock() = Some(observer);
    }

    fn update(&self, change: impl FnOnce(&mut Apps)) -> io::Result<()> {
        {
            let _writing = self.write.lock();
            let (current, broken) = self.load();
            if broken {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{} does not parse", self.path.display())));
            }
            let mut apps = (*current).clone();
            change(&mut apps);
            if apps == *current {
                return Ok(());
            }
            let text = toml::to_string_pretty(&apps).map_err(io::Error::other)?;
            if let Some(folder) = self.path.parent() {
                std::fs::create_dir_all(folder)?;
            }
            let temporary = self.path.with_extension(format!("toml.{}.tmp", std::process::id()));
            // On the disk before the rename, and the rename on the disk before
            // the answer counts: a crash must not leave an empty file, which
            // means no answers and forgets each refusal.
            let written = write_synced(&temporary, text.as_bytes()).and_then(|()| std::fs::rename(&temporary, &self.path));
            if let Err(e) = written {
                let _ = std::fs::remove_file(&temporary);
                return Err(e);
            }
            sync_folder(self.path.parent())?;
        }
        let observer = self.observer.lock().clone();
        if let Some(observer) = observer {
            observer();
        }
        Ok(())
    }

    /// The answer to the first question of `caller`, about `capabilities`.
    pub(crate) fn answer_first(&self, caller: &Caller, capabilities: &[Capability], allow: bool) -> io::Result<()> {
        let names: Vec<String> = capabilities.iter().map(ToString::to_string).collect();
        let (team_id, identifier, path) = match &caller.identity {
            Identity::Signed { team_id, identifier } => (Some(team_id.clone()), Some(identifier.clone()), None),
            Identity::Path(path) => (None, None, Some(path.clone())),
        };
        let record = AppRecord {
            name: caller.name.clone(),
            team_id,
            identifier,
            signer: caller.signer.clone(),
            path,
            allowed: allow,
            granted: if allow { names.clone() } else { Vec::new() },
            denied: if allow { Vec::new() } else { names },
            first_seen: now(),
        };
        self.update(|apps| {
            if apps.find(&caller.identity).is_none() {
                apps.apps.push(record);
            }
        })
    }

    /// The answer to a question about new capabilities of an approved app.
    pub(crate) fn answer_new(&self, id: &str, capabilities: &[Capability], allow: bool) -> io::Result<()> {
        self.update(|apps| {
            let Some(record) = apps.record_mut(id) else { return };
            for capability in capabilities {
                if !record.has_answer(capability) {
                    let list = if allow { &mut record.granted } else { &mut record.denied };
                    list.push(capability.to_string());
                }
            }
        })
    }

    /// A switch of Settings. The capability moves between `granted` and `denied`.
    pub fn set_grant(&self, id: &str, capability: &Capability, on: bool) -> io::Result<()> {
        self.update(|apps| {
            let Some(record) = apps.record_mut(id) else { return };
            let name = capability.as_str();
            record.granted.retain(|known| known != name);
            record.denied.retain(|known| known != name);
            let list = if on { &mut record.granted } else { &mut record.denied };
            list.push(name.to_string());
        })
    }

    /// **Allow** in Settings, for an app that the user refused: it gets the
    /// capabilities that it asked for, and the user can turn each one off.
    pub fn allow(&self, id: &str) -> io::Result<()> {
        self.update(|apps| {
            let Some(record) = apps.record_mut(id) else { return };
            record.allowed = true;
            let denied = std::mem::take(&mut record.denied);
            record.granted.extend(denied);
        })
    }

    /// **Remove** and **Forget** in Settings. At its next `hello`, Sayso asks again.
    pub fn remove(&self, id: &str) -> io::Result<()> {
        self.update(|apps| apps.apps.retain(|record| record.id().as_deref() != Some(id)))
    }
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    io::Write::write_all(&mut file, bytes)?;
    file.sync_all()
}

/// Windows cannot open a folder as a file, and its rename needs no such step.
fn sync_folder(folder: Option<&Path>) -> io::Result<()> {
    #[cfg(unix)]
    if let Some(folder) = folder {
        std::fs::File::open(folder)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = folder;
    Ok(())
}

/// The time now, for `first_seen`.
fn now() -> Option<Datetime> {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
[[apps]]
name = "Notetaker"            # The name from the system, which Sayso shows.
team_id = "ABCDE12345"        # macOS, signed apps.
identifier = "app.justsayso.notetaker"
allowed = true
granted = ["session.live", "audio.mic", "audio.system", "transcribe.file"]
denied = ["dictation.observe"]
first_seen = 2026-10-29T10:00:00Z

[[apps]]
name = "transcribe"
path = "/opt/transcribe/bin/transcribe"
allowed = false
denied = ["transcribe.file"]
"#;

    fn signed() -> Identity {
        Identity::Signed { team_id: "ABCDE12345".into(), identifier: "app.justsayso.notetaker".into() }
    }

    fn caller(path: &str) -> Caller {
        Caller::from_path(path.into())
    }

    #[test]
    fn the_record_of_the_docs_parses() {
        let apps = Apps::parse(EXAMPLE).unwrap();
        let record = apps.find(&signed()).unwrap();
        assert_eq!(record.name, "Notetaker");
        assert!(record.allowed);
        assert_eq!(record.capabilities(), vec![Capability::ModelsRead, Capability::SessionLive, Capability::AudioMic, Capability::AudioSystem, Capability::TranscribeFile]);
        assert_eq!(record.denied_capabilities(), vec![Capability::DictationObserve]);
        assert_eq!(record.first_seen.as_ref().map(ToString::to_string).as_deref(), Some("2026-10-29T10:00:00Z"));
        let by_path = apps.find(&Identity::Path("/opt/transcribe/bin/transcribe".into())).unwrap();
        assert!(!by_path.allowed);
        assert!(apps.find(&Identity::Path("/opt/other".into())).is_none());
        assert!(apps.any_approved());
    }

    #[test]
    fn a_record_that_names_no_program_or_two_matches_nothing() {
        let apps = Apps::parse("[[apps]]\nname = \"x\"\nallowed = true\n\n[[apps]]\nname = \"y\"\nteam_id = \"T\"\nallowed = true\n\n[[apps]]\nname = \"z\"\nteam_id = \"T\"\nidentifier = \"i\"\npath = \"/x\"\nallowed = true\n").unwrap();
        assert!(apps.apps.iter().all(|record| record.identity().is_none()));
        assert!(!apps.any_approved());
    }

    #[test]
    fn a_record_without_an_answer_is_not_approved() {
        let apps = Apps::parse("[[apps]]\nname = \"x\"\npath = \"/x\"\ngranted = [\"audio.mic\"]\n").unwrap();
        assert!(!apps.apps[0].allowed);
    }

    #[test]
    fn models_read_needs_no_grant_and_unknown_names_are_left_out() {
        let apps = Apps::parse("[[apps]]\nname = \"x\"\npath = \"/x\"\nallowed = true\ngranted = [\"camera.read\", \"ai.enhance\", \"models.read\"]\n").unwrap();
        assert_eq!(apps.apps[0].capabilities(), vec![Capability::ModelsRead, Capability::AiEnhance]);
    }

    #[test]
    fn only_capabilities_without_an_answer_need_a_question() {
        let apps = Apps::parse(EXAMPLE).unwrap();
        let record = apps.find(&signed()).unwrap();
        let asked = [Capability::ModelsRead, Capability::AudioMic, Capability::DictationObserve, Capability::AiEnhance, Capability::Other("camera.read".into()), Capability::AiEnhance];
        assert_eq!(unanswered(record, &asked), vec![Capability::AiEnhance]);
    }

    #[test]
    fn the_file_is_read_again_when_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apps.toml");
        let file = AppsFile::new(path.clone());
        assert_eq!(*file.current(), Apps::default(), "a missing file means no answers");
        assert!(!file.is_broken());
        std::fs::write(&path, EXAMPLE).unwrap();
        assert_eq!(file.current().apps.len(), 2);
        std::fs::write(&path, EXAMPLE.replace(r#", "audio.mic""#, "")).unwrap();
        assert!(!file.current().find(&signed()).unwrap().capabilities().contains(&Capability::AudioMic));
        std::fs::write(&path, "[[apps]\nname = ").unwrap();
        assert_eq!(*file.current(), Apps::default(), "a broken file means no apps");
        assert!(file.is_broken());
        std::fs::remove_file(&path).unwrap();
        assert!(!file.is_broken());
    }

    #[test]
    fn a_file_that_cannot_be_read_is_broken_until_it_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apps.toml");
        // A folder at the path: the read fails with another error than "not found".
        std::fs::create_dir(&path).unwrap();
        let file = AppsFile::new(path.clone());
        assert!(file.is_broken());
        std::fs::remove_dir(&path).unwrap();
        assert!(!file.is_broken(), "no file means no answers yet");
    }

    #[test]
    fn a_capability_in_both_lists_is_denied() {
        let apps = Apps::parse("[[apps]]\nname = \"x\"\npath = \"/x\"\nallowed = true\ngranted = [\"audio.mic\", \"ai.enhance\"]\ndenied = [\"audio.mic\"]\n").unwrap();
        assert_eq!(apps.apps[0].capabilities(), vec![Capability::ModelsRead, Capability::AiEnhance]);
        assert_eq!(apps.apps[0].denied_capabilities(), vec![Capability::AudioMic]);
    }

    #[test]
    fn a_path_in_the_file_does_not_match_a_signed_identity() {
        let apps = Apps::parse("[[apps]]\nname = \"x\"\npath = \"ABCDE12345/app.justsayso.notetaker\"\nallowed = true\n").unwrap();
        assert!(apps.find(&signed()).is_none());
    }

    #[test]
    fn a_name_in_the_file_is_cleaned_when_it_shows() {
        let apps = Apps::parse("[[apps]]\nname = \"No\\u202Etes\\n\"\npath = \"/x\"\n\n[[apps]]\nname = \"\\u200B\"\npath = \"/y\"\n").unwrap();
        assert_eq!(apps.apps[0].shown_name(), "Notes");
        assert_eq!(apps.apps[1].shown_name(), "?");
    }

    #[test]
    fn a_write_leaves_the_whole_file_and_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apps.toml");
        let file = AppsFile::new(path.clone());
        file.answer_first(&caller("/opt/App"), &[Capability::AudioMic], false).unwrap();
        assert!(AppsFile::new(path).current().find(&Identity::Path("/opt/App".into())).is_some());
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(names, ["apps.toml"]);
    }

    #[test]
    fn a_broken_file_is_never_written_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apps.toml");
        std::fs::write(&path, "[[apps]\n").unwrap();
        let file = AppsFile::new(path.clone());
        assert!(file.answer_first(&caller("/x"), &[], true).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[[apps]\n");
    }

    #[test]
    fn answers_and_changes_go_to_the_file_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("apps.toml");
        let file = AppsFile::new(path.clone());
        let changes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = changes.clone();
        file.set_observer(Arc::new(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        let app = caller("/opt/App");
        let id = app.identity.id();
        file.answer_first(&app, &[Capability::AudioMic], true).unwrap();
        let again = AppsFile::new(path.clone());
        let read = again.current();
        let record = read.record(&id).unwrap();
        assert_eq!((record.allowed, record.granted.clone(), record.denied.clone()), (true, vec!["audio.mic".to_string()], vec![]));
        assert!(record.first_seen.is_some());

        file.answer_new(&id, &[Capability::AiEnhance], false).unwrap();
        file.answer_new(&id, &[Capability::TranscribeFile], true).unwrap();
        let record = file.current().record(&id).unwrap().clone();
        assert_eq!(record.capabilities(), vec![Capability::ModelsRead, Capability::AudioMic, Capability::TranscribeFile]);
        assert_eq!(record.denied_capabilities(), vec![Capability::AiEnhance]);

        file.set_grant(&id, &Capability::AudioMic, false).unwrap();
        file.set_grant(&id, &Capability::AiEnhance, true).unwrap();
        let record = file.current().record(&id).unwrap().clone();
        assert_eq!(record.capabilities(), vec![Capability::ModelsRead, Capability::TranscribeFile, Capability::AiEnhance]);
        assert_eq!(record.denied_capabilities(), vec![Capability::AudioMic]);

        file.remove(&id).unwrap();
        assert!(file.current().record(&id).is_none());
        assert_eq!(changes.load(std::sync::atomic::Ordering::SeqCst), 6);
    }

    #[test]
    fn allow_in_settings_grants_what_the_user_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = AppsFile::new(dir.path().join("apps.toml"));
        let app = caller("/opt/App");
        let id = app.identity.id();
        file.answer_first(&app, &[Capability::AudioMic, Capability::AiEnhance], false).unwrap();
        let record = file.current().record(&id).unwrap().clone();
        assert!(!record.allowed);
        assert_eq!(record.capabilities(), vec![Capability::ModelsRead]);
        assert!(!file.current().any_approved());
        file.allow(&id).unwrap();
        let record = file.current().record(&id).unwrap().clone();
        assert!(record.allowed);
        assert_eq!(record.capabilities(), vec![Capability::ModelsRead, Capability::AudioMic, Capability::AiEnhance]);
        assert!(file.current().any_approved());
    }
}
