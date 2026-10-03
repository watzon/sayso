//! Request handlers, the stream worker, and the stdin loop.
//!
//! Threads, as in the Swift engine:
//! - The stdin loop reads requests. It never runs a model.
//! - Each request that has one reply (`hello`, `list_models`, `download`,
//!   `delete`, `load`, `transcribe`) runs on its own thread, so a long
//!   download or load never blocks other requests.
//! - `unload` runs on the stdin loop, so a later `load` of the same model
//!   finds the model gone.
//! - The streaming requests (`stream_start`, `stream_audio`, `stream_end`)
//!   run in arrival order on one stream worker thread.

use crate::audio;
use crate::download::{self, Progress};
use crate::layout;
use crate::partials::Partials;
use crate::protocol::{Output, PROTOCOL_VERSION, Request};
use crate::recognizer::{LivePreview, Loaded, Loader, ModelDirs, PreviewStream};
use crate::spec::ModelSpec;
use crate::store::ModelStore;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Settings of one engine process.
#[derive(Debug, Clone)]
pub struct Config {
    /// `SAYSO_MODELS_DIR`.
    pub models_dir: PathBuf,
    /// Where the archives come from. `SAYSO_MODELS_URL` overrides the sherpa-onnx release.
    pub base_url: String,
    /// Shortest time between two decodes of a live preview stream.
    pub decode_interval: Duration,
}

impl Config {
    pub fn from_env(models_dir: PathBuf) -> Self {
        let base_url = std::env::var("SAYSO_MODELS_URL")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| download::DEFAULT_BASE_URL.to_string());
        Config {
            models_dir,
            base_url,
            decode_interval: Duration::from_millis(150),
        }
    }
}

#[derive(Default)]
struct State {
    loaded: HashMap<String, (ModelSpec, Loaded)>,
    /// Models with a download, delete, or load in progress.
    busy: HashSet<String>,
}

pub struct Engine {
    config: Config,
    store: ModelStore,
    out: Arc<Output>,
    loader: Arc<dyn Loader>,
    agent: ureq::Agent,
    state: Mutex<State>,
}

/// Marks a model busy until it is dropped.
struct Busy<'a> {
    engine: &'a Engine,
    model: String,
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.engine.state().busy.remove(&self.model);
    }
}

impl Engine {
    pub fn new(config: Config, out: Arc<Output>, loader: Arc<dyn Loader>) -> Self {
        Engine {
            store: ModelStore::new(config.models_dir.clone()),
            config,
            out,
            loader,
            agent: download::agent(),
            state: Mutex::new(State::default()),
        }
    }

    pub fn out(&self) -> &Output {
        &self.out
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn acquire(&self, model: &str) -> Result<Busy<'_>> {
        if !self.state().busy.insert(model.to_string()) {
            bail!("model {model} is busy");
        }
        Ok(Busy {
            engine: self,
            model: model.to_string(),
        })
    }

    fn model_state(&self, id: Option<&str>, model: &str, state: &str, extra: Value) {
        let mut fields = json!({ "model": model, "state": state });
        if let (Value::Object(fields), Value::Object(extra)) = (&mut fields, extra) {
            fields.extend(extra);
        }
        self.out.send("model_state", id, fields);
    }

    /// Handle one request that has a single reply. Errors become `error` replies.
    pub fn handle(&self, request: &Request) {
        let id = request.id();
        let result = match request.kind.as_str() {
            "hello" => {
                self.out.send(
                    "hello",
                    id,
                    json!({ "version": env!("CARGO_PKG_VERSION"), "protocol": PROTOCOL_VERSION }),
                );
                Ok(())
            }
            "list_models" => self.list_models(request),
            "download" => self.download(request),
            "delete" => self.delete(request),
            "load" => self.load(request),
            "unload" => self.unload(request),
            "transcribe" => self.transcribe(request),
            other => Err(anyhow!("unknown request type {other}")),
        };
        if let Err(e) = result {
            self.out.error(&format!("{e:#}"), id);
        }
    }

    // -----------------------------------------------------------------------
    // Status
    // -----------------------------------------------------------------------

    /// The state of a model and the extra fields of its `model_state`. A model
    /// whose final pass is out and whose live preview stays is `downloaded`
    /// with `preview: true`.
    fn status(&self, model: &str, spec: &ModelSpec) -> (&'static str, Value) {
        match self.state().loaded.get(model) {
            Some((spec, loaded)) if final_pass_is_out(spec, loaded) => {
                ("downloaded", json!({ "preview": true }))
            }
            Some(_) => ("ready", json!({})),
            None if self.store.is_downloaded(model, spec) => ("downloaded", json!({})),
            None => ("not_downloaded", json!({})),
        }
    }

    /// `list_models`: one `model_state` per model, from the disk only.
    fn list_models(&self, request: &Request) -> Result<()> {
        let list = request
            .value("models")?
            .as_array()
            .context("models is not an array")?;
        let mut models = Vec::with_capacity(list.len());
        for entry in list {
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .context("models[].id is missing")?;
            let engine = entry.get("engine").context("models[].engine is missing")?;
            let spec = ModelSpec::from_json(engine).with_context(|| format!("model {id}"))?;
            models.push((id.to_string(), spec));
        }
        for (model, spec) in models {
            let (status, extra) = self.status(&model, &spec);
            self.model_state(request.id(), &model, status, extra);
        }
        self.out.ok(request.id(), json!({}));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Download and delete
    // -----------------------------------------------------------------------

    fn download(&self, request: &Request) -> Result<()> {
        let id = request.id();
        let model = request.string("model")?;
        let spec = ModelSpec::from_json(request.value("engine")?)?;
        self.store.folder(model)?;
        if self.store.is_downloaded(model, &spec) {
            self.model_state(id, model, "downloaded", json!({}));
            self.out.ok(id, json!({}));
            return Ok(());
        }
        let _busy = self.acquire(model)?;
        let mut progress = Progress::new(request.optional_u64("size_bytes").unwrap_or(0));
        progress.due(Instant::now());
        self.report(id, model, &progress, 0.0);
        if let Err(e) = self.fetch(id, model, &spec, &mut progress) {
            self.model_state(id, model, "failed", json!({ "message": format!("{e:#}") }));
            return Err(e);
        }
        self.report(id, model, &progress, 1.0);
        self.model_state(id, model, "downloaded", json!({}));
        self.out.ok(id, json!({}));
        Ok(())
    }

    /// Send `download_progress` and the matching `model_state`.
    fn report(&self, id: Option<&str>, model: &str, progress: &Progress, fraction: f64) {
        let fields = json!({
            "model": model,
            "fraction": fraction,
            "bytes_done": progress.bytes_done(),
            "bytes_total": progress.bytes_total(),
        });
        self.out.send("download_progress", id, fields.clone());
        self.model_state(id, model, "downloading", fields);
    }

    /// Download the archives that are missing, check the files, and mark the model complete.
    fn fetch(
        &self,
        id: Option<&str>,
        model: &str,
        spec: &ModelSpec,
        progress: &mut Progress,
    ) -> Result<()> {
        self.store.prepare_download(model)?;
        for archive in spec.archives() {
            let dest = self.store.archive_dir(model, archive)?;
            if dest.is_dir() {
                continue; // A complete archive from a download that stopped later.
            }
            let staging = self.store.staging_dir(model, archive)?;
            let url = download::archive_url(&self.config.base_url, archive);
            download::fetch_archive(&self.agent, &url, &staging, &dest, &mut |bytes| {
                progress.add(bytes);
                if progress.due(Instant::now()) {
                    self.report(id, model, progress, progress.fraction());
                }
            })?;
        }
        check_files(spec, &self.dirs(model, spec)?)?;
        self.store.mark_downloaded(model, spec)?;
        if !self.store.is_downloaded(model, spec) {
            bail!("download finished but the files are incomplete");
        }
        Ok(())
    }

    fn delete(&self, request: &Request) -> Result<()> {
        let model = request.string("model")?;
        let _busy = self.acquire(model)?;
        self.evict(model);
        self.store.delete(model)?;
        self.model_state(request.id(), model, "not_downloaded", json!({}));
        self.out.ok(request.id(), json!({}));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Load
    // -----------------------------------------------------------------------

    fn dirs(&self, model: &str, spec: &ModelSpec) -> Result<ModelDirs> {
        Ok(ModelDirs {
            main: self.store.archive_dir(model, &spec.archive)?,
            stream: match spec.stream_archive() {
                Some(archive) => Some(self.store.archive_dir(model, archive)?),
                None => None,
            },
        })
    }

    fn load(&self, request: &Request) -> Result<()> {
        let id = request.id();
        let model = request.string("model")?;
        let spec = ModelSpec::from_json(request.value("engine")?)?;
        // The live preview that an `unload` with `keep_preview` left in memory.
        let kept_preview = match self.state().loaded.get(model) {
            Some((s, loaded)) if *s == spec && final_pass_is_out(s, loaded) => {
                loaded.preview.clone()
            }
            Some((s, _)) if *s == spec => {
                self.model_state(id, model, "ready", json!({ "load_ms": 0 }));
                self.out.ok(id, json!({}));
                return Ok(());
            }
            _ => None,
        };
        if !self.store.is_downloaded(model, &spec) {
            bail!("model {model} is not downloaded");
        }
        let _busy = self.acquire(model)?;
        self.model_state(id, model, "optimizing", json!({}));
        let start = Instant::now();
        let dirs = self.dirs(model, &spec)?;
        // With the live preview in memory, only the final pass loads.
        let result = match kept_preview {
            Some(preview) => self
                .loader
                .load_final_pass(&spec, &dirs)
                .map(|final_pass| Loaded {
                    final_pass,
                    preview: Some(preview),
                }),
            None => self.loader.load(&spec, &dirs),
        };
        let loaded = match result {
            Ok(loaded) => loaded,
            Err(e) => {
                self.model_state(id, model, "failed", json!({ "message": format!("{e:#}") }));
                return Err(e);
            }
        };
        self.state()
            .loaded
            .insert(model.to_string(), (spec, loaded));
        let load_ms = start.elapsed().as_millis() as u64;
        self.model_state(id, model, "ready", json!({ "load_ms": load_ms }));
        self.out.ok(id, json!({}));
        Ok(())
    }

    /// `unload`: take a model out of memory. Its files stay on disk. With
    /// `keep_preview`, a model that has a live preview keeps it, and only its
    /// final pass leaves.
    fn unload(&self, request: &Request) -> Result<()> {
        let model = request.string("model")?;
        let keep_preview = request
            .value("keep_preview")
            .ok()
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let _busy = self.acquire(model)?;
        let has_preview = self
            .state()
            .loaded
            .get(model)
            .is_some_and(|(_, loaded)| loaded.preview.is_some());
        if keep_preview && has_preview {
            if self.evict_final_pass(model) {
                let extra = json!({ "preview": true });
                self.model_state(request.id(), model, "downloaded", extra);
            }
        } else if self.evict(model) {
            self.model_state(request.id(), model, "downloaded", json!({}));
        }
        self.out.ok(request.id(), json!({}));
        Ok(())
    }

    /// Drop the final pass of a loaded model and keep its live preview. False
    /// when the model had no final pass in memory.
    fn evict_final_pass(&self, model: &str) -> bool {
        let removed = match self.state().loaded.get_mut(model) {
            Some((_, loaded)) => loaded.final_pass.take(),
            None => None,
        };
        let was_loaded = removed.is_some();
        drop(removed);
        if was_loaded {
            release_free_memory();
        }
        was_loaded
    }

    /// Drop a loaded model and give its memory back to the system. False when
    /// the model was not loaded. A final pass or a stream that runs now keeps
    /// its part of the model until it ends.
    fn evict(&self, model: &str) -> bool {
        let removed = self.state().loaded.remove(model);
        let was_loaded = removed.is_some();
        drop(removed);
        if was_loaded {
            release_free_memory();
        }
        was_loaded
    }

    // -----------------------------------------------------------------------
    // Final pass
    // -----------------------------------------------------------------------

    fn transcribe(&self, request: &Request) -> Result<()> {
        let model = request.string("model")?;
        let path = PathBuf::from(request.string("path")?);
        let language = request.optional_string("language").unwrap_or("en");
        // Dictionary words: the sherpa-onnx models in the catalog cannot use them
        // (hotwords work only for transducers with beam search), so they are ignored.
        let (spec, loaded) = self
            .state()
            .loaded
            .get(model)
            .cloned()
            .ok_or_else(|| anyhow!("model {model} is not loaded"))?;
        if final_pass_is_out(&spec, &loaded) {
            bail!("model {model} is not loaded");
        }
        let final_pass = loaded
            .final_pass
            .ok_or_else(|| anyhow!("model {model} has no final pass"))?;
        let samples = audio::read_wav(&path)?;
        if samples.is_empty() {
            bail!("audio file has no samples");
        }
        let start = Instant::now();
        let mut parts = Vec::new();
        for piece in audio::split(&samples, spec.recipe.max_chunk_seconds()) {
            parts.push(final_pass.transcribe(piece, language)?);
        }
        let text = audio::join(&parts);
        let elapsed_ms = start.elapsed().as_millis() as u64;
        self.out.send(
            "final",
            request.id(),
            json!({ "text": text.trim(), "elapsed_ms": elapsed_ms }),
        );
        Ok(())
    }

    /// The live preview of a loaded model.
    fn preview(&self, model: &str) -> Result<Arc<dyn LivePreview>> {
        let state = self.state();
        let (_, loaded) = state
            .loaded
            .get(model)
            .ok_or_else(|| anyhow!("model {model} is not loaded"))?;
        loaded
            .preview
            .clone()
            .ok_or_else(|| anyhow!("model {model} has no live preview"))
    }
}

/// Check that each archive holds the files its recipe needs.
fn check_files(spec: &ModelSpec, dirs: &ModelDirs) -> Result<()> {
    if spec.recipe.final_pass() {
        layout::offline_files(spec.recipe, &dirs.main)?;
    }
    if let Some(stream) = &dirs.stream {
        layout::online_files(stream)?;
    }
    Ok(())
}

/// True when the model has a final pass that is not in memory now: an
/// `unload` with `keep_preview` took it out.
fn final_pass_is_out(spec: &ModelSpec, loaded: &Loaded) -> bool {
    spec.recipe.final_pass() && loaded.final_pass.is_none()
}

/// Return freed heap memory to the system. Without this, glibc keeps the
/// memory of a dropped model in the process.
fn release_free_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim has no preconditions.
    unsafe {
        libc::malloc_trim(0);
    }
}

// ---------------------------------------------------------------------------
// Live preview
// ---------------------------------------------------------------------------

struct Session {
    model: String,
    stream: Box<dyn PreviewStream>,
    partials: Partials,
    last_decode: Instant,
}

/// Runs the streaming requests in arrival order.
pub struct StreamWorker {
    engine: Arc<Engine>,
    sessions: HashMap<u64, Session>,
}

impl StreamWorker {
    pub fn new(engine: Arc<Engine>) -> Self {
        StreamWorker {
            engine,
            sessions: HashMap::new(),
        }
    }

    pub fn handle(&mut self, request: &Request) {
        if let Err(e) = self.run(request) {
            self.engine.out.error(&format!("{e:#}"), request.id());
        }
    }

    fn run(&mut self, request: &Request) -> Result<()> {
        let session = request.u64("session")?;
        match request.kind.as_str() {
            "stream_start" => {
                let model = request.string("model")?;
                let preview = self.engine.preview(model)?;
                // One live stream per model. A new session replaces an older one.
                self.sessions.retain(|_, s| s.model != model);
                let stream = preview.start()?;
                let session_state = Session {
                    model: model.to_string(),
                    stream,
                    partials: Partials::default(),
                    last_decode: Instant::now(),
                };
                self.sessions.insert(session, session_state);
                self.engine.out.ok(request.id(), json!({}));
            }
            "stream_audio" => {
                let interval = self.engine.config.decode_interval;
                let state = self
                    .sessions
                    .get_mut(&session)
                    .ok_or_else(|| anyhow!("unknown stream session {session}"))?;
                let samples = audio::decode_pcm(request.string("pcm")?)?;
                state.stream.accept(&samples);
                if state.last_decode.elapsed() >= interval {
                    state.last_decode = Instant::now();
                    let segment = state.stream.decode();
                    state.partials.update(&segment);
                    send_partial(&self.engine.out, session, &mut state.partials);
                }
            }
            "stream_end" => {
                let mut state = self
                    .sessions
                    .remove(&session)
                    .ok_or_else(|| anyhow!("unknown stream session {session}"))?;
                let segment = state.stream.finish();
                state.partials.update(&segment);
                send_partial(&self.engine.out, session, &mut state.partials);
                let text = state.partials.text();
                self.engine
                    .out
                    .ok(request.id(), json!({ "session": session, "text": text }));
            }
            other => bail!("unknown request type {other}"),
        }
        Ok(())
    }
}

fn send_partial(out: &Output, session: u64, partials: &mut Partials) {
    if let Some((committed, tentative)) = partials.changed() {
        out.send(
            "partial",
            None,
            json!({ "session": session, "committed": committed, "tentative": tentative }),
        );
    }
}

/// Read requests until the input closes or a `shutdown` request arrives.
pub fn serve(input: impl BufRead, engine: Arc<Engine>) {
    let (stream_tx, stream_rx) = mpsc::channel::<Request>();
    let worker_engine = Arc::clone(&engine);
    let spawned = std::thread::Builder::new()
        .name("stream".into())
        .spawn(move || {
            let mut worker = StreamWorker::new(worker_engine);
            for request in stream_rx {
                worker.handle(&request);
            }
        });
    if let Err(e) = spawned {
        engine
            .out
            .error(&format!("cannot start the stream thread: {e}"), None);
        return;
    }
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let request = match Request::parse(&line) {
            Ok(request) => request,
            Err(e) => {
                engine.out.error(&e.to_string(), None);
                continue;
            }
        };
        match request.kind.as_str() {
            "stream_start" | "stream_audio" | "stream_end" => {
                let _ = stream_tx.send(request);
            }
            "shutdown" => {
                engine.out.ok(request.id(), json!({}));
                return;
            }
            // In arrival order: a `load` that follows must not find the model.
            "unload" => engine.handle(&request),
            _ => {
                let worker = Arc::clone(&engine);
                let job = request.clone();
                let spawned = std::thread::Builder::new()
                    .name(format!("request-{}", request.kind))
                    .spawn(move || worker.handle(&job));
                if spawned.is_err() {
                    // No thread for it: run it here, late but with a reply.
                    engine.handle(&request);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
