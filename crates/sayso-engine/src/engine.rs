//! Request handlers, the stream worker, and the stdin loop.
//!
//! Threads, as in the Swift engine:
//! - The stdin loop reads requests. It never runs a model.
//! - Each request that has one reply (`hello`, `list_models`, `download`,
//!   `delete`, `load`, `transcribe`, `vad`, `diarize`) runs on its own thread,
//!   so a long download or load never blocks other requests.
//! - `unload` runs on the stdin loop, so a later `load` of the same model
//!   finds the model gone.
//! - The streaming requests (`stream_start`, `vad_start`, `stream_audio`,
//!   `stream_end`) run in arrival order on one stream worker thread. A VAD
//!   session is a stream: it has a session id and counts toward the limit.

use crate::audio;
use crate::diarization;
use crate::download::{self, Progress};
use crate::layout;
use crate::partials::Partials;
use crate::protocol::{Coded, FEATURES, Output, PROTOCOL_VERSION, Request};
use crate::timing::{self, Piece};
use crate::recognizer::{Diarizer, LivePreview, Loaded, Loader, ModelDirs, PreviewStream, VadModel};
use crate::spec::{ModelSpec, Source, Task};
use crate::store::{Legacy, ModelStore};
use crate::vad::{self, Tracker};
use sayso_core::engine_catalog;
use sayso_core::models_onnx::System;
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
    /// Where the archives come from. `SAYSO_MODELS_URL` overrides the sherpa-onnx release. The
    /// releases of the diarization models are next to it (`download::sibling_release_url`).
    pub base_url: String,
    /// Where a GGUF file comes from. `SAYSO_HF_ENDPOINT` overrides Hugging Face, as in the Windows engine.
    pub hf_endpoint: String,
    /// Shortest time between two decodes of a live preview stream.
    pub decode_interval: Duration,
}

impl Config {
    pub fn from_env(models_dir: PathBuf) -> Self {
        let base_url = std::env::var("SAYSO_MODELS_URL")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| download::DEFAULT_BASE_URL.to_string());
        let hf_endpoint = std::env::var("SAYSO_HF_ENDPOINT")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| download::DEFAULT_HF_ENDPOINT.to_string());
        Config {
            models_dir,
            base_url,
            hf_endpoint,
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
    /// The built-in catalog: `{models, retired}` (`sayso_core::engine_catalog`).
    catalog: Value,
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
            catalog: engine_catalog::json(engine_catalog::Engine::Rust(System::current())),
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
                    json!({
                        "version": env!("CARGO_PKG_VERSION"),
                        "protocol": PROTOCOL_VERSION,
                        "features": FEATURES,
                    }),
                );
                Ok(())
            }
            "list_models" => self.list_models(request),
            "download" => self.download(request),
            "delete" => self.delete(request),
            "prepare" => {
                self.out.ok(id, json!({}));
                Ok(())
            }
            "load" => self.load(request),
            "unload" => self.unload(request),
            "transcribe" => self.transcribe(request),
            "vad" => self.vad(request),
            "diarize" => self.diarize(request),
            other => Err(anyhow!("unknown request type {other}")),
        };
        if let Err(e) = result {
            self.out.failure(&e, id);
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
                return ("downloaded", json!({ "preview": true }));
            }
            Some(_) => return ("ready", json!({})),
            None => {}
        }
        if self.adopt_legacy(model, spec) == Legacy::FormatChanged {
            let message = "the files of this model are in an old format; download the model again";
            return ("failed", json!({ "message": message, "reason": "format_changed" }));
        }
        if self.store.is_downloaded(model, spec) {
            ("downloaded", json!({}))
        } else {
            ("not_downloaded", json!({}))
        }
    }

    /// Move the files of the Windows engine before 0.12 into place
    /// (`ModelStore::adopt_legacy`). A model with a download or a delete in
    /// progress is left alone. A failure leaves the files as they are.
    fn adopt_legacy(&self, model: &str, spec: &ModelSpec) -> Legacy {
        let Ok(_busy) = self.acquire(model) else {
            return Legacy::None;
        };
        match self.store.adopt_legacy(model, spec) {
            Ok(legacy) => legacy,
            Err(e) => {
                self.out.log("warning", &format!("{model}: cannot move the old files into place: {e:#}"));
                Legacy::FormatChanged
            }
        }
    }

    /// The engine of a model: the `engine` object of the request, else the one of
    /// the built-in catalog.
    fn spec_of(&self, model: &str, engine: Option<&Value>) -> Result<ModelSpec> {
        if let Some(engine) = engine {
            return ModelSpec::from_json(engine).with_context(|| format!("model {model}"));
        }
        let by_id = |list: &str| {
            self.catalog[list]
                .as_array()
                .and_then(|entries| entries.iter().find(|entry| entry["id"] == model))
        };
        if let Some(entry) = by_id("models") {
            return ModelSpec::from_json(&entry["engine"]).with_context(|| format!("model {model}"));
        }
        if let Some(retired) = by_id("retired") {
            return Err(Coded {
                code: "model_retired",
                message: format!("model {model} was taken out of the catalog. {}", retired["message"].as_str().unwrap_or_default()),
            }
            .into());
        }
        Err(Coded { code: "unknown_model", message: format!("model {model} is not in the catalog; send its engine object") }.into())
    }

    /// `list_models`: one `model_state` per model, from the disk only. Without
    /// `models`, the models of the catalog, and the reply has the catalog.
    fn list_models(&self, request: &Request) -> Result<()> {
        let (list, whole_catalog) = match request.optional_value("models") {
            Some(list) => (list.as_array().context("models is not an array")?, false),
            None => (self.catalog["models"].as_array().context("the catalog has no models")?, true),
        };
        let mut models = Vec::with_capacity(list.len());
        for entry in list {
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .context("models[].id is missing")?;
            let engine = entry.get("engine").filter(|v| !v.is_null());
            models.push((id.to_string(), self.spec_of(id, engine)?));
        }
        for (model, spec) in models {
            let (status, extra) = self.status(&model, &spec);
            self.model_state(request.id(), &model, status, extra);
        }
        let reply = match whole_catalog {
            true => self.catalog.clone(),
            false => json!({}),
        };
        self.out.ok(request.id(), reply);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Download and delete
    // -----------------------------------------------------------------------

    fn download(&self, request: &Request) -> Result<()> {
        let id = request.id();
        let model = request.string("model")?;
        let spec = self.spec_of(model, request.optional_value("engine"))?;
        self.store.folder(model)?;
        if self.adopt_legacy(model, &spec) != Legacy::FormatChanged && self.store.is_downloaded(model, &spec) {
            self.model_state(id, model, "downloaded", json!({}));
            self.out.ok(id, json!({}));
            return Ok(());
        }
        let _busy = self.acquire(model)?;
        let mut progress = Progress::new(request.optional_u64("size_bytes").unwrap_or(0));
        progress.due(Instant::now());
        self.report(id, model, &progress, 0.0);
        if let Err(e) = self.fetch(id, model, &spec, &mut progress) {
            let reason = if e.is::<Incomplete>() { "incomplete_download" } else { "download_failed" };
            self.model_state(id, model, "failed", json!({ "message": format!("{e:#}"), "reason": reason }));
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
        self.store.prepare_download(model, spec)?;
        for archive in spec.archives() {
            let dest = self.store.archive_dir(model, archive)?;
            if dest.is_dir() {
                continue; // A complete archive from a download that stopped later.
            }
            let staging = self.store.staging_dir(model, archive)?;
            let (url, file) = self.source_of(spec, archive);
            download::fetch(&self.agent, &url, file, &staging, &dest, &mut |bytes| {
                progress.add(bytes);
                if progress.due(Instant::now()) {
                    self.report(id, model, progress, progress.fraction());
                }
            })?;
        }
        check_files(spec, &self.dirs(model, spec)?).map_err(|e| anyhow::Error::new(Incomplete(format!("{e:#}"))))?;
        self.store.mark_downloaded(model, spec)?;
        if !self.store.is_downloaded(model, spec) {
            return Err(Incomplete("download finished but the files are incomplete".into()).into());
        }
        Ok(())
    }

    /// Where one archive of a model comes from: its URL, and its file name when it is one
    /// file and not a `.tar.bz2` archive.
    fn source_of<'a>(&self, spec: &'a ModelSpec, archive: &str) -> (String, Option<&'a str>) {
        let base = &self.config.base_url;
        match &spec.source {
            Source::HuggingFace { repo, file } if archive == spec.archive => {
                (download::hf_file_url(&self.config.hf_endpoint, repo, archive, file), Some(file.as_str()))
            }
            Source::SherpaFile { file } => (download::file_url(base, file), Some(file.as_str())),
            Source::Diarization { embedding } if spec.embedding_archive() == Some(archive) => {
                let release = download::sibling_release_url(base, download::EMBEDDING_RELEASE);
                (download::file_url(&release, embedding), Some(embedding.as_str()))
            }
            Source::Diarization { .. } => {
                let release = download::sibling_release_url(base, download::SEGMENTATION_RELEASE);
                (download::archive_url(&release, archive), None)
            }
            _ => (download::archive_url(base, archive), None),
        }
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
            embedding: match spec.embedding_archive() {
                Some(archive) => Some(self.store.archive_dir(model, archive)?),
                None => None,
            },
        })
    }

    fn load(&self, request: &Request) -> Result<()> {
        let id = request.id();
        let model = request.string("model")?;
        let spec = self.spec_of(model, request.optional_value("engine"))?;
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
        self.adopt_legacy(model, &spec);
        if !self.store.is_downloaded(model, &spec) {
            bail!("model {model} is not downloaded");
        }
        let busy = self.acquire(model)?;
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
                    ..Loaded::default()
                }),
            None => self.loader.load(&spec, &dirs).and_then(|loaded| {
                if !spec.redecode_preview {
                    return Ok(loaded);
                }
                let final_pass = loaded
                    .final_pass
                    .ok_or_else(|| anyhow!("model {model} has no final pass for its live preview"))?;
                Ok(crate::redecode::attach(final_pass))
            }),
        };
        let loaded = match result {
            Ok(loaded) => loaded,
            Err(e) => {
                let reason = if e.is::<crate::parakeet::NoGpu>() { "no_gpu" } else { "load_failed" };
                self.model_state(id, model, "failed", json!({ "message": format!("{e:#}"), "reason": reason }));
                drop(busy);
                release_free_memory(); // a load that fails late has freed what it allocated, into glibc
                return Err(e);
            }
        };
        self.state()
            .loaded
            .insert(model.to_string(), (spec, loaded));
        let load_ms = start.elapsed().as_millis() as u64;
        self.model_state(id, model, "ready", json!({ "load_ms": load_ms }));
        self.out.ok(id, json!({}));
        // The model is free before the trim: an `unload` or `delete` that follows the reply
        // must not find it busy.
        drop(busy);
        release_free_memory(); // a load leaves 200-320 MB free inside glibc
        Ok(())
    }

    /// `unload`: take a model out of memory. Its files stay on disk. With
    /// `keep_preview`, a model whose live preview is a model of its own (a
    /// streaming archive) keeps it, and only its final pass leaves. A redecode
    /// preview is the final pass, so all of that model leaves.
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
            .is_some_and(|(spec, loaded)| loaded.preview.is_some() && spec.stream_archive().is_some());
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
        let vocabulary: Vec<String> = request
            .value("vocabulary")
            .ok()
            .and_then(Value::as_array)
            .map(|words| words.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let (spec, loaded) = self
            .state()
            .loaded
            .get(model)
            .cloned()
            .ok_or_else(|| anyhow!("model {model} is not loaded"))?;
        check_task(model, &spec, Task::Speech)?;
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
        let with_segments = request.value("segments").ok().and_then(Value::as_bool).unwrap_or(false);
        let start = Instant::now();
        let mut parts = Vec::new();
        let mut pieces = Vec::new();
        let mut offset = 0;
        for piece in audio::split(&samples, spec.recipe.max_chunk_seconds()) {
            let start_ms = (offset * 1000 / audio::SAMPLE_RATE) as u64;
            offset += piece.len();
            let end_ms = (offset * 1000 / audio::SAMPLE_RATE) as u64;
            if with_segments {
                let timed = final_pass.transcribe_timed(piece, language, &vocabulary)?;
                parts.push(timed.text.clone());
                pieces.push(Piece { start_ms, end_ms, timed });
            } else {
                parts.push(final_pass.transcribe(piece, language, &vocabulary)?);
            }
        }
        let text = audio::join(&parts);
        let elapsed_ms = start.elapsed().as_millis() as u64;
        let mut fields = json!({ "text": text.trim(), "elapsed_ms": elapsed_ms });
        if with_segments {
            fields["segments"] = timing::to_json(&timing::segments(&pieces));
        }
        self.out.send("final", request.id(), fields);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // VAD and diarization
    // -----------------------------------------------------------------------

    /// `vad`: the parts of a WAV file with speech. It uses the same tracker as a VAD
    /// session, so a session that gets the same audio finds the same parts.
    fn vad(&self, request: &Request) -> Result<()> {
        let model = request.string("model")?;
        let path = PathBuf::from(request.string("path")?);
        let vad_model = self.vad_model(model)?;
        let samples = audio::read_wav(&path)?;
        let mut tracker = Tracker::new(vad_model.detector()?);
        let mut events = tracker.accept(&samples);
        events.extend(tracker.finish());
        let speech: Vec<Value> = vad::parts(&events)
            .into_iter()
            .map(|(start_ms, end_ms)| json!({ "start_ms": start_ms, "end_ms": end_ms }))
            .collect();
        self.out.ok(request.id(), json!({ "speech": speech }));
        Ok(())
    }

    /// `diarize`: who speaks when in a WAV file.
    fn diarize(&self, request: &Request) -> Result<()> {
        let model = request.string("model")?;
        let path = PathBuf::from(request.string("path")?);
        let speakers = speakers(request)?;
        let diarizer = self.diarizer(model)?;
        let samples = audio::read_wav(&path)?;
        let audio_ms = (samples.len() * 1000 / audio::SAMPLE_RATE) as u64;
        let raw = if samples.is_empty() { Vec::new() } else { diarizer.diarize(&samples, speakers)? };
        self.out.ok(request.id(), diarization::finish(&raw, audio_ms).to_json());
        Ok(())
    }

    /// The VAD of a loaded VAD model.
    fn vad_model(&self, model: &str) -> Result<Arc<dyn VadModel>> {
        let state = self.state();
        let (spec, loaded) = state
            .loaded
            .get(model)
            .ok_or_else(|| anyhow!("model {model} is not loaded"))?;
        check_task(model, spec, Task::Vad)?;
        loaded.vad.clone().ok_or_else(|| anyhow!("model {model} has no VAD"))
    }

    /// The diarizer of a loaded diarization model.
    fn diarizer(&self, model: &str) -> Result<Arc<dyn Diarizer>> {
        let state = self.state();
        let (spec, loaded) = state
            .loaded
            .get(model)
            .ok_or_else(|| anyhow!("model {model} is not loaded"))?;
        check_task(model, spec, Task::Diarization)?;
        loaded.diarizer.clone().ok_or_else(|| anyhow!("model {model} has no diarizer"))
    }

    /// The live preview of a loaded model.
    fn preview(&self, model: &str) -> Result<Arc<dyn LivePreview>> {
        let state = self.state();
        let (spec, loaded) = state
            .loaded
            .get(model)
            .ok_or_else(|| anyhow!("model {model} is not loaded"))?;
        check_task(model, spec, Task::Speech)?;
        loaded
            .preview
            .clone()
            .ok_or_else(|| anyhow!("model {model} has no live preview"))
    }
}

/// The `speakers` field of `diarize`: not there, or an integer of 1 or more.
fn speakers(request: &Request) -> Result<Option<u32>> {
    match request.value("speakers") {
        Err(_) | Ok(Value::Null) => Ok(None),
        Ok(value) => match value.as_u64().filter(|n| *n >= 1) {
            Some(n) => Ok(Some(u32::try_from(n).unwrap_or(u32::MAX))),
            None => bail!("speakers must be an integer of 1 or more"),
        },
    }
}

/// Fail when the model does another task than `task`. A speech request gets a speech model,
/// a VAD request a VAD model, and so on.
fn check_task(model: &str, spec: &ModelSpec, task: Task) -> Result<()> {
    let actual = spec.recipe.task();
    if actual != task {
        bail!("model {model} is a {} model, not a {} model", actual.name(), task.name());
    }
    Ok(())
}

/// A download that ended with files that are not complete. The `reason` of the
/// `failed` state is `incomplete_download` for it, and `download_failed` for any
/// other error.
#[derive(Debug)]
struct Incomplete(String);

impl std::fmt::Display for Incomplete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Incomplete {}

/// Check that each archive holds the files its recipe needs.
fn check_files(spec: &ModelSpec, dirs: &ModelDirs) -> Result<()> {
    // A GGUF or GGML model is one file, checked by its size as it downloads.
    if spec.recipe.final_pass() && !spec.recipe.single_file() {
        layout::offline_files(spec.recipe, &dirs.main)?;
    }
    if let Some(stream) = &dirs.stream {
        layout::online_files(stream)?;
    }
    match &spec.source {
        Source::SherpaFile { file } => {
            layout::vad_file(&dirs.main, file)?;
        }
        Source::Diarization { embedding } => {
            let folder = dirs.embedding.as_ref().context("a diarization model has an embedding folder")?;
            layout::diarization_files(&dirs.main, folder, embedding)?;
        }
        _ => {}
    }
    Ok(())
}

/// True when the model has a final pass that is not in memory now: an
/// `unload` with `keep_preview` took it out.
fn final_pass_is_out(spec: &ModelSpec, loaded: &Loaded) -> bool {
    spec.recipe.final_pass() && loaded.final_pass.is_none()
}

/// Return freed heap memory to the system. Without this, glibc keeps the
/// memory of a load or of a dropped model in the process.
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

/// A stream: a live preview, or a VAD session.
enum Session {
    Preview(PreviewSession),
    Vad(Tracker),
}

struct PreviewSession {
    stream: Box<dyn PreviewStream>,
    partials: Partials,
    last_decode: Instant,
}

/// The most streams that are open at the same time, on all models together.
/// The protocol asks for at least 3: a dictation, and the two sources of a
/// live session.
const MAX_STREAMS: usize = 8;

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
            self.engine.out.failure(&e, request.id());
        }
    }

    /// Make room for the session `session`. A session id that is open already starts again
    /// (`concurrent_streams`). The limit counts preview streams and VAD sessions together.
    fn reserve(&mut self, session: u64) -> Result<()> {
        self.sessions.remove(&session);
        if self.sessions.len() >= MAX_STREAMS {
            return Err(Coded {
                code: "too_many_streams",
                message: format!("{MAX_STREAMS} streams are open; end one first"),
            }
            .into());
        }
        Ok(())
    }

    fn run(&mut self, request: &Request) -> Result<()> {
        let session = request.u64("session")?;
        match request.kind.as_str() {
            "stream_start" => {
                let model = request.string("model")?;
                let preview = self.engine.preview(model)?;
                self.reserve(session)?;
                let stream = preview.start()?;
                let session_state = PreviewSession {
                    stream,
                    partials: Partials::default(),
                    last_decode: Instant::now(),
                };
                self.sessions.insert(session, Session::Preview(session_state));
                self.engine.out.ok(request.id(), json!({}));
            }
            "vad_start" => {
                let model = request.string("model")?;
                let vad_model = self.engine.vad_model(model)?;
                self.reserve(session)?;
                let tracker = Tracker::new(vad_model.detector()?);
                self.sessions.insert(session, Session::Vad(tracker));
                self.engine.out.ok(request.id(), json!({}));
            }
            "stream_audio" => {
                let interval = self.engine.config.decode_interval;
                let state = self
                    .sessions
                    .get_mut(&session)
                    .ok_or_else(|| anyhow!("unknown stream session {session}"))?;
                let samples = audio::decode_pcm(request.string("pcm")?)?;
                match state {
                    Session::Preview(state) => {
                        state.stream.accept(&samples);
                        if state.last_decode.elapsed() >= interval {
                            state.last_decode = Instant::now();
                            // After an ended segment, ask again at once: a stream can end a
                            // segment and start the next one in the same decode.
                            loop {
                                let segment = state.stream.decode();
                                state.partials.update(&segment);
                                if !segment.endpoint {
                                    break;
                                }
                            }
                            send_partial(&self.engine.out, session, &mut state.partials);
                        }
                    }
                    Session::Vad(tracker) => send_speech(&self.engine.out, session, tracker.accept(&samples)),
                }
            }
            "stream_end" => {
                let state = self
                    .sessions
                    .remove(&session)
                    .ok_or_else(|| anyhow!("unknown stream session {session}"))?;
                match state {
                    Session::Preview(mut state) => {
                        let segment = state.stream.finish();
                        state.partials.update(&segment);
                        send_partial(&self.engine.out, session, &mut state.partials);
                        let text = state.partials.text();
                        self.engine
                            .out
                            .ok(request.id(), json!({ "session": session, "text": text }));
                    }
                    Session::Vad(mut tracker) => {
                        send_speech(&self.engine.out, session, tracker.finish());
                        self.engine.out.ok(request.id(), json!({ "session": session }));
                    }
                }
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

/// Send the `speech_start` and `speech_end` events of a VAD session.
fn send_speech(out: &Output, session: u64, events: Vec<vad::Event>) {
    for event in events {
        match event {
            vad::Event::Start { start_ms } => {
                out.send("speech_start", None, json!({ "session": session, "start_ms": start_ms }));
            }
            vad::Event::End { start_ms, end_ms } => out.send(
                "speech_end",
                None,
                json!({ "session": session, "start_ms": start_ms, "end_ms": end_ms }),
            ),
        }
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
            "stream_start" | "vad_start" | "stream_audio" | "stream_end" => {
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
