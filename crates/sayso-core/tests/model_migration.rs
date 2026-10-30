//! The migration of the old models folders into the model store, with small fake
//! models and no network. A child process runs the migration and the test kills
//! it at each step (`docs/sdk.md`, "Migration and downgrade").

use sayso_core::engine_catalog::descriptors::{self, Artifact, Descriptor, Descriptors, File, Part, Source};
use sayso_core::model_migration::{self, JOURNAL, Links, OldModel, OldPart, Report, STEPS};
use sayso_core::model_store::{HOOK_ENV, Store};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const FORMAT: &str = "test-1";
const CHILD_ENV: &str = "SAYSO_TEST_MIGRATE_CHILD";

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn artifact(files: &[(&str, &str)]) -> Artifact {
    let files: Vec<File> =
        files.iter().map(|(path, text)| File { path: path.to_string(), size: text.len() as u64, sha256: sha(text.as_bytes()) }).collect();
    Artifact { source: Source::File { url: "http://unused".into() }, digest: descriptors::digest(&files).unwrap(), files }
}

/// `a` and `b` share `shared`. `changed` has a file with other bytes on disk,
/// `short` has a file that is missing, and `absent` has no folder.
fn data() -> Descriptors {
    let model = |model: &str, parts: &[(&str, &str)]| Descriptor {
        model: model.into(),
        format: FORMAT.into(),
        revision: 1,
        parts: parts.iter().map(|(role, artifact)| Part { role: role.to_string(), artifact: artifact.to_string() }).collect(),
    };
    Descriptors {
        artifacts: BTreeMap::from([
            ("shared".to_string(), artifact(&[("main.bin", "shared bytes"), ("sub/vocab.txt", "words")])),
            ("only-a".to_string(), artifact(&[("a.bin", "bytes of a")])),
            ("only-b".to_string(), artifact(&[("b.bin", "bytes of b")])),
            ("changed".to_string(), artifact(&[("c.bin", "the bytes of the descriptor")])),
            ("short".to_string(), artifact(&[("s1.bin", "one"), ("s2.bin", "two")])),
            ("absent".to_string(), artifact(&[("x.bin", "x")])),
        ]),
        descriptors: vec![
            model("a", &[("main", "shared"), ("extra", "only-a")]),
            model("absent", &[("main", "absent")]),
            model("b", &[("main", "shared"), ("extra", "only-b")]),
            model("changed", &[("main", "changed"), ("extra", "only-a")]),
            model("short", &[("main", "short")]),
        ],
    }
}

fn old() -> Vec<OldModel> {
    let model = |model: &str, parts: &[(&str, &str)], remove: &[&str]| OldModel {
        model: model.into(),
        format: FORMAT.into(),
        parts: parts.iter().map(|(role, folder)| OldPart { role: role.to_string(), folders: vec![folder.to_string()] }).collect(),
        remove: remove.iter().map(|f| f.to_string()).collect(),
    };
    vec![
        model("a", &[("main", "tools/shared"), ("extra", "repo-a/fast")], &["repo-a/fast", "tools/shared"]),
        model("b", &[("main", "tools/shared"), ("extra", "repo-b")], &["repo-b", "tools/shared"]),
        model("changed", &[("main", "repo-changed"), ("extra", "repo-a/fast")], &["repo-a/fast", "repo-changed"]),
        model("short", &[("main", "repo-short")], &["repo-short"]),
        model("absent", &[("main", "repo-absent")], &["repo-absent"]),
    ]
}

fn put(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A models folder as Sayso 0.13 leaves it, with markers, a file that the
/// descriptor does not list, and a folder that the migration does not know.
fn models_folder(root: &Path) {
    put(root, "tools/shared/main.bin", "shared bytes");
    put(root, "tools/shared/sub/vocab.txt", "words");
    put(root, "tools/shared/.cache/huggingface/tokenizer.json.metadata", "library metadata");
    put(root, "repo-a/fast/a.bin", "bytes of a");
    put(root, "repo-a/fast/.sayso-downloaded", "");
    put(root, "repo-b/b.bin", "bytes of b");
    put(root, "repo-changed/c.bin", "other bytes on disk");
    put(root, "repo-short/s1.bin", "one");
    put(root, "cleanup/s1-mini/model.gguf", "a cleanup model");
    put(root, "retired-model/file.bin", "a model that the catalog does not know");
}

/// Every file under `root`, by its path. Lock files say nothing about the result.
fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(folder: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(folder).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = format!("{prefix}{name}");
            if path.ends_with("v1/locks") || path == "locks" {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                out.insert(format!("{path}/"), Vec::new());
                walk(&entry.path(), &format!("{path}/"), out);
            } else {
                out.insert(path, std::fs::read(entry.path()).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out);
    out
}

fn open(root: &Path) -> Store {
    Store::open(root.join("v1")).unwrap()
}

fn migrate(root: &Path) -> Report {
    model_migration::run_with(&open(root), root, &data(), &old(), Links::Hard).unwrap()
}

fn on_disk(root: &Path, model: &str) -> bool {
    let data = data();
    open(root).on_disk(data.find(model, FORMAT).unwrap()).unwrap()
}

/// The result of a migration without a crash.
fn expect_migrated(root: &Path) {
    for model in ["a", "b"] {
        assert!(on_disk(root, model), "{model} moved");
    }
    for model in ["changed", "short", "absent"] {
        assert!(!on_disk(root, model), "{model} did not move");
    }
    // Models that moved need no download.
    let data = data();
    for model in ["a", "b"] {
        let descriptor = data.find(model, FORMAT).unwrap();
        open(root).download(&data, descriptor, &mut |_, _, _| panic!("no download"), &mut || {}).unwrap();
        let lease = open(root).load(descriptor).unwrap().unwrap();
        let main = lease.folder("main").unwrap();
        assert_eq!(std::fs::read_to_string(main.join("sub/vocab.txt")).unwrap(), "words");
        assert!(!main.join(".cache").exists(), "library metadata is not a file of the artifact");
    }
    // The folders of moved models are gone, with their parents.
    assert!(!root.join("tools").exists());
    assert!(!root.join("repo-b").exists());
    // `repo-a/fast` holds a file that the model `changed` still wants.
    assert_eq!(std::fs::read_to_string(root.join("repo-a/fast/a.bin")).unwrap(), "bytes of a");
    // Models that did not move are not lost, and what the migration does not know stays.
    assert_eq!(std::fs::read_to_string(root.join("repo-changed/c.bin")).unwrap(), "other bytes on disk");
    assert_eq!(std::fs::read_to_string(root.join("repo-short/s1.bin")).unwrap(), "one");
    assert_eq!(std::fs::read_to_string(root.join("cleanup/s1-mini/model.gguf")).unwrap(), "a cleanup model");
    assert_eq!(std::fs::read_to_string(root.join("retired-model/file.bin")).unwrap(), "a model that the catalog does not know");
    // Nothing waits in the store.
    let v1 = root.join("v1");
    assert_eq!(std::fs::read_dir(v1.join("staging")).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(v1.join("trash")).unwrap().count(), 0);
}

#[test]
fn complete_models_move_and_others_stay() {
    let dir = tempfile::tempdir().unwrap();
    models_folder(dir.path());

    let report = migrate(dir.path());

    assert_eq!(report.moved, ["a", "b"]);
    let kept: Vec<&str> = report.kept.iter().map(|(model, _)| model.as_str()).collect();
    assert_eq!(kept, ["changed", "short"]);
    assert!(report.kept[0].1.contains("hash") || report.kept[0].1.contains("bytes"), "{:?}", report.kept[0]);
    assert!(report.kept[1].1.contains("missing"), "{:?}", report.kept[1]);
    expect_migrated(dir.path());
}

#[test]
fn a_volume_without_hard_links_gets_copies() {
    let dir = tempfile::tempdir().unwrap();
    models_folder(dir.path());
    model_migration::run_with(&open(dir.path()), dir.path(), &data(), &old(), Links::CopyOnly).unwrap();
    expect_migrated(dir.path());
}

#[cfg(unix)]
#[test]
fn a_hard_link_takes_the_same_file() {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    models_folder(dir.path());
    let inode = std::fs::metadata(dir.path().join("repo-b/b.bin")).unwrap().ino();
    migrate(dir.path());
    let data = data();
    let lease = open(dir.path()).load(data.find("b", FORMAT).unwrap()).unwrap().unwrap();
    assert_eq!(std::fs::metadata(lease.folder("extra").unwrap().join("b.bin")).unwrap().ino(), inode);
}

#[test]
fn a_second_run_changes_nothing_and_does_not_hash_a_kept_model_again() {
    let dir = tempfile::tempdir().unwrap();
    models_folder(dir.path());
    migrate(dir.path());
    let before = tree(dir.path());

    let report = migrate(dir.path());

    assert!(report.moved.is_empty());
    assert_eq!(report.kept.len(), 2);
    assert_eq!(tree(dir.path()), before);
    // With the right bytes now, it would move, if the run looked at it again.
    put(dir.path(), "repo-changed/c.bin", "the bytes of the descriptor");
    assert!(migrate(dir.path()).moved.is_empty());
}

#[test]
fn the_folders_of_a_kept_model_go_when_the_model_is_in_the_store() {
    let dir = tempfile::tempdir().unwrap();
    models_folder(dir.path());
    migrate(dir.path());
    let data = data();
    let descriptor = data.find("changed", FORMAT).unwrap();
    open(dir.path())
        .download(
            &data,
            descriptor,
            &mut |artifact, _, files| {
                for file in &artifact.files {
                    std::fs::create_dir_all(files)?;
                    std::fs::write(files.join(&file.path), "the bytes of the descriptor")?;
                }
                Ok(())
            },
            &mut || {},
        )
        .unwrap();

    let report = migrate(dir.path());

    assert_eq!(report.kept.len(), 1, "{report:?}");
    assert!(!dir.path().join("repo-changed").exists());
    assert!(!dir.path().join("repo-a").exists(), "no model wants it now");
    assert!(on_disk(dir.path(), "changed"));
}

#[test]
fn a_model_in_the_store_needs_no_old_file() {
    let dir = tempfile::tempdir().unwrap();
    models_folder(dir.path());
    migrate(dir.path());
    // A downgrade, then an upgrade: the older Sayso downloaded `a` and `b` again.
    put(dir.path(), "tools/shared/main.bin", "another revision");
    put(dir.path(), "repo-b/b.bin", "another revision");

    let report = migrate(dir.path());

    assert_eq!(report.moved, ["a", "b"]);
    assert!(!dir.path().join("repo-b").exists());
    assert!(!dir.path().join("tools").exists());
    assert!(on_disk(dir.path(), "a"));
}

#[test]
fn a_folder_with_nothing_goes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("repo-absent")).unwrap();
    std::fs::remove_dir(dir.path().join("repo-absent")).unwrap();
    std::fs::create_dir_all(dir.path().join("repo-a")).unwrap();
    let report = migrate(dir.path());
    assert!(report.moved.is_empty());
    assert!(report.kept.is_empty());
    assert!(!dir.path().join("repo-a").exists(), "an empty parent of an old folder goes");
    assert!(dir.path().join("v1").join(JOURNAL).is_file());
}

/// A file where the stages go stops a run with an error before any model moved.
fn block_stages(root: &Path) {
    put(root, "v1/migrating", "");
}

fn unblock_stages(root: &Path) {
    std::fs::remove_file(root.join("v1/migrating")).unwrap();
}

#[test]
fn a_model_downloaded_after_a_failed_run_is_not_lost() {
    let dir = tempfile::tempdir().unwrap();
    models_folder(dir.path());
    block_stages(dir.path());
    assert!(model_migration::run_with(&open(dir.path()), dir.path(), &data(), &old(), Links::Hard).is_err());
    assert!(!model_migration::store_in_use(dir.path()), "each old folder is there, so the engine can use them");
    assert_eq!(std::fs::read_to_string(dir.path().join("repo-b/b.bin")).unwrap(), "bytes of b");

    // The engine ran on the old folders, and the user downloaded and repaired a model.
    put(dir.path(), "repo-absent/x.bin", "x");
    put(dir.path(), "repo-changed/c.bin", "the bytes of the descriptor");
    unblock_stages(dir.path());
    let report = migrate(dir.path());

    assert!(report.moved.contains(&"absent".to_string()), "{report:?}");
    assert!(report.moved.contains(&"changed".to_string()), "{report:?}");
    assert!(on_disk(dir.path(), "absent"));
    assert!(on_disk(dir.path(), "changed"));
    assert!(model_migration::store_in_use(dir.path()));
}

#[test]
fn the_cleanup_of_an_engine_leaves_a_stage_of_the_migration() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "v1/migrating/stage/files/a.bin", "bytes");
    open(dir.path()).cleanup().unwrap();
    assert!(dir.path().join("v1/migrating/stage/files/a.bin").is_file());
    // The next run removes what a run left.
    migrate(dir.path());
    assert!(!dir.path().join("v1/migrating").exists());
}

#[cfg(unix)]
#[test]
fn files_that_are_read_only_or_unreadable_do_not_stop_the_run() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    models_folder(dir.path());
    let set = |path: &str, mode: u32| std::fs::set_permissions(dir.path().join(path), std::fs::Permissions::from_mode(mode)).unwrap();
    set("repo-b/b.bin", 0o444);
    put(dir.path(), "repo-absent/x.bin", "x");
    set("repo-absent/x.bin", 0o000);
    let unreadable = std::fs::File::open(dir.path().join("repo-absent/x.bin")).is_err();

    let report = migrate(dir.path());

    assert!(report.moved.contains(&"b".to_string()), "{report:?}");
    if unreadable {
        assert!(report.kept.iter().any(|(model, _)| model == "absent"), "{report:?}");
        set("repo-absent/x.bin", 0o644);
        assert_eq!(std::fs::read_to_string(dir.path().join("repo-absent/x.bin")).unwrap(), "x");
    }
}

#[cfg(unix)]
#[test]
fn a_link_to_a_missing_volume_keeps_its_place() {
    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("/nonexistent/sayso-volume", dir.path().join("repo-absent")).unwrap();
    migrate(dir.path());
    assert!(dir.path().join("repo-absent").is_symlink());
}

/// The child: it runs the migration with the hook of its environment. It runs
/// only when the test below starts it.
#[test]
#[ignore = "the kill test starts this as a child process"]
fn child_migration() {
    let Some(root) = std::env::var_os(CHILD_ENV) else { return };
    let root = PathBuf::from(root);
    model_migration::run_with(&open(&root), &root, &data(), &old(), Links::Hard).unwrap();
}

fn wait_for(path: &Path, child: &mut std::process::Child, step: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !path.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("the child ended with {status} before the step {step}");
        }
        assert!(Instant::now() < deadline, "the child did not get to the step {step}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_kill_at_each_step_gives_the_same_result_after_the_next_run() {
    let want = {
        let dir = tempfile::tempdir().unwrap();
        models_folder(dir.path());
        migrate(dir.path());
        tree(dir.path())
    };
    let original = {
        let dir = tempfile::tempdir().unwrap();
        models_folder(dir.path());
        tree(dir.path())
    };
    for step in STEPS {
        let dir = tempfile::tempdir().unwrap();
        let hook = tempfile::tempdir().unwrap();
        models_folder(dir.path());
        std::fs::write(hook.path().join(format!("hold-{step}")), b"").unwrap();

        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["child_migration", "--exact", "--ignored", "--nocapture"])
            .env(CHILD_ENV, dir.path())
            .env(HOOK_ENV, hook.path())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        wait_for(&hook.path().join(format!("at-{step}")), &mut child, step);
        child.kill().unwrap();
        child.wait().unwrap();
        let before_removal = STEPS.iter().position(|s| *s == step) < STEPS.iter().position(|s| *s == "migration.folder_removed");
        assert_eq!(model_migration::store_in_use(dir.path()), !before_removal, "at {step}");

        // Before the old folders go, each of their files is there.
        if STEPS.iter().position(|s| *s == step) < STEPS.iter().position(|s| *s == "migration.folder_removed") {
            for (path, bytes) in &original {
                assert_eq!(std::fs::read(dir.path().join(path)).ok().as_ref().or(path.ends_with('/').then_some(bytes)), Some(bytes), "{path} at {step}");
            }
        }

        // The next run makes the same store, and one more run changes nothing.
        migrate(dir.path());
        expect_migrated(dir.path());
        assert_eq!(tree(dir.path()), want, "after a kill at {step}");
        migrate(dir.path());
        assert_eq!(tree(dir.path()), want, "after one more run, for a kill at {step}");
    }
}
