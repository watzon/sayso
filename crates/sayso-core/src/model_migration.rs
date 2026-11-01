//! The move of the models from the old folders into the model store
//! (`docs/sdk.md`, "Migration and downgrade").
//!
//! It runs before Sayso starts its engine, and only one Sayso runs for each data
//! folder, so no engine of Sayso writes the old folders while it runs. It holds
//! `locks/meta` for the whole run, so an engine of another program waits.
//!
//! `migration.json` is the journal. After a crash, the next start does each step
//! again, and each step can run twice with the same result. A hard link leaves
//! the old folder as it was, so nothing is lost until the old folder goes, and a
//! folder goes only when each model that needs it is in the store.
//!
//! A model whose old files do not match its descriptor is not moved. Its folders
//! stay, the journal keeps its name so that later starts do not hash it again, and
//! the folders go at the first start with the model in the store.
//!
//! Until the first old folder goes, an engine can run on the old folders after an
//! error ([`store_in_use`]). Each later run then looks at the old folders again.

use crate::engine_catalog::descriptors::{self, Artifact, Descriptor, Descriptors};
use crate::engine_catalog::{self, Engine};
use crate::model_store::{self, Error, Store, io, rename, sync_folder, verify};
use crate::models::EngineKind;
use crate::models_onnx::System;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Write;
use std::path::Path;

/// The name of the journal in the store.
pub const JOURNAL: &str = "migration.json";

/// The folder of the stages in the store. It is not `staging/`, because the cleanup
/// of an engine of another program deletes a stage whose `get` lock nobody holds,
/// and the migration cannot wait for a `get` lock while it holds `meta`.
const STAGES: &str = "migrating";

/// The steps of the test hook, in order ([`Store::step`]).
pub const STEPS: [&str; 9] = [
    "migration.meta_locked",
    "migration.planned",
    "migration.linked",
    "migration.verified",
    "migration.published",
    "migration.manifest_written",
    "migration.marked",
    "migration.folder_removed",
    "migration.finished",
];

/// The folder of the whisper.cpp file of a model in the Rust engine (`WHISPER_CPP_REVISION`).
const WHISPER_CPP_FOLDER: &str = "main";

/// The old place of one artifact of a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OldPart {
    pub role: String,
    /// The folders that can hold its files, relative to the models folder in `/`
    /// form. The first that has a file gives it.
    pub folders: Vec<String>,
}

/// The old files of one model, for one format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OldModel {
    pub model: String,
    pub format: String,
    /// The first part says if the model is there: a model without its folder is not moved.
    pub parts: Vec<OldPart>,
    /// The folders that go when the model has moved, relative to the models folder.
    pub remove: Vec<String>,
}

/// What a run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// The models that are in the store now and were in the old folders.
    pub moved: Vec<String>,
    /// The models that stay in the old folders, each with the reason.
    pub kept: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Pending,
    Done,
    Kept,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    #[serde(flatten)]
    old: OldModel,
    state: State,
    /// Why a model was kept.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    why: String,
}

/// `migration.json`. It stays after the end, with the models that were kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Journal {
    finished: bool,
    /// Old folders can be gone from here on, so an engine must use the store.
    #[serde(default)]
    removing: bool,
    models: Vec<Entry>,
}

/// How a file gets into a staging folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Links {
    /// A hard link, and a copy when the volume has no hard links.
    Hard,
    /// A copy, as on a volume without hard links. For tests.
    CopyOnly,
}

/// Move the models of this build from the old folders in `models_dir` into the
/// store in `models_dir`. Call it before the engine starts. An error leaves the
/// old folders that were not done yet, and the next start goes on.
pub fn run(models_dir: &Path) -> Result<Report, Error> {
    let store = Store::open(models_dir.join(model_store::FOLDER))?;
    let engine = if cfg!(target_os = "macos") { Engine::Swift } else { Engine::Rust(System::current()) };
    run_with(&store, models_dir, descriptors::all(), &old_models(engine), Links::Hard)
}

/// After an error of [`run`]: true when the engine must use the store anyway,
/// because the old folders of the models that moved can be gone. When false, each
/// old folder is still there, and the engine can run on them for this start.
pub fn store_in_use(models_dir: &Path) -> bool {
    std::fs::read(models_dir.join(model_store::FOLDER).join(JOURNAL))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Journal>(&bytes).ok())
        .is_some_and(|journal| journal.finished || journal.removing)
}

/// [`run`] with other descriptors and another list of old models.
pub fn run_with(store: &Store, models_dir: &Path, data: &Descriptors, old: &[OldModel], links: Links) -> Result<Report, Error> {
    let meta = store.meta_exclusive()?;
    store.step("migration.meta_locked");
    // A run that stopped can leave copies, and they must not fill the disk.
    let stages = store.root().join(STAGES);
    let _ = std::fs::remove_dir_all(&stages);
    let path = store.root().join(JOURNAL);
    // A journal that does not parse holds nothing that the old folders do not say.
    let stored: Option<Journal> = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let first = stored.is_none();
    let resume = stored.as_ref().is_some_and(|j| !j.finished);
    let mut journal = match stored {
        // An engine can have run on the old folders since then, so a model can be
        // new, and a model that did not match can match now.
        Some(mut journal) if !journal.finished => {
            for entry in journal.models.iter_mut().filter(|e| e.state == State::Kept) {
                entry.state = State::Pending;
                entry.why.clear();
            }
            let removing = journal.removing;
            Journal { removing, ..plan(models_dir, old, journal.models) }
        }
        previous => plan(models_dir, old, previous.map(|j| j.models).unwrap_or_default()),
    };
    let work = first || resume || journal.models.iter().any(|e| e.state == State::Pending);
    if work {
        journal.finished = false;
        write_journal(&path, &journal)?;
    }
    store.step("migration.planned");

    let mut report = Report::default();
    for index in 0..journal.models.len() {
        let entry = &journal.models[index];
        if entry.state != State::Pending {
            continue;
        }
        let name = entry.old.model.clone();
        let result = match data.find(&name, &entry.old.format) {
            Some(descriptor) => migrate_model(store, models_dir, data, descriptor, &entry.old, links)?,
            None => Err("no descriptor".to_string()),
        };
        let entry = &mut journal.models[index];
        match result {
            Ok(()) => {
                entry.state = State::Done;
                report.moved.push(name);
            }
            Err(why) => {
                log::warn!("the model {name} is not moved into the store: {why}");
                entry.state = State::Kept;
                entry.why = why;
            }
        }
        write_journal(&path, &journal)?;
        store.step("migration.marked");
    }

    // A kept model that is in the store now gives its folders up.
    for entry in journal.models.iter_mut().filter(|e| e.state == State::Kept) {
        let on_store = data.find(&entry.old.model, &entry.old.format).is_some_and(|d| {
            store.read_manifest(&d.model, &d.format).ok().flatten().is_some_and(|m| store.complete(&m, d))
        });
        if on_store {
            entry.state = State::Done;
        }
    }
    let kept: Vec<Entry> = journal.models.iter().filter(|e| e.state == State::Kept).cloned().collect();
    let keep: BTreeSet<&str> = kept
        .iter()
        .flat_map(|e| e.old.remove.iter().chain(e.old.parts.iter().flat_map(|p| &p.folders)))
        .map(String::as_str)
        .collect();
    let mut remove: BTreeSet<&str> = journal.models.iter().flat_map(|e| e.old.remove.iter().map(String::as_str)).collect();
    if work {
        remove.extend(old.iter().flat_map(|m| m.remove.iter().map(String::as_str)));
    }
    let removed = journal.models.iter().any(|e| e.state == State::Done);
    if work {
        journal.removing = true;
        write_journal(&path, &journal)?;
    }
    for folder in remove.difference(&keep) {
        remove_old_folder(models_dir, folder);
        store.step("migration.folder_removed");
    }
    report.kept = kept.iter().map(|e| (e.old.model.clone(), e.why.clone())).collect();
    if work || removed {
        write_journal(&path, &Journal { finished: true, removing: false, models: kept })?;
    }
    let _ = std::fs::remove_dir_all(&stages);
    store.step("migration.finished");
    drop(meta);
    // A crash can leave a staging folder and an artifact without a manifest.
    if work && let Err(e) = store.cleanup() {
        log::warn!("the cleanup of the model store failed: {e}");
    }
    Ok(report)
}

/// The journal of a new run: the models of `previous`, and each other model whose
/// first folder is there.
fn plan(models_dir: &Path, old: &[OldModel], previous: Vec<Entry>) -> Journal {
    let mut models = previous;
    for model in old {
        if models.iter().any(|e| e.old.model == model.model && e.old.format == model.format) {
            continue;
        }
        let there = model.parts.first().is_some_and(|part| part.folders.iter().any(|f| models_dir.join(f).is_dir()));
        if there {
            models.push(Entry { old: model.clone(), state: State::Pending, why: String::new() });
        }
    }
    Journal { finished: true, removing: false, models }
}

/// Steps 2 and 3 for one model. Hold `meta` exclusive when you call this. The inner
/// error says why the model does not move. The outer error stops the run.
fn migrate_model(
    store: &Store,
    models_dir: &Path,
    data: &Descriptors,
    descriptor: &Descriptor,
    old: &OldModel,
    links: Links,
) -> Result<Result<(), String>, Error> {
    // A model that is in the store (from an engine of another program, or before
    // a downgrade) needs no file from here.
    if store.read_manifest(&descriptor.model, &descriptor.format)?.is_some_and(|m| store.complete(&m, descriptor)) {
        return Ok(Ok(()));
    }
    let mut staged = Vec::new();
    let mut outcome = Ok(());
    for (role, artifact) in data.parts(descriptor) {
        if store.artifact(&artifact.digest).is_dir() || staged.iter().any(|(digest, _)| *digest == artifact.digest) {
            continue;
        }
        let Some(part) = old.parts.iter().find(|p| p.role == role) else {
            outcome = Err(format!("no old folder for the role {role}"));
            break;
        };
        match stage_artifact(store, models_dir, artifact, part, links)? {
            Ok(stage) => staged.push((artifact.digest.clone(), stage)),
            Err(why) => {
                outcome = Err(why);
                break;
            }
        }
    }
    if let Err(why) = outcome {
        for (_, stage) in staged {
            let _ = std::fs::remove_dir_all(stage);
        }
        return Ok(Err(why));
    }
    for (digest, stage) in staged {
        let dest = store.artifact(&digest);
        if !dest.is_dir() {
            rename(&stage.join("files"), &dest)?;
        }
        let _ = std::fs::remove_dir_all(&stage);
        store.step("migration.published");
    }
    sync_folder(&store.root().join("artifacts"));
    store.write_manifest(descriptor, data)?;
    store.step("migration.manifest_written");
    let manifest = store.read_manifest(&descriptor.model, &descriptor.format)?;
    if !manifest.is_some_and(|m| store.complete(&m, descriptor)) {
        return Ok(Err("a manifest with a higher revision names a missing artifact".to_string()));
    }
    Ok(Ok(()))
}

/// Step 2 for one artifact: links of the old files in a staging folder, then the
/// check against the descriptor. Returns the staging folder.
fn stage_artifact(
    store: &Store,
    models_dir: &Path,
    artifact: &Artifact,
    part: &OldPart,
    links: Links,
) -> Result<Result<std::path::PathBuf, String>, Error> {
    let stage = store.root().join(STAGES).join(&artifact.digest);
    let files = stage.join("files");
    std::fs::create_dir_all(&files).map_err(io("cannot create", &files))?;
    // A file that this user cannot read keeps its model out. Another error, as for
    // a full disk, stops the run, and the next start tries again.
    let unreadable = |e: &Error| matches!(e, Error::Io { source, .. } if source.kind() == std::io::ErrorKind::PermissionDenied);
    let refuse = |why: String| {
        let _ = std::fs::remove_dir_all(&stage);
        Ok(Err(why))
    };
    for file in &artifact.files {
        let found = part.folders.iter().map(|folder| models_dir.join(folder).join(&file.path)).find(|path| {
            std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
        });
        let Some(from) = found else {
            return refuse(format!("the file {} is missing", file.path));
        };
        let to = files.join(&file.path);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(io("cannot create", parent))?;
        }
        if let Err(e) = link_or_copy(&from, &to, links).map_err(io("cannot copy", &from)) {
            return if unreadable(&e) { refuse(e.to_string()) } else { Err(e) };
        }
    }
    store.step("migration.linked");
    match verify(&files, artifact) {
        Ok(()) => {}
        Err(Error::Verify(why)) => return refuse(why),
        Err(e) if unreadable(&e) => return refuse(e.to_string()),
        Err(e) => return Err(e),
    }
    store.step("migration.verified");
    Ok(Ok(stage))
}

fn link_or_copy(from: &Path, to: &Path, links: Links) -> std::io::Result<()> {
    if links == Links::Hard && std::fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map(|_| ())
}

/// Delete an old folder and the parents that are empty then. A folder that does
/// not go now goes at a later start, because its model is still found there.
fn remove_old_folder(models_dir: &Path, folder: &str) {
    let path = models_dir.join(folder);
    // A link to a volume that is not there at this start keeps its place.
    if path.is_symlink() && !path.exists() {
        return;
    }
    match std::fs::remove_dir_all(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            log::warn!("cannot delete {}: {e}", path.display());
            return;
        }
        _ => {}
    }
    let mut parent = path.parent();
    while let Some(folder) = parent.filter(|p| *p != models_dir && p.starts_with(models_dir)) {
        if std::fs::remove_dir(folder).is_err() {
            break;
        }
        parent = folder.parent();
    }
}

fn write_journal(path: &Path, journal: &Journal) -> Result<(), Error> {
    let folder = path.parent().expect("the journal has a folder");
    let temp = folder.join(format!(".{}.{}.tmp", JOURNAL, uuid::Uuid::new_v4().simple()));
    let bytes = serde_json::to_vec_pretty(journal).expect("a journal is JSON");
    let written = File::create(&temp).and_then(|mut file| {
        file.write_all(&bytes)?;
        file.sync_all()
    });
    written.map_err(io("cannot write", &temp))?;
    rename(&temp, path)?;
    sync_folder(folder);
    Ok(())
}

/// The old files of the models of the catalog of `engine`. The folder names of the
/// Swift engine come from FluidAudio (`Repo.folderName`) and WhisperKit, and a test
/// of the Swift engine checks them against `ModelStore.swift`.
pub fn old_models(engine: Engine) -> Vec<OldModel> {
    engine_catalog::models(engine)
        .iter()
        .filter_map(|m| {
            let format = descriptors::format(&m.engine)?;
            let id = m.id.as_str();
            let (parts, remove) = match engine {
                Engine::Swift => swift_folders(&m.engine)?,
                Engine::Rust(_) => rust_folders(id, &m.engine)?,
            };
            Some(OldModel { model: id.into(), format: format.into(), parts, remove })
        })
        .collect()
}

fn part(role: &str, folders: &[&str]) -> OldPart {
    OldPart { role: role.into(), folders: folders.iter().map(|f| f.to_string()).collect() }
}

/// The folders of a model of the Swift engine, as `ModelStore.swift` has them.
fn swift_folders(engine: &EngineKind) -> Option<(Vec<OldPart>, Vec<String>)> {
    let one = |folder: String| Some((vec![part("model", &[&folder])], vec![folder.clone()]));
    match engine {
        EngineKind::ParakeetUnified { .. } => {
            let (model, ctc) = ("parakeet-unified-en-0.6b", "parakeet-ctc-110m-coreml");
            Some((vec![part("model", &[model]), part("ctc", &[ctc])], vec![model.into(), ctc.into()]))
        }
        EngineKind::ParakeetEou => one("parakeet-eou-streaming/160ms".into()),
        EngineKind::ParakeetTdt { version } => one(
            match version.as_str() {
                "v2" => "parakeet-tdt-0.6b-v2",
                "v3" => "parakeet-tdt-0.6b-v3",
                "ultra" => "parakeet-ultra",
                "redux" => "parakeet-redux",
                "phonon2" => "phonon-2",
                "tdt_ctc_110m" => "parakeet-tdt-ctc-110m",
                "ja" => "parakeet-ja",
                "fluid_mini" => "fluid-speech-mini-coreml",
                other => panic!("no old folder for the Parakeet TDT version {other}"),
            }
            .into(),
        ),
        EngineKind::Nemotron { chunk_ms } => one(format!("nemotron-streaming/{chunk_ms}ms")),
        EngineKind::NemotronMultilingual { chunk_ms } => one(format!("nemotron-multilingual/multilingual/{chunk_ms}ms")),
        EngineKind::Cohere => {
            let folder = "cohere-transcribe/q8";
            Some((vec![part("model", &[folder]), part("vocabulary", &[folder])], vec![folder.into()]))
        }
        EngineKind::Canary => one("canary-1b-v2".into()),
        EngineKind::SenseVoice => one("sensevoice-small".into()),
        EngineKind::Paraformer => one("paraformer-large-zh".into()),
        EngineKind::FluidVad => one("silero-vad".into()),
        EngineKind::FluidDiarization => one("speaker-diarization".into()),
        EngineKind::Whisper { variant } => {
            let model = format!("models/argmaxinc/whisperkit-coreml/{variant}");
            let tokenizer = format!("models/{}", descriptors::whisper_tokenizer(variant));
            // The Hub code keeps what it knows about a download beside the variants.
            let cache = "models/argmaxinc/whisperkit-coreml/.cache".to_string();
            Some((vec![part("model", &[&model]), part("tokenizer", &[&tokenizer])], vec![model, tokenizer, cache]))
        }
        _ => None,
    }
}

/// The folders of a model of the Rust engine: `<model id>/<archive>/`, as
/// `crates/sayso-engine/src/store.rs` has them. The Windows engine before 0.12 kept a
/// Whisper file directly in the folder of the model (finding 1 of "Write inventory").
fn rust_folders(id: &str, engine: &EngineKind) -> Option<(Vec<OldPart>, Vec<String>)> {
    let archive = |name: &str| format!("{id}/{name}");
    let stem = |file: &str| file.strip_suffix(".onnx").unwrap_or(file).to_string();
    let parts = match engine {
        EngineKind::Sherpa { archive: main, preview_archive, .. } => {
            let mut parts = vec![part("main", &[&archive(main)])];
            parts.extend(preview_archive.iter().map(|p| part("preview", &[&archive(p)])));
            parts
        }
        EngineKind::SherpaVad { file, .. } => vec![part("main", &[&archive(&stem(file))])],
        EngineKind::SherpaDiarization { segmentation, embedding } => {
            vec![part("segmentation", &[&archive(segmentation)]), part("embedding", &[&archive(&stem(embedding))])]
        }
        EngineKind::WhisperCpp { .. } => vec![part("main", &[&archive(WHISPER_CPP_FOLDER), id])],
        EngineKind::ParakeetCpp { revision, preview_archive, .. } => {
            let mut parts = vec![part("main", &[&archive(revision)])];
            parts.extend(preview_archive.iter().map(|p| part("preview", &[&archive(p)])));
            parts
        }
        _ => return None,
    };
    Some((parts, vec![id.to_string()]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each model of each catalog with a descriptor has an old place for each part,
    /// and each artifact file of a real descriptor can come from there.
    #[test]
    fn each_part_of_each_descriptor_has_an_old_folder() {
        for engine in [Engine::Swift, Engine::Rust(System::Linux), Engine::Rust(System::Windows)] {
            let old = old_models(engine);
            for model in &old {
                let descriptor = descriptors::all()
                    .find(&model.model, &model.format)
                    .unwrap_or_else(|| panic!("{} {} has no descriptor", model.model, model.format));
                for wanted in &descriptor.parts {
                    assert!(model.parts.iter().any(|p| p.role == wanted.role), "{} has no folder for {}", model.model, wanted.role);
                }
                assert!(!model.remove.is_empty(), "{}", model.model);
            }
            assert!(!old.is_empty());
        }
    }

    /// The test of the Swift engine reads this list (`OldFoldersTests.swift`).
    #[test]
    fn the_swift_test_file_is_up_to_date() {
        let mut list = String::new();
        for model in old_models(Engine::Swift) {
            let parts: Vec<String> = model.parts.iter().map(|p| format!("(\"{}\", \"{}\")", p.role, p.folders[0])).collect();
            list += &format!("    (\"{}\", [{}]),\n", model.model, parts.join(", "));
        }
        let want = format!(
            "// Generated from crates/sayso-core/src/model_migration.rs. Do not edit.\n\
             // Make it again with: SAYSO_WRITE_CATALOG=1 cargo test -p sayso-core model_migration\n\
             \n\
             /// The old folder of each part of each model, as the migration in Sayso looks for them.\n\
             let migrationOldFolders: [(id: String, parts: [(role: String, folder: String)])] = [\n{list}]\n"
        );
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../native/macos/SaysoEngine/Tests/SaysoEngineTests/OldFolders.generated.swift");
        if std::env::var_os("SAYSO_WRITE_CATALOG").is_some() {
            std::fs::write(&path, &want).unwrap();
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
        assert!(have == want, "{} is out of date; run: SAYSO_WRITE_CATALOG=1 cargo test -p sayso-core model_migration", path.display());
    }
}
