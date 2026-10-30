//! The store level: two engine processes on one model store (`docs/sdk.md`,
//! "Model store"), with a small real VAD model.
//!
//! An engine downloads the model once with the descriptors that it has built
//! in. The suite then serves those bytes itself, and gives the engines test
//! descriptors, so the rest needs no network and can make models that share an
//! artifact. The test hook of the engine stops it at each step of the rules,
//! while a second engine does its work or the suite kills the first.

use super::*;
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

const STORE_ENV: &str = "SAYSO_MODEL_STORE";
const HOOK_ENV: &str = "SAYSO_STORE_TEST_HOOK";
const DESCRIPTORS_ENV: &str = "SAYSO_STORE_TEST_DESCRIPTORS";
const HF_ENV: &str = "SAYSO_HF_ENDPOINT";

const DOWNLOAD_STEPS: [&str; 8] = [
    "download.manifest_read",
    "download.get_locked",
    "download.staged",
    "download.fetched",
    "download.meta_locked",
    "download.artifact_published",
    "download.manifest_written",
    "download.done",
];
const LOAD_STEPS: [&str; 4] = ["load.manifest_read", "load.use_locked", "load.ready", "load.loaded"];
const DELETE_STEPS: [&str; 2] = ["delete.meta_locked", "delete.manifest_removed"];
const CLEANUP_STEPS: [&str; 7] = [
    "cleanup.meta_locked",
    "cleanup.use_locked",
    "cleanup.trashed",
    "cleanup.meta_released",
    "cleanup.trash_emptied",
    "cleanup.staging_locked",
    "cleanup.done",
];

/// The longest wait for a step of the store.
const STEP_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the second engine gets before the first goes on. It can wait for a lock.
const CONTENTION: Duration = Duration::from_millis(150);

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// The digest of the files of an artifact (`docs/sdk.md`, "Digest").
fn digest(files: &BTreeMap<String, Vec<u8>>) -> String {
    let mut hasher = Sha256::new();
    for (path, bytes) in files {
        hasher.update(format!("{path}\0{}\0{}\n", bytes.len(), sha256(bytes)));
    }
    hex(&hasher.finalize())
}

// ---------------------------------------------------------------------------
// The file server
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Server {
    port: u16,
    /// How many times each path was asked for.
    requests: Arc<Mutex<HashMap<String, usize>>>,
}

fn serve(files: HashMap<String, Vec<u8>>) -> Result<Server, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("cannot start the file server: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let requests = Arc::new(Mutex::new(HashMap::new()));
    let (counted, files) = (Arc::clone(&requests), Arc::new(files));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (counted, files) = (Arc::clone(&counted), Arc::clone(&files));
            std::thread::spawn(move || answer(stream, &counted, &files));
        }
    });
    Ok(Server { port, requests })
}

fn answer(mut stream: TcpStream, requests: &Mutex<HashMap<String, usize>>, files: &HashMap<String, Vec<u8>>) {
    let mut head = Vec::new();
    let mut byte = [0];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => return,
        }
    }
    let head = String::from_utf8_lossy(&head);
    let path = head.split_whitespace().nth(1).unwrap_or_default().to_string();
    *requests.lock().unwrap().entry(path.clone()).or_insert(0) += 1;
    let _ = match files.get(&path) {
        Some(bytes) => stream
            .write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).as_bytes())
            .and_then(|()| stream.write_all(bytes)),
        None => stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
    };
}

// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------

/// The real model, the server, and the test descriptors.
struct Fixture {
    suite: Suite,
    engine_json: Value,
    format: String,
    server: Server,
    /// The test descriptors, and the same with a lower revision of `store-rev`.
    descriptors: PathBuf,
    older: PathBuf,
    /// The files of each artifact, by digest.
    artifacts: HashMap<String, BTreeMap<String, Vec<u8>>>,
    /// The digest of an artifact, by its name in the test descriptors.
    names: HashMap<String, String>,
    _dir: tempfile::TempDir,
}

/// The one manifest of a model.
struct ManifestFile {
    format: String,
    revision: u64,
    /// The role and the digest of each part.
    parts: Vec<(String, String)>,
}

fn read_manifest(root: &Path, model: &str) -> Result<ManifestFile, String> {
    let folder = root.join("models").join(model);
    let mut entries = list(&folder)?;
    entries.retain(|name| name.ends_with(".json"));
    let [file] = entries.as_slice() else {
        return Err(format!("{} has the files {entries:?}, not one manifest", folder.display()));
    };
    let bytes = std::fs::read(folder.join(file)).map_err(|e| format!("cannot read {file}: {e}"))?;
    let manifest: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{file} is not JSON: {e}"))?;
    let revision = manifest["revision"].as_u64().ok_or_else(|| format!("no revision in {manifest}"))?;
    let parts = manifest["parts"]
        .as_array()
        .ok_or_else(|| format!("no parts in {manifest}"))?
        .iter()
        .filter_map(|p| Some((p["role"].as_str()?.to_string(), p["digest"].as_str()?.to_string())))
        .collect();
    Ok(ManifestFile { format: file.trim_end_matches(".json").to_string(), revision, parts })
}

fn list(folder: &Path) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(folder).map_err(|e| format!("cannot read {}: {e}", folder.display()))?;
    let mut names: Vec<String> = entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    Ok(names)
}

/// Every file under `folder`, by its path in `/` form.
fn read_tree(folder: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    fn walk(folder: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) -> Result<(), String> {
        for name in list(folder)? {
            let path = folder.join(&name);
            if path.is_dir() {
                walk(&path, &format!("{prefix}{name}/"), out)?;
            } else {
                out.insert(format!("{prefix}{name}"), std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?);
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(folder, "", &mut out)?;
    Ok(out)
}

impl Fixture {
    fn new(suite: &Suite, model: &Model) -> Result<Fixture, String> {
        let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
        // The real files: from the descriptors that the engine has built in.
        let seed_root = dir.path().join("seed");
        let mut seed_suite = suite.clone();
        seed_suite.env.push((STORE_ENV.into(), seed_root.display().to_string()));
        let mut seed = Engine::start(&seed_suite, None)?;
        let fields = json!({ "model": model.id, "engine": model.engine });
        seed.request("download", fields, suite.load_timeout)?.expect_type("ok")?;
        seed.finish(suite.timeout)?;
        let ManifestFile { format, parts, .. } = read_manifest(&seed_root, &model.id)?;

        let mut served: HashMap<String, Vec<u8>> = HashMap::new();
        let mut artifacts = HashMap::new();
        let mut names = HashMap::new();
        let mut json_artifacts = serde_json::Map::new();
        let mut add = |name: String, files: BTreeMap<String, Vec<u8>>| {
            let digest = digest(&files);
            let list: Vec<Value> = files
                .iter()
                .map(|(path, bytes)| json!({ "path": path, "size": bytes.len(), "sha256": sha256(bytes) }))
                .collect();
            let repo = format!("conformance/{name}");
            for (path, bytes) in &files {
                served.insert(format!("/{repo}/resolve/r/{path}"), bytes.clone());
            }
            json_artifacts.insert(
                name.clone(),
                json!({ "source": { "kind": "hugging_face", "repo": repo, "revision": "r" }, "digest": digest, "files": list }),
            );
            names.insert(name, digest.clone());
            artifacts.insert(digest, files);
        };

        let mut real_parts = Vec::new();
        let mut broken_parts = Vec::new();
        for (i, (role, digest)) in parts.iter().enumerate() {
            let files = read_tree(&seed_root.join("artifacts").join(digest))?;
            let broken: BTreeMap<String, Vec<u8>> = files.keys().map(|path| (path.clone(), b"not a model".to_vec())).collect();
            add(format!("real-{i}"), files);
            add(format!("broken-{i}"), broken);
            real_parts.push(json!({ "role": role, "artifact": format!("real-{i}") }));
            broken_parts.push(json!({ "role": role, "artifact": format!("broken-{i}") }));
        }
        for name in ["extra-a", "extra-b", "rev-1", "rev-2"] {
            add(name.to_string(), BTreeMap::from([(format!("{name}.bin"), format!("the bytes of {name}").into_bytes())]));
        }
        let server = serve(served)?;

        let with = |extra: &str| {
            let mut parts = real_parts.clone();
            parts.push(json!({ "role": extra, "artifact": extra }));
            parts
        };
        let descriptor = |model: &str, revision: u32, parts: Vec<Value>| json!({ "model": model, "format": format, "revision": revision, "parts": parts });
        let write = |name: &str, revision: u32| -> Result<PathBuf, String> {
            let data = json!({
                "artifacts": json_artifacts,
                "descriptors": [
                    descriptor("store-a", 1, with("extra-a")),
                    descriptor("store-b", 1, with("extra-b")),
                    descriptor("store-broken", 1, broken_parts.clone()),
                    descriptor("store-rev", revision, with(&format!("rev-{revision}"))),
                ],
            });
            let path = dir.path().join(name);
            std::fs::write(&path, data.to_string()).map_err(|e| e.to_string())?;
            Ok(path)
        };
        let descriptors = write("descriptors.json", 2)?;
        let older = write("older.json", 1)?;

        let mut suite = suite.clone();
        suite.env.push((HF_ENV.into(), format!("http://127.0.0.1:{}", server.port)));
        // From here on, no file comes from the network.
        suite.load_timeout = STEP_TIMEOUT;
        Ok(Fixture { suite, engine_json: model.engine.clone(), format, server, descriptors, older, artifacts, names, _dir: dir })
    }

    fn fields(&self, model: &str) -> Value {
        json!({ "model": model, "engine": self.engine_json })
    }
}

// ---------------------------------------------------------------------------
// One store and its engines
// ---------------------------------------------------------------------------

struct Run<'a> {
    fx: &'a Fixture,
    dir: tempfile::TempDir,
}

/// An engine with the test hook.
struct Hooked<'a> {
    fx: &'a Fixture,
    engine: Engine,
    hook: tempfile::TempDir,
}

impl<'a> Run<'a> {
    fn new(fx: &'a Fixture) -> Result<Run<'a>, String> {
        Ok(Run { fx, dir: tempfile::tempdir().map_err(|e| e.to_string())? })
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("v1")
    }

    /// Start an engine, stopped at `hold` when it gets there.
    fn start_with(&self, descriptors: &Path, hold: Option<&str>) -> Result<Hooked<'a>, String> {
        let hook = tempfile::tempdir().map_err(|e| e.to_string())?;
        if let Some(step) = hold {
            std::fs::write(hook.path().join(format!("hold-{step}")), b"").map_err(|e| e.to_string())?;
        }
        let mut suite = self.fx.suite.clone();
        suite.env.push((STORE_ENV.into(), self.root().display().to_string()));
        suite.env.push((HOOK_ENV.into(), hook.path().display().to_string()));
        suite.env.push((DESCRIPTORS_ENV.into(), descriptors.display().to_string()));
        Ok(Hooked { fx: self.fx, engine: Engine::start(&suite, None)?, hook })
    }

    /// Start an engine and wait for the cleanup of its start.
    fn start(&self) -> Result<Hooked<'a>, String> {
        let engine = self.start_with(&self.fx.descriptors, None)?;
        engine.wait_at("cleanup.done")?;
        engine.forget("cleanup.done");
        Ok(engine)
    }

    /// Make models complete with an engine that then ends.
    fn prepare(&self, models: &[&str]) -> Result<(), String> {
        let mut engine = self.start()?;
        for model in models {
            engine.request_ok("download", model)?;
        }
        engine.end()
    }

    /// What a crash leaves: a model whose manifest went, so its artifacts have
    /// no manifest, and files in `staging` and `trash`.
    fn leave_garbage(&self, model: &str) -> Result<(), String> {
        let root = self.root();
        let manifest = root.join("models").join(model).join(format!("{}.json", self.fx.format));
        std::fs::remove_file(&manifest).map_err(|e| format!("cannot remove {}: {e}", manifest.display()))?;
        let stopped = root.join("staging").join(format!("{}.1.stopped", self.fx.names["rev-1"]));
        std::fs::create_dir_all(stopped.join("files")).map_err(|e| e.to_string())?;
        std::fs::write(stopped.join("owner"), "1\n").map_err(|e| e.to_string())?;
        std::fs::create_dir_all(root.join("trash").join("old.1").join("folder")).map_err(|e| e.to_string())
    }

    /// What must always be true: each manifest names artifacts that exist, and
    /// each artifact has its files, whole.
    fn check(&self) -> Result<(), String> {
        let root = self.root();
        for model in list(&root.join("models"))? {
            for file in list(&root.join("models").join(&model))? {
                let path = root.join("models").join(&model).join(&file);
                let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                let manifest: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{file} of {model} does not parse: {e}"))?;
                for part in manifest["parts"].as_array().into_iter().flatten() {
                    let digest = part["digest"].as_str().unwrap_or_default();
                    if !root.join("artifacts").join(digest).is_dir() {
                        return Err(format!("the manifest of {model} names the missing artifact {digest}"));
                    }
                }
            }
        }
        for digest in list(&root.join("artifacts"))? {
            let Some(want) = self.fx.artifacts.get(&digest) else {
                return Err(format!("the artifact {digest} is not in the descriptors"));
            };
            if &read_tree(&root.join("artifacts").join(&digest))? != want {
                return Err(format!("the artifact {digest} does not have the files of its descriptor"));
            }
        }
        Ok(())
    }

    /// After the cleanup of a new engine, with nothing else running: `check`,
    /// no artifact without a manifest, and nothing in `staging` and `trash`.
    fn check_clean(&self) -> Result<(), String> {
        self.start()?.end()?;
        self.check()?;
        let root = self.root();
        let mut named = BTreeSet::new();
        for model in list(&root.join("models"))? {
            named.extend(read_manifest(&root, &model)?.parts.into_iter().map(|(_, digest)| digest));
        }
        let artifacts: BTreeSet<String> = list(&root.join("artifacts"))?.into_iter().collect();
        if artifacts != named {
            return Err(format!("the artifacts are {artifacts:?}, the manifests name {named:?}"));
        }
        for folder in ["staging", "trash"] {
            let left = list(&root.join(folder))?;
            if !left.is_empty() {
                return Err(format!("{folder} still has {left:?}"));
            }
        }
        Ok(())
    }
}

impl Hooked<'_> {
    fn hold(&self, step: &str) -> Result<(), String> {
        std::fs::write(self.hook.path().join(format!("hold-{step}")), b"").map_err(|e| e.to_string())
    }

    fn release(&self, step: &str) -> Result<(), String> {
        std::fs::remove_file(self.hook.path().join(format!("hold-{step}"))).map_err(|e| e.to_string())
    }

    fn forget(&self, step: &str) {
        let _ = std::fs::remove_file(self.hook.path().join(format!("at-{step}")));
    }

    fn wait_at(&self, step: &str) -> Result<(), String> {
        let deadline = Instant::now() + STEP_TIMEOUT;
        while !self.hook.path().join(format!("at-{step}")).exists() {
            if Instant::now() >= deadline {
                return Err(format!("the engine did not get to the step {step}"));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }

    fn send(&mut self, kind: &str, model: &str) -> Result<String, String> {
        self.engine.send(kind, self.fx.fields(model))
    }

    fn request(&mut self, kind: &str, model: &str) -> Result<Reply, String> {
        let id = self.send(kind, model)?;
        self.engine.wait_reply(&id, self.fx.suite.load_timeout)
    }

    fn request_ok(&mut self, kind: &str, model: &str) -> Result<Reply, String> {
        let reply = self.request(kind, model)?;
        reply.expect_type("ok").map_err(|e| format!("{kind} {model}: {e}"))?;
        Ok(reply)
    }

    /// The state of a model in `list_models`.
    fn state(&mut self, model: &str) -> Result<String, String> {
        let entry = json!({ "id": model, "engine": self.fx.engine_json });
        let reply = self.engine.request("list_models", json!({ "models": [entry] }), self.fx.suite.timeout)?;
        reply.expect_type("ok")?;
        let state = reply.events.iter().find(|e| e["type"] == "model_state" && e["model"] == json!(model));
        state.and_then(|e| e["state"].as_str()).map(str::to_string).ok_or_else(|| format!("no state for {model}"))
    }

    fn expect_state(&mut self, model: &str, want: &str) -> Result<(), String> {
        match self.state(model)? {
            state if state == want => Ok(()),
            state => Err(format!("{model} is {state}, not {want}")),
        }
    }

    /// Run the loaded VAD model on a second of silence, so it is sure that it works.
    fn use_vad(&mut self, model: &str, wav: &Path) -> Result<(), String> {
        let fields = json!({ "model": model, "path": wav.display().to_string() });
        let reply = self.engine.request("vad", fields, self.fx.suite.load_timeout)?;
        reply.expect_type("ok").map_err(|e| format!("vad {model}: {e}"))
    }

    /// End the engine. It must have left its own models folder empty, and each
    /// line it wrote must be right.
    fn end(self) -> Result<(), String> {
        let left = list(self.engine.models.path())?;
        if !left.is_empty() {
            return Err(format!("the engine wrote {left:?} in its models folder"));
        }
        self.engine.finish(self.fx.suite.timeout)
    }

    /// Kill the engine, as a crash does.
    fn kill(mut self) -> Result<(), String> {
        self.engine.child.kill().map_err(|e| format!("cannot kill the engine: {e}"))?;
        self.engine.child.wait().map_err(|e| format!("cannot wait for the engine: {e}"))?;
        Ok(())
    }
}

/// Wait until `done` is true. An artifact goes shortly after its last use, on another thread.
fn eventually(what: &str, done: impl Fn() -> bool) -> Result<(), String> {
    let deadline = Instant::now() + STEP_TIMEOUT;
    while !done() {
        if Instant::now() >= deadline {
            return Err(format!("{what} did not happen"));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The checks
// ---------------------------------------------------------------------------

pub(super) fn store_level(suite: &Suite, model: &Model, report: &mut Report) {
    let fx = match Fixture::new(suite, model) {
        Ok(fx) => {
            report.record("store: a download with the built-in descriptors", Ok(()));
            fx
        }
        Err(why) => {
            report.record("store: a download with the built-in descriptors", Err(why));
            return;
        }
    };
    let wav_dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(e) => return report.record("store: a temporary folder", Err(e.to_string())),
    };
    let wav = wav_dir.path().join("silence.wav");
    if let Err(e) = write_wav(&wav, &[0; 16_000]) {
        return report.record("store: a WAV file", Err(e.to_string()));
    }
    report.record("store: two engines make one download for two requests", one_download(&fx));
    report.record("store: a loaded model stays usable when another engine deletes it", loaded_stays(&fx, &wav));
    report.record("store: a shared artifact stays when the other model goes", shared_stays(&fx, &wav));
    report.record("store: a manifest does not go back to a lower revision", no_lower_revision(&fx));
    report.record("store: a load that fails does not write in the store", failed_load(&fx));

    for (op, steps) in [(Op::Download, &DOWNLOAD_STEPS[..]), (Op::Load, &LOAD_STEPS[..]), (Op::Delete, &DELETE_STEPS[..]), (Op::Cleanup, &CLEANUP_STEPS[..])] {
        for step in steps {
            for other in [Other::Download("store-a"), Other::Download("store-b"), Other::Load("store-a"), Other::Delete("store-a")] {
                let name = format!("store: stopped at {step}, the other engine does {other:?}");
                report.record(&name, stopped_at(&fx, op, step, other, &wav));
            }
            report.record(&format!("store: killed at {step}"), killed_at(&fx, op, step));
        }
    }
}

/// Two requests of one model: one download, and the second engine reports `downloading`.
fn one_download(fx: &Fixture) -> Result<(), String> {
    let run = Run::new(fx)?;
    let mut first = run.start()?;
    let mut second = run.start()?;
    first.hold("download.fetched")?;
    let first_id = first.send("download", "store-a")?;
    first.wait_at("download.fetched")?;
    let second_id = second.send("download", "store-a")?;
    std::thread::sleep(CONTENTION);
    second.engine.drain();
    // One `downloading` at the start of each download, and one more for the wait.
    let reports = second.engine.pending.iter().filter(|m| m["id"] == json!(second_id) && m["type"] == "model_state" && m["state"] == "downloading");
    if reports.count() < 2 {
        return Err(format!("the second engine did not report that it waits: {:?}", second.engine.pending));
    }
    first.release("download.fetched")?;
    first.engine.wait_reply(&first_id, fx.suite.load_timeout)?.expect_type("ok")?;
    second.engine.wait_reply(&second_id, fx.suite.load_timeout)?.expect_type("ok")?;
    let requests = fx.server.requests.lock().unwrap().clone();
    let repeated: Vec<_> = requests.iter().filter(|(_, count)| **count > 1).collect();
    if !repeated.is_empty() {
        return Err(format!("files were downloaded twice: {repeated:?}"));
    }
    first.expect_state("store-a", "downloaded")?;
    second.expect_state("store-a", "downloaded")?;
    run.check()?;
    first.end()?;
    second.end()?;
    run.check_clean()
}

fn loaded_stays(fx: &Fixture, wav: &Path) -> Result<(), String> {
    let run = Run::new(fx)?;
    run.prepare(&["store-a"])?;
    let mut holder = run.start()?;
    let mut other = run.start()?;
    holder.request_ok("load", "store-a")?;
    other.request_ok("delete", "store-a")?;
    other.expect_state("store-a", "not_downloaded")?;
    holder.expect_state("store-a", "ready")?;
    holder.use_vad("store-a", wav)?;
    let extra = run.root().join("artifacts").join(&fx.names["extra-a"]);
    if !extra.is_dir() {
        return Err("the artifacts went while the model was loaded".into());
    }
    holder.request_ok("unload", "store-a")?;
    holder.expect_state("store-a", "not_downloaded")?;
    eventually("the removal of the artifacts after the unload", || !extra.exists())?;
    holder.end()?;
    other.end()?;
    run.check_clean()
}

fn shared_stays(fx: &Fixture, wav: &Path) -> Result<(), String> {
    let run = Run::new(fx)?;
    run.prepare(&["store-a", "store-b"])?;
    let mut engine = run.start()?;
    engine.request_ok("delete", "store-a")?;
    engine.expect_state("store-a", "not_downloaded")?;
    engine.expect_state("store-b", "downloaded")?;
    if run.root().join("artifacts").join(&fx.names["extra-a"]).exists() {
        return Err("the artifact of the deleted model stayed".into());
    }
    engine.request_ok("load", "store-b")?;
    engine.use_vad("store-b", wav)?;
    engine.end()?;
    run.check_clean()
}

fn no_lower_revision(fx: &Fixture) -> Result<(), String> {
    let run = Run::new(fx)?;
    // With the shared artifacts on disk, each engine waits only for its own `rev` artifact.
    run.prepare(&["store-a"])?;
    let mut older = run.start_with(&fx.older, None)?;
    older.wait_at("cleanup.done")?;
    older.hold("download.fetched")?;
    let older_id = older.send("download", "store-rev")?;
    older.wait_at("download.fetched")?;

    let mut newer = run.start()?;
    newer.request_ok("download", "store-rev")?;
    newer.expect_state("store-rev", "downloaded")?;

    older.release("download.fetched")?;
    older.engine.wait_reply(&older_id, fx.suite.load_timeout)?.expect_type("ok")?;
    let revision = read_manifest(&run.root(), "store-rev")?.revision;
    if revision != 2 {
        return Err(format!("the manifest has the revision {revision}, not 2"));
    }
    older.expect_state("store-rev", "downloaded")?;
    newer.expect_state("store-rev", "downloaded")?;
    run.check()?;
    older.end()?;
    newer.end()?;
    run.check_clean()
}

/// Everything under the store but its lock files.
fn snapshot(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut tree = read_tree(root)?;
    tree.retain(|path, _| !path.starts_with("locks/"));
    Ok(tree)
}

fn failed_load(fx: &Fixture) -> Result<(), String> {
    let run = Run::new(fx)?;
    run.prepare(&["store-broken"])?;
    let before = snapshot(&run.root())?;
    let mut engine = run.start()?;
    // A library can end the process on a broken model, which is not a good reply
    // but leaves the store as it was. The next engine finds the model.
    match engine.request("load", "store-broken") {
        Ok(reply) => {
            reply.expect_error()?;
            reply.expect_state("store-broken", "failed")?;
            engine.end()?;
        }
        Err(why) if why.contains("closed stdout") => {}
        Err(why) => return Err(why),
    }
    if snapshot(&run.root())? != before {
        return Err("the load changed the store".into());
    }
    let mut next = run.start()?;
    next.expect_state("store-broken", "downloaded")?;
    next.end()
}

/// What the first engine does when it stops at a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Download,
    Load,
    Delete,
    Cleanup,
}

/// What the second engine does while the first is stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Other {
    Download(&'static str),
    Load(&'static str),
    Delete(&'static str),
}

impl Other {
    fn kind(self) -> &'static str {
        match self {
            Other::Download(_) => "download",
            Other::Load(_) => "load",
            Other::Delete(_) => "delete",
        }
    }

    fn model(self) -> &'static str {
        match self {
            Other::Download(model) | Other::Load(model) | Other::Delete(model) => model,
        }
    }
}

/// Make the store that `op` needs, and start the engine that stops at `step`.
fn start_stopped<'a>(run: &Run<'a>, op: Op, step: &str) -> Result<(Hooked<'a>, Option<String>), String> {
    match op {
        Op::Download => {}
        Op::Load | Op::Delete => run.prepare(&["store-a"])?,
        Op::Cleanup => {
            run.prepare(&["store-a"])?;
            run.leave_garbage("store-a")?;
        }
    }
    if op == Op::Cleanup {
        let engine = run.start_with(&run.fx.descriptors, Some(step))?;
        engine.wait_at(step)?;
        return Ok((engine, None));
    }
    let mut engine = run.start()?;
    engine.hold(step)?;
    let id = engine.send(if op == Op::Load { "load" } else if op == Op::Delete { "delete" } else { "download" }, "store-a")?;
    engine.wait_at(step)?;
    Ok((engine, Some(id)))
}

/// The first engine stops at `step` of `op`. The second does `other`, or waits
/// for a lock of the first. Then the first goes on. Both must end, and the store
/// must be right at each point.
fn stopped_at(fx: &Fixture, op: Op, step: &str, other: Other, wav: &Path) -> Result<(), String> {
    let run = Run::new(fx)?;
    let (mut first, first_id) = start_stopped(&run, op, step)?;
    run.check()?;

    let mut second = run.start_with(&fx.descriptors, None)?;
    let second_id = second.send(other.kind(), other.model())?;
    let early = second.engine.wait_reply(&second_id, CONTENTION);
    first.release(step)?;

    let first_reply = match &first_id {
        Some(id) => Some(first.engine.wait_reply(id, fx.suite.load_timeout)?),
        None => None,
    };
    if op == Op::Cleanup {
        first.wait_at("cleanup.done")?;
    }
    let second_reply = match early {
        Ok(reply) => reply,
        Err(_) => second.engine.wait_reply(&second_id, fx.suite.load_timeout)?,
    };
    run.check()?;

    // A download and a delete always end well. A load finds the model, or says that it is gone.
    match (op, &first_reply) {
        (Op::Load, Some(reply)) if reply.kind() == "ok" => first.use_vad("store-a", wav)?,
        (Op::Load, Some(reply)) if !matches!(other, Other::Delete(_)) => return Err(format!("the load failed: {}", reply.reply)),
        (Op::Load, Some(reply)) => reply.expect_state("store-a", "not_downloaded")?,
        (_, Some(reply)) => reply.expect_type("ok")?,
        (_, None) => {}
    }
    match other {
        Other::Load(model) if second_reply.kind() == "ok" => second.use_vad(model, wav)?,
        Other::Load(model) if op == Op::Load => return Err(format!("the load of {model} failed: {}", second_reply.reply)),
        Other::Load(model) => second_reply.expect_state(model, "not_downloaded")?,
        _ => second_reply.expect_type("ok")?,
    }
    // A model that a download finished is there, unless the other engine deleted it after that.
    if let Other::Download(model) = other {
        let deleted = op == Op::Delete && model == "store-a";
        if !deleted {
            second.expect_state(model, "downloaded")?;
        }
    }

    first.engine.request("unload", fx.fields("store-a"), fx.suite.timeout)?;
    second.engine.request("unload", fx.fields(other.model()), fx.suite.timeout)?;
    first.end()?;
    second.end()?;
    run.check_clean()
}

/// The engine is killed at `step` of `op`. The next start makes the store clean.
/// A model that was complete stays, and the interrupted work can run again.
fn killed_at(fx: &Fixture, op: Op, step: &str) -> Result<(), String> {
    let run = Run::new(fx)?;
    run.prepare(&["store-b"])?;
    let (victim, _) = start_stopped(&run, op, step)?;
    run.check()?;
    victim.kill()?;
    run.check()?;
    run.check_clean()?;
    let mut next = run.start()?;
    next.expect_state("store-b", "downloaded")?;
    next.request_ok("download", "store-a")?;
    next.expect_state("store-a", "downloaded")?;
    next.end()?;
    run.check_clean()
}
