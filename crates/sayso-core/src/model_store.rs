//! The model store: the rules of `docs/sdk.md`, "Model store".
//!
//! Each artifact is a folder that never changes. A manifest for each model and
//! format names its artifacts. Engines of different programs share one store,
//! so each step that changes it holds file locks, and a process that stops
//! lets its locks go:
//!
//! - `locks/meta`: shared to read manifests, exclusive to change them or to
//!   decide what is garbage. It is held for short steps only.
//! - `locks/<digest>.get`: exclusive while one engine downloads the artifact.
//! - `locks/<digest>.use`: shared while a model is loaded, exclusive to delete.
//!
//! The order is `get`, `meta`, `use`, and locks of one kind go in the order of
//! the digests. Nobody waits for a lock while it holds a lock that comes later.
//!
//! This file has the rules and no network: an engine gives [`Store::download`]
//! a function that fetches the files of one artifact.

use crate::engine_catalog::descriptors::{Artifact, Descriptor, Descriptors};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The folder of the store in the models folder of Sayso.
pub const FOLDER: &str = "v1";

/// A store folder for an engine. An engine without it keeps its models in
/// `SAYSO_MODELS_DIR`, and an engine of an older version ignores it.
pub const STORE_ENV: &str = "SAYSO_MODEL_STORE";

/// A folder that makes the engine wait at the steps of the store, for tests
/// (`docs/engine-protocol.md`, "Conformance tests").
pub const HOOK_ENV: &str = "SAYSO_STORE_TEST_HOOK";

/// A file with descriptors in place of the built-in ones, for tests.
pub const DESCRIPTORS_ENV: &str = "SAYSO_STORE_TEST_DESCRIPTORS";

/// The error of a fetch function.
pub type FetchError = Box<dyn std::error::Error + Send + Sync>;

/// Gets an artifact, an empty work folder, and the path that must hold the files
/// of the artifact when it returns.
pub type Fetch<'a> = dyn FnMut(&Artifact, &Path, &Path) -> Result<(), FetchError> + 'a;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{what}: {source}")]
    Io { what: String, source: std::io::Error },
    #[error("{0:?} is not a plain name")]
    Name(String),
    /// The fetch function failed. A later download starts this artifact again.
    #[error("{0}")]
    Fetch(FetchError),
    /// The fetched files are not the files of the descriptor.
    #[error("{0}")]
    Verify(String),
}

pub(crate) fn io(what: impl std::fmt::Display, path: &Path) -> impl FnOnce(std::io::Error) -> Error {
    let what = format!("{what} {}", path.display());
    move |source| Error::Io { what, source }
}

/// What a manifest says: `models/<model id>/<format>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub revision: u32,
    pub parts: Vec<ManifestPart>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestPart {
    pub role: String,
    pub digest: String,
}

/// A model that is loaded: the folder of each role and the shared `use` locks.
/// Keep it until nothing reads the files.
#[derive(Debug)]
pub struct Lease {
    folders: BTreeMap<String, PathBuf>,
    _locks: Vec<File>,
}

impl Lease {
    /// The folder of the artifact with this role.
    pub fn folder(&self, role: &str) -> Option<&Path> {
        self.folders.get(role).map(PathBuf::as_path)
    }
}

/// The store at one folder.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
    hook: Option<PathBuf>,
}

/// A file lock. It goes when the file closes.
fn open_lock(path: &Path) -> Result<File, Error> {
    File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(io("cannot open the lock", path))
}

impl Store {
    /// Open the store at `root`, and make its folders.
    pub fn open(root: PathBuf) -> Result<Store, Error> {
        let hook = std::env::var_os(HOOK_ENV).filter(|v| !v.is_empty()).map(PathBuf::from);
        Store::with_hook(root, hook)
    }

    fn with_hook(root: PathBuf, hook: Option<PathBuf>) -> Result<Store, Error> {
        for folder in ["artifacts", "models", "locks", "staging", "trash"] {
            let path = root.join(folder);
            std::fs::create_dir_all(&path).map_err(io("cannot create", &path))?;
        }
        Ok(Store { root, hook })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A step of the rules. With the test hook, it writes `at-<step>` in the
    /// folder of the hook. With the file `hold-<step>` there, it then waits
    /// until that file is gone.
    pub fn step(&self, name: &str) {
        let Some(dir) = &self.hook else { return };
        let _ = std::fs::write(dir.join(format!("at-{name}")), b"");
        let hold = dir.join(format!("hold-{name}"));
        while hold.exists() {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub(crate) fn artifact(&self, digest: &str) -> PathBuf {
        self.root.join("artifacts").join(digest)
    }

    fn lock_path(&self, name: &str) -> PathBuf {
        self.root.join("locks").join(name)
    }

    fn meta_shared(&self) -> Result<File, Error> {
        let path = self.lock_path("meta");
        let lock = open_lock(&path)?;
        lock.lock_shared().map_err(io("cannot lock", &path))?;
        Ok(lock)
    }

    pub(crate) fn meta_exclusive(&self) -> Result<File, Error> {
        let path = self.lock_path("meta");
        let lock = open_lock(&path)?;
        lock.lock().map_err(io("cannot lock", &path))?;
        Ok(lock)
    }

    fn manifest_path(&self, model: &str, format: &str) -> Result<PathBuf, Error> {
        let plain = |name: &str| !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', ':']);
        for name in [model, format] {
            if !plain(name) {
                return Err(Error::Name(name.into()));
            }
        }
        Ok(self.root.join("models").join(model).join(format!("{format}.json")))
    }

    /// Hold `meta` when you call this. A file that does not parse is no manifest.
    pub(crate) fn read_manifest(&self, model: &str, format: &str) -> Result<Option<Manifest>, Error> {
        let path = self.manifest_path(model, format)?;
        match std::fs::read(&path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io("cannot read", &path)(e)),
        }
    }

    /// Hold `meta` when you call this. The revision is enough and each artifact exists.
    pub(crate) fn complete(&self, manifest: &Manifest, descriptor: &Descriptor) -> bool {
        manifest.revision >= descriptor.revision && manifest.parts.iter().all(|p| self.artifact(&p.digest).is_dir())
    }

    /// True when the model is on disk: its manifest has the revision of the
    /// descriptor or a higher one, and each artifact exists.
    pub fn on_disk(&self, descriptor: &Descriptor) -> Result<bool, Error> {
        let _meta = self.meta_shared()?;
        Ok(self.read_manifest(&descriptor.model, &descriptor.format)?.is_some_and(|m| self.complete(&m, descriptor)))
    }

    /// [`Store::on_disk`] without a wait: None when another engine has `meta` now.
    pub fn on_disk_now(&self, descriptor: &Descriptor) -> Result<Option<bool>, Error> {
        let path = self.lock_path("meta");
        let meta = open_lock(&path)?;
        match meta.try_lock_shared() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(e)) => return Err(io("cannot lock", &path)(e)),
        }
        let manifest = self.read_manifest(&descriptor.model, &descriptor.format)?;
        Ok(Some(manifest.is_some_and(|m| self.complete(&m, descriptor))))
    }

    /// "Load", steps 1 to 3: the folders of the model, locked for use. None when
    /// the model is not on disk. It never writes in the store, apart from the
    /// lock files.
    pub fn load(&self, descriptor: &Descriptor) -> Result<Option<Lease>, Error> {
        let meta = self.meta_shared()?;
        let Some(manifest) = self.read_manifest(&descriptor.model, &descriptor.format)? else {
            return Ok(None);
        };
        self.step("load.manifest_read");
        if manifest.revision < descriptor.revision {
            return Ok(None);
        }
        let digests: BTreeSet<&str> = manifest.parts.iter().map(|p| p.digest.as_str()).collect();
        let mut locks = Vec::with_capacity(digests.len());
        for digest in digests {
            let path = self.lock_path(&format!("{digest}.use"));
            let lock = open_lock(&path)?;
            lock.lock_shared().map_err(io("cannot lock", &path))?;
            locks.push(lock);
        }
        self.step("load.use_locked");
        if !self.complete(&manifest, descriptor) {
            return Ok(None);
        }
        let folders = manifest.parts.iter().map(|p| (p.role.clone(), self.artifact(&p.digest))).collect();
        drop(meta);
        self.step("load.ready");
        Ok(Some(Lease { folders, _locks: locks }))
    }

    /// "Download". `waiting` runs when another engine downloads an artifact that
    /// this model needs.
    pub fn download(
        &self,
        data: &Descriptors,
        descriptor: &Descriptor,
        fetch: &mut Fetch<'_>,
        waiting: &mut dyn FnMut(),
    ) -> Result<(), Error> {
        let mut artifacts: Vec<&Artifact> = data.parts(descriptor).map(|(_, artifact)| artifact).collect();
        artifacts.sort_by(|a, b| a.digest.cmp(&b.digest));
        artifacts.dedup_by(|a, b| a.digest == b.digest);
        loop {
            // Step 1.
            let missing: Vec<&Artifact> = {
                let _meta = self.meta_shared()?;
                let manifest = self.read_manifest(&descriptor.model, &descriptor.format)?;
                if manifest.is_some_and(|m| self.complete(&m, descriptor)) {
                    return Ok(());
                }
                artifacts.iter().copied().filter(|a| !self.artifact(&a.digest).is_dir()).collect()
            };
            self.step("download.manifest_read");
            // Step 2. The `get` locks stay until the artifacts are published.
            let mut gets = Vec::with_capacity(missing.len());
            let mut staged = Vec::with_capacity(missing.len());
            for artifact in missing {
                gets.push(self.lock_get(&artifact.digest, waiting)?);
                self.step("download.get_locked");
                let exists = {
                    let _meta = self.meta_shared()?;
                    self.artifact(&artifact.digest).is_dir()
                };
                if !exists {
                    match self.fetch(artifact, fetch) {
                        Ok(stage) => staged.push(stage),
                        Err(e) => {
                            // A retry starts again, so a stage of this attempt is only in the way.
                            for (_, stage) in staged {
                                let _ = std::fs::remove_dir_all(stage);
                            }
                            return Err(e);
                        }
                    }
                }
            }
            // Step 3.
            let meta = self.meta_exclusive()?;
            self.step("download.meta_locked");
            for (digest, stage) in staged {
                let dest = self.artifact(&digest);
                if !dest.is_dir() {
                    rename(&stage.join("files"), &dest)?;
                }
                let _ = std::fs::remove_dir_all(&stage);
                self.step("download.artifact_published");
            }
            sync_folder(&self.root.join("artifacts"));
            // A cleanup can take an artifact that no manifest named after step 1.
            // Then this goes back to step 1, with what it has published, and the
            // `get` locks go in the right order again.
            if artifacts.iter().all(|a| self.artifact(&a.digest).is_dir()) {
                self.write_manifest(descriptor, data)?;
                self.step("download.manifest_written");
                // A manifest with a higher revision stays, and it can name an
                // artifact that a power cut or a person took away.
                let manifest = self.read_manifest(&descriptor.model, &descriptor.format)?;
                if !manifest.is_some_and(|m| self.complete(&m, descriptor)) {
                    return Err(Error::Verify(format!(
                        "the manifest of {} has a higher revision and names a missing artifact; delete the model",
                        descriptor.model
                    )));
                }
                drop(meta);
                drop(gets);
                self.step("download.done");
                return Ok(());
            }
        }
    }

    fn lock_get(&self, digest: &str, waiting: &mut dyn FnMut()) -> Result<File, Error> {
        let path = self.lock_path(&format!("{digest}.get"));
        let lock = open_lock(&path)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                waiting();
                lock.lock().map_err(io("cannot lock", &path))?;
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(io("cannot lock", &path)(e)),
        }
        Ok(lock)
    }

    /// Step 2 for one artifact: a staging folder, the `owner` file, the files,
    /// and the check against the descriptor. The caller holds the `get` lock.
    /// Returns the digest and the staging folder.
    fn fetch(
        &self,
        artifact: &Artifact,
        fetch: &mut Fetch<'_>,
    ) -> Result<(String, PathBuf), Error> {
        let name = format!("{}.{}.{}", artifact.digest, std::process::id(), uuid::Uuid::new_v4().simple());
        let stage = self.root.join("staging").join(name);
        let work = stage.join("work");
        std::fs::create_dir_all(&work).map_err(io("cannot create", &work))?;
        let owner = stage.join("owner");
        std::fs::write(&owner, format!("{}\n", std::process::id())).map_err(io("cannot write", &owner))?;
        self.step("download.staged");
        let files = stage.join("files");
        let result = fetch(artifact, &work, &files).map_err(Error::Fetch).and_then(|()| verify(&files, artifact));
        if let Err(e) = result {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(e);
        }
        self.step("download.fetched");
        Ok((artifact.digest.clone(), stage))
    }

    /// Hold `meta` exclusive when you call this. A manifest never goes back to
    /// a lower revision.
    pub(crate) fn write_manifest(&self, descriptor: &Descriptor, data: &Descriptors) -> Result<(), Error> {
        let existing = self.read_manifest(&descriptor.model, &descriptor.format)?;
        if existing.is_some_and(|m| m.revision > descriptor.revision) {
            return Ok(());
        }
        let manifest = Manifest {
            revision: descriptor.revision,
            parts: data
                .parts(descriptor)
                .map(|(role, artifact)| ManifestPart { role: role.to_string(), digest: artifact.digest.clone() })
                .collect(),
        };
        let path = self.manifest_path(&descriptor.model, &descriptor.format)?;
        let folder = path.parent().expect("a manifest has a folder");
        std::fs::create_dir_all(folder).map_err(io("cannot create", folder))?;
        let temp = folder.join(format!(".{}.{}.tmp", std::process::id(), uuid::Uuid::new_v4().simple()));
        let bytes = serde_json::to_vec_pretty(&manifest).expect("a manifest is JSON");
        let written = File::create(&temp).and_then(|mut file| {
            file.write_all(&bytes)?;
            file.sync_all()
        });
        written.map_err(io("cannot write", &temp))?;
        rename(&temp, &path)?;
        sync_folder(folder);
        Ok(())
    }

    /// "Delete a model", then "Clean up". This never fails because of another
    /// engine: an engine that has the model loaded keeps it until it unloads.
    pub fn delete(&self, model: &str, format: &str) -> Result<(), Error> {
        let path = self.manifest_path(model, format)?;
        {
            let _meta = self.meta_exclusive()?;
            self.step("delete.meta_locked");
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io("cannot delete", &path)(e)),
                _ => {}
            }
            self.step("delete.manifest_removed");
        }
        // The model is gone now. What a cleanup cannot do stays for the next one.
        if let Err(e) = self.cleanup() {
            log::warn!("the cleanup of the model store failed: {e}");
        }
        Ok(())
    }

    /// "Clean up". Run it at the start of an engine and after each delete. What
    /// it cannot remove now (another engine has the artifact loaded, or a file
    /// is busy on Windows) stays for the next cleanup.
    pub fn cleanup(&self) -> Result<(), Error> {
        {
            let _meta = self.meta_exclusive()?;
            self.step("cleanup.meta_locked");
            if let Some(named) = self.named_digests()? {
                for entry in folders(&self.root.join("artifacts"))? {
                    let digest = entry.file_name().to_string_lossy().into_owned();
                    if named.contains(&digest) {
                        continue;
                    }
                    let lock = open_lock(&self.lock_path(&format!("{digest}.use")))?;
                    if lock.try_lock().is_err() {
                        continue;
                    }
                    self.step("cleanup.use_locked");
                    let trash = self.root.join("trash").join(format!("{digest}.{}", uuid::Uuid::new_v4().simple()));
                    let _ = rename(&entry.path(), &trash);
                    self.step("cleanup.trashed");
                }
            }
        }
        self.step("cleanup.meta_released");
        for entry in read_dir(&self.root.join("trash"))? {
            let _ = std::fs::remove_dir_all(entry.path());
        }
        self.step("cleanup.trash_emptied");
        for entry in folders(&self.root.join("staging"))? {
            let name = entry.file_name().to_string_lossy().into_owned();
            let digest = name.split('.').next().unwrap_or_default();
            let lock = open_lock(&self.lock_path(&format!("{digest}.get")))?;
            if lock.try_lock().is_ok() {
                self.step("cleanup.staging_locked");
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        self.step("cleanup.done");
        Ok(())
    }

    /// Hold `meta` exclusive when you call this. The digests that the manifests
    /// name. It removes manifest files that a crash left half written, and the
    /// folder of a model without manifests. None when a manifest does not parse,
    /// so that it is not known what is garbage.
    fn named_digests(&self) -> Result<Option<HashSet<String>>, Error> {
        let mut named = HashSet::new();
        // A file that is no folder (`.DS_Store`, `desktop.ini`) is not a model.
        for model in folders(&self.root.join("models"))? {
            for file in read_dir(&model.path())? {
                let path = file.path();
                if file.file_name().to_string_lossy().starts_with('.') {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                let bytes = std::fs::read(&path).map_err(io("cannot read", &path))?;
                let Ok(manifest) = serde_json::from_slice::<Manifest>(&bytes) else {
                    return Ok(None);
                };
                named.extend(manifest.parts.into_iter().map(|p| p.digest));
            }
            let _ = std::fs::remove_dir(model.path());
        }
        Ok(Some(named))
    }
}

fn read_dir(path: &Path) -> Result<Vec<std::fs::DirEntry>, Error> {
    let entries = std::fs::read_dir(path).map_err(io("cannot read", path))?;
    entries.collect::<std::io::Result<_>>().map_err(io("cannot read", path))
}

/// The folders in `path`. The store ignores other files that programs leave there.
fn folders(path: &Path) -> Result<Vec<std::fs::DirEntry>, Error> {
    let mut entries = read_dir(path)?;
    entries.retain(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()));
    Ok(entries)
}

/// Make a rename in `folder` last through a power cut, where the system can do
/// that. A failure leaves the rename as good as the system made it.
pub(crate) fn sync_folder(folder: &Path) {
    #[cfg(unix)]
    if let Ok(file) = File::open(folder) {
        let _ = file.sync_all();
    }
    #[cfg(not(unix))]
    let _ = folder;
}

/// A rename. Windows can refuse for a moment while another program has a file
/// open (a virus scanner), so there it tries again for about two seconds.
pub(crate) fn rename(from: &Path, to: &Path) -> Result<(), Error> {
    let attempts = if cfg!(windows) { 20 } else { 1 };
    for attempt in 1.. {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(_) if attempt < attempts => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(io("cannot move", from)(e)),
        }
    }
    unreachable!("the loop returns")
}

/// Check that `folder` holds the files of `artifact` and nothing else.
pub(crate) fn verify(folder: &Path, artifact: &Artifact) -> Result<(), Error> {
    let mut found = Vec::new();
    walk(folder, "", &mut found)?;
    found.sort();
    let mut wanted: Vec<&str> = artifact.files.iter().map(|f| f.path.as_str()).collect();
    wanted.sort_unstable();
    if found != wanted {
        return Err(Error::Verify(format!("the files of {} are not the files of its descriptor: {found:?}", artifact.digest)));
    }
    for file in &artifact.files {
        let path = folder.join(&file.path);
        let size = std::fs::metadata(&path).map_err(io("cannot read", &path))?.len();
        if size != file.size {
            return Err(Error::Verify(format!("{} has {size} bytes, not {}", file.path, file.size)));
        }
        if hash_file(&path)? != file.sha256 {
            return Err(Error::Verify(format!("{} does not have the hash of its descriptor", file.path)));
        }
        // After a power cut, a published artifact must not have short files: only
        // the folder is checked at a load.
        let synced = File::options().write(true).open(&path).and_then(|f| f.sync_all());
        synced.map_err(io("cannot write", &path))?;
    }
    Ok(())
}

/// The paths of the files under `folder`, in `/` form. A link is an error.
fn walk(folder: &Path, prefix: &str, found: &mut Vec<String>) -> Result<(), Error> {
    for entry in read_dir(folder)? {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = format!("{prefix}{name}");
        let kind = entry.file_type().map_err(io("cannot read", &entry.path()))?;
        if kind.is_dir() {
            walk(&entry.path(), &format!("{path}/"), found)?;
        } else if kind.is_file() {
            found.push(path);
        } else {
            return Err(Error::Verify(format!("{path} is not a file or a folder")));
        }
    }
    Ok(())
}

/// The SHA-256 of a file, in lowercase hex.
pub fn hash_file(path: &Path) -> Result<String, Error> {
    let mut file = File::open(path).map_err(io("cannot read", path))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let n = file.read(&mut buffer).map_err(io("cannot read", path))?;
        if n == 0 {
            return Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect());
        }
        hasher.update(&buffer[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_catalog::descriptors::{self, File as ArtifactFile, Part, Source};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    fn artifact(files: &[(&str, &str)]) -> Artifact {
        let files: Vec<ArtifactFile> = files
            .iter()
            .map(|(path, text)| ArtifactFile { path: path.to_string(), size: text.len() as u64, sha256: sha(text.as_bytes()) })
            .collect();
        Artifact {
            source: Source::File { url: "http://unused".into() },
            digest: descriptors::digest(&files).unwrap(),
            files,
        }
    }

    /// A test catalog: `shared` is in the models `a` and `b`, and `only-a` and
    /// `only-b` in one each.
    fn data(revision: u32) -> Descriptors {
        let model = |model: &str, other: &str| Descriptor {
            model: model.into(),
            format: "test-1".into(),
            revision,
            parts: vec![
                Part { role: "main".into(), artifact: "shared".into() },
                Part { role: "extra".into(), artifact: other.into() },
            ],
        };
        Descriptors {
            artifacts: BTreeMap::from([
                ("shared".to_string(), artifact(&[("main.bin", "shared bytes"), ("sub/vocab.txt", "words")])),
                ("only-a".to_string(), artifact(&[("a.bin", "bytes of a")])),
                ("only-b".to_string(), artifact(&[("b.bin", "bytes of b")])),
            ]),
            descriptors: vec![model("a", "only-a"), model("b", "only-b")],
        }
    }

    /// The contents of the test artifacts, by the path of the file.
    fn contents(path: &str) -> &'static str {
        match path {
            "main.bin" => "shared bytes",
            "sub/vocab.txt" => "words",
            "a.bin" => "bytes of a",
            "b.bin" => "bytes of b",
            other => panic!("no contents for {other}"),
        }
    }

    fn put_files(artifact: &Artifact, files: &Path) -> Result<(), FetchError> {
        for file in &artifact.files {
            let path = files.join(&file.path);
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(path, contents(&file.path))?;
        }
        Ok(())
    }

    fn download(store: &Store, data: &Descriptors, model: &str) {
        let descriptor = data.find(model, "test-1").unwrap();
        store.download(data, descriptor, &mut |a, _, files| put_files(a, files), &mut || {}).unwrap();
    }

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::with_hook(dir.path().join("v1"), None).unwrap();
        (dir, store)
    }

    fn names(store: &Store, folder: &str) -> Vec<String> {
        let mut names: Vec<String> = read_dir(&store.root.join(folder))
            .unwrap()
            .into_iter()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn digest_of(data: &Descriptors, name: &str) -> String {
        data.artifacts[name].digest.clone()
    }

    #[test]
    fn a_download_publishes_the_artifacts_and_the_manifest() {
        let (_dir, store) = store();
        let data = data(1);
        let a = data.find("a", "test-1").unwrap();
        assert!(!store.on_disk(a).unwrap());
        assert!(store.load(a).unwrap().is_none());

        download(&store, &data, "a");

        assert!(store.on_disk(a).unwrap());
        assert!(names(&store, "staging").is_empty());
        let lease = store.load(a).unwrap().unwrap();
        let main = lease.folder("main").unwrap();
        assert_eq!(std::fs::read_to_string(main.join("sub/vocab.txt")).unwrap(), "words");
        assert!(!main.join("owner").exists());
        assert!(lease.folder("extra").unwrap().join("a.bin").is_file());
    }

    #[test]
    fn a_model_on_disk_needs_no_fetch() {
        let (_dir, store) = store();
        let data = data(1);
        download(&store, &data, "a");
        let a = data.find("a", "test-1").unwrap();
        store.download(&data, a, &mut |_, _, _| panic!("no fetch"), &mut || {}).unwrap();
    }

    #[test]
    fn two_downloads_of_a_model_fetch_it_once() {
        let (_dir, store) = store();
        let data = Arc::new(data(1));
        let fetches = Arc::new(AtomicUsize::new(0));
        let start = Arc::new(Barrier::new(2));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let (store, data, fetches, start) = (store.clone(), data.clone(), fetches.clone(), start.clone());
                std::thread::spawn(move || {
                    let a = data.find("a", "test-1").unwrap();
                    start.wait();
                    let mut fetch = |artifact: &Artifact, _: &Path, files: &Path| {
                        fetches.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(50));
                        put_files(artifact, files)
                    };
                    store.download(&data, a, &mut fetch, &mut || {}).unwrap();
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(fetches.load(Ordering::SeqCst), 2, "two artifacts, each once");
        assert!(store.on_disk(data.find("a", "test-1").unwrap()).unwrap());
    }

    #[test]
    fn a_shared_artifact_stays_when_the_other_model_goes() {
        let (_dir, store) = store();
        let data = data(1);
        download(&store, &data, "a");
        download(&store, &data, "b");
        store.delete("a", "test-1").unwrap();

        assert!(!store.on_disk(data.find("a", "test-1").unwrap()).unwrap());
        assert!(store.on_disk(data.find("b", "test-1").unwrap()).unwrap());
        assert!(store.artifact(&digest_of(&data, "shared")).is_dir());
        assert!(!store.artifact(&digest_of(&data, "only-a")).exists());
        assert!(names(&store, "trash").is_empty());

        store.delete("b", "test-1").unwrap();
        assert!(names(&store, "artifacts").is_empty());
        assert!(names(&store, "models").is_empty());
    }

    #[test]
    fn a_loaded_model_keeps_its_files_until_the_lease_goes() {
        let (_dir, store) = store();
        let data = data(1);
        download(&store, &data, "a");
        let a = data.find("a", "test-1").unwrap();
        let lease = store.load(a).unwrap().unwrap();

        store.delete("a", "test-1").unwrap();
        assert!(!store.on_disk(a).unwrap(), "the other programs see it gone");
        assert!(store.load(a).unwrap().is_none());
        assert!(lease.folder("main").unwrap().join("main.bin").is_file(), "it stays while it is loaded");

        drop(lease);
        store.cleanup().unwrap();
        assert!(names(&store, "artifacts").is_empty());
    }

    #[test]
    fn a_manifest_does_not_go_back_to_a_lower_revision() {
        let (_dir, store) = store();
        let (old, new) = (data(1), data(2));
        download(&store, &old, "a");
        let (old_a, new_a) = (old.find("a", "test-1").unwrap(), new.find("a", "test-1").unwrap());
        assert!(!store.on_disk(new_a).unwrap(), "a new descriptor wants a new revision");
        assert!(store.load(new_a).unwrap().is_none());

        download(&store, &new, "a");
        assert_eq!(store.read_manifest("a", "test-1").unwrap().unwrap().revision, 2);
        assert!(store.on_disk(old_a).unwrap(), "an older engine sees the model");

        // An older engine that started before the new manifest came publishes last.
        let _meta = store.meta_exclusive().unwrap();
        store.write_manifest(old_a, &old).unwrap();
        assert_eq!(store.read_manifest("a", "test-1").unwrap().unwrap().revision, 2);
    }

    #[test]
    fn a_failed_fetch_or_wrong_files_leave_nothing() {
        let (_dir, store) = store();
        let data = data(1);
        let a = data.find("a", "test-1").unwrap();

        let error = store.download(&data, a, &mut |_, _, _| Err("no network".into()), &mut || {}).unwrap_err();
        assert!(matches!(error, Error::Fetch(_)), "{error}");

        let mut wrong_bytes = |artifact: &Artifact, _: &Path, files: &Path| {
            std::fs::create_dir_all(files)?;
            for file in &artifact.files {
                let path = files.join(&file.path);
                std::fs::create_dir_all(path.parent().unwrap())?;
                std::fs::write(path, "x".repeat(file.size as usize))?;
            }
            Ok(())
        };
        let error = store.download(&data, a, &mut wrong_bytes, &mut || {}).unwrap_err();
        assert!(matches!(error, Error::Verify(_)), "{error}");

        let mut extra_file = |artifact: &Artifact, work: &Path, files: &Path| {
            put_files(artifact, files)?;
            std::fs::write(files.join("extra"), "x")?;
            std::fs::write(work.join("leftover"), "x")?;
            Ok(())
        };
        let error = store.download(&data, a, &mut extra_file, &mut || {}).unwrap_err();
        assert!(matches!(error, Error::Verify(_)), "{error}");

        assert!(names(&store, "staging").is_empty());
        assert!(names(&store, "artifacts").is_empty());
        assert!(names(&store, "models").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_files_is_refused() {
        let (_dir, store) = store();
        let data = data(1);
        let a = data.find("a", "test-1").unwrap();
        let mut with_link = |artifact: &Artifact, _: &Path, files: &Path| {
            put_files(artifact, files)?;
            std::os::unix::fs::symlink("/etc/passwd", files.join("link"))?;
            Ok(())
        };
        let error = store.download(&data, a, &mut with_link, &mut || {}).unwrap_err();
        assert!(matches!(error, Error::Verify(_)), "{error}");
    }

    #[test]
    fn cleanup_keeps_what_runs_and_removes_what_a_crash_left() {
        let (_dir, store) = store();
        let data = data(1);
        let (shared, only_a) = (digest_of(&data, "shared"), digest_of(&data, "only-a"));
        // An artifact without a manifest, and a staging folder of a stopped engine.
        std::fs::create_dir_all(store.artifact(&shared)).unwrap();
        let stopped = store.root.join("staging").join(format!("{only_a}.1.x"));
        std::fs::create_dir_all(stopped.join("files")).unwrap();
        // A staging folder of a download that runs, and a manifest that a crash left half written.
        let running_get = open_lock(&store.lock_path("running.get")).unwrap();
        running_get.lock().unwrap();
        let running = store.root.join("staging").join("running.2.y");
        std::fs::create_dir_all(&running).unwrap();
        let half = store.root.join("models/a/.1.x.tmp");
        std::fs::create_dir_all(half.parent().unwrap()).unwrap();
        std::fs::write(&half, b"{").unwrap();

        store.cleanup().unwrap();

        assert!(!store.artifact(&shared).exists());
        assert!(!stopped.exists());
        assert!(running.is_dir());
        assert!(!half.exists());
    }

    #[test]
    fn an_artifact_without_a_manifest_is_used_by_a_download() {
        let (_dir, store) = store();
        let data = data(1);
        let shared = &data.artifacts["shared"];
        put_files(shared, &store.artifact(&shared.digest)).unwrap();

        let a = data.find("a", "test-1").unwrap();
        let fetches = AtomicUsize::new(0);
        let mut fetch = |artifact: &Artifact, _: &Path, files: &Path| {
            fetches.fetch_add(1, Ordering::SeqCst);
            put_files(artifact, files)
        };
        store.download(&data, a, &mut fetch, &mut || {}).unwrap();
        assert_eq!(fetches.load(Ordering::SeqCst), 1, "only the artifact that is missing");
    }

    #[test]
    fn a_manifest_that_does_not_parse_stops_the_removal_of_artifacts() {
        let (_dir, store) = store();
        let data = data(1);
        download(&store, &data, "a");
        std::fs::write(store.manifest_path("a", "test-1").unwrap(), b"not json").unwrap();
        store.cleanup().unwrap();
        assert_eq!(names(&store, "artifacts").len(), 2);
        assert!(!store.on_disk(data.find("a", "test-1").unwrap()).unwrap());
    }

    #[test]
    fn a_cleanup_between_the_steps_of_a_download_does_not_lose_the_model() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = tempfile::tempdir().unwrap();
        let waiting = Store::with_hook(dir.path().join("v1"), Some(hooks.path().to_path_buf())).unwrap();
        let other = Store::with_hook(dir.path().join("v1"), None).unwrap();
        let data = Arc::new(data(1));
        // An artifact without a manifest: the download does not fetch it.
        let shared = &data.artifacts["shared"];
        put_files(shared, &other.artifact(&shared.digest)).unwrap();

        // The download waits after it looked at the artifacts.
        let hold = hooks.path().join("hold-download.manifest_read");
        std::fs::write(&hold, b"").unwrap();
        let thread = {
            let data = data.clone();
            std::thread::spawn(move || download(&waiting, &data, "a"))
        };
        while !hooks.path().join("at-download.manifest_read").exists() {
            std::thread::sleep(Duration::from_millis(5));
        }
        // A cleanup takes the artifact, because no manifest names it.
        other.cleanup().unwrap();
        assert!(!other.artifact(&shared.digest).exists());
        std::fs::remove_file(&hold).unwrap();
        thread.join().unwrap();

        // Each manifest names artifacts that exist.
        assert!(other.on_disk(data.find("a", "test-1").unwrap()).unwrap());
        assert!(other.artifact(&shared.digest).join("main.bin").is_file());
    }

    #[test]
    fn a_failure_in_a_later_artifact_leaves_no_stage_of_the_earlier_one() {
        let (_dir, store) = store();
        let data = data(1);
        let a = data.find("a", "test-1").unwrap();
        let mut count = 0;
        let mut second_fails = |artifact: &Artifact, _: &Path, files: &Path| {
            count += 1;
            if count == 2 {
                return Err("no network".into());
            }
            put_files(artifact, files)
        };
        store.download(&data, a, &mut second_fails, &mut || {}).unwrap_err();
        assert!(names(&store, "staging").is_empty());
        assert!(names(&store, "artifacts").is_empty());
    }

    #[test]
    fn stray_files_stop_nothing() {
        let (_dir, store) = store();
        let data = data(1);
        download(&store, &data, "a");
        for folder in ["models", "artifacts", "staging", "trash"] {
            std::fs::write(store.root.join(folder).join(".DS_Store"), b"x").unwrap();
        }
        store.delete("a", "test-1").unwrap();
        assert_eq!(names(&store, "artifacts"), [".DS_Store"]);
        assert_eq!(names(&store, "models"), [".DS_Store"]);
        download(&store, &data, "a");
        assert!(store.on_disk(data.find("a", "test-1").unwrap()).unwrap());
    }

    #[test]
    fn a_download_says_so_when_a_higher_manifest_names_a_missing_artifact() {
        let (_dir, store) = store();
        // The new revision has an artifact that the older descriptor does not know.
        let (old, mut new) = (data(1), data(2));
        new.artifacts.insert("only-a".to_string(), artifact(&[("b.bin", "bytes of b")]));
        download(&store, &new, "a");
        std::fs::remove_dir_all(store.artifact(&digest_of(&new, "only-a"))).unwrap();
        let a = old.find("a", "test-1").unwrap();
        let error = store.download(&old, a, &mut |x, _, files| put_files(x, files), &mut || {}).unwrap_err();
        assert!(matches!(error, Error::Verify(_)), "{error}");
    }

    #[test]
    fn on_disk_now_does_not_wait() {
        let (_dir, store) = store();
        let data = data(1);
        download(&store, &data, "a");
        let a = data.find("a", "test-1").unwrap();
        assert_eq!(store.on_disk_now(a).unwrap(), Some(true));
        let meta = store.meta_exclusive().unwrap();
        assert_eq!(store.on_disk_now(a).unwrap(), None);
        drop(meta);
    }

    #[test]
    fn bad_names_are_refused() {
        let (_dir, store) = store();
        for bad in ["", "..", ".x", "a/b", "a\\b", "a:b"] {
            assert!(store.manifest_path(bad, "f").is_err(), "{bad}");
            assert!(store.delete(bad, "f").is_err(), "{bad}");
        }
    }
}
