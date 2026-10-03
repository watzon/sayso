//! Engine state and request handling: downloads, loaded models, live preview
//! sessions, and transcription.
//!
//! `main` reads stdin and calls [`Engine::handle`]: on a thread of its own for
//! most requests, and on one serial thread for `stream_start`, `stream_audio`,
//! and `stream_end`, which must run in arrival order. `unload` runs on the
//! stdin thread, so a later `load` of the same model finds the model gone.

use crate::audio;
use crate::download::{Downloader, Throttle};
use crate::preview::{self, Decoder, Session};
use crate::protocol::{ENGINE_VERSION, Output, PROTOCOL_VERSION, Request, Result};
use crate::recognizer::Recognizer;
use crate::store::{Family, ModelStore, remote_files};
use sayso_core::models::EngineKind;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

/// Loads the files of a model folder. `recognizer::load` in the real engine.
pub type Loader = dyn Fn(&Path, &EngineKind) -> Result<Box<dyn Recognizer>> + Send + Sync;

pub struct Config {
    pub models_dir: PathBuf,
    /// Hugging Face, `https://huggingface.co`. Tests point it at a local server.
    pub hf_endpoint: String,
    pub preview: preview::Config,
}

impl Config {
    pub fn new(models_dir: PathBuf) -> Config {
        Config {
            models_dir,
            hf_endpoint: "https://huggingface.co".into(),
            preview: preview::Config::default(),
        }
    }
}

/// A model in memory. One call at a time.
pub struct Slot {
    family: Family,
    recognizer: Mutex<Box<dyn Recognizer>>,
}

#[derive(Default)]
struct State {
    /// The engine of each model id the client told us about.
    registry: HashMap<String, EngineKind>,
    loaded: HashMap<String, Arc<Slot>>,
    /// Models with a download, a load, or a delete running.
    busy: HashSet<String>,
    downloading: HashSet<String>,
}

struct Live {
    model: String,
    session: Session,
}

pub struct Engine {
    out: Output,
    store: ModelStore,
    endpoint: String,
    preview: preview::Config,
    loader: Box<Loader>,
    downloader: Downloader,
    state: Mutex<State>,
    /// One load at a time. A second request for the same model waits and
    /// then finds it loaded.
    load_lock: Mutex<()>,
    sessions: Mutex<HashMap<u64, Live>>,
    /// Final passes running now. The preview pauses while one runs, so the
    /// final pass has the processor and the model.
    finals: Arc<AtomicUsize>,
    /// Sessions whose unknown-session error went out once already.
    unknown_sessions: Mutex<HashSet<u64>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Counts a running final pass while it lives.
struct FinalPass(Arc<AtomicUsize>);

impl FinalPass {
    fn start(counter: &Arc<AtomicUsize>) -> FinalPass {
        counter.fetch_add(1, Ordering::SeqCst);
        FinalPass(Arc::clone(counter))
    }
}

impl Drop for FinalPass {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The preview decoder of a session: the shared model, without vocabulary.
struct SlotDecoder {
    slot: Arc<Slot>,
    language: String,
}

impl Decoder for SlotDecoder {
    fn decode(&mut self, samples: &[f32]) -> Result<String> {
        lock(&self.slot.recognizer).transcribe(samples, &self.language, &[])
    }
}

impl Engine {
    pub fn new(out: Output, config: Config, loader: Box<Loader>) -> Arc<Engine> {
        Arc::new(Engine {
            out,
            store: ModelStore::new(config.models_dir),
            endpoint: config.hf_endpoint,
            preview: config.preview,
            loader,
            downloader: Downloader::new(),
            state: Mutex::new(State::default()),
            load_lock: Mutex::new(()),
            sessions: Mutex::new(HashMap::new()),
            finals: Arc::new(AtomicUsize::new(0)),
            unknown_sessions: Mutex::new(HashSet::new()),
        })
    }

    /// Requests that must run in arrival order on one thread.
    pub fn is_stream(kind: &str) -> bool {
        matches!(kind, "stream_start" | "stream_audio" | "stream_end")
    }

    /// Handle one request and send its reply. Never panics.
    pub fn handle(&self, request: &Request) {
        let id = request.id.as_deref();
        let result = catch_unwind(AssertUnwindSafe(|| self.dispatch(request)))
            .unwrap_or_else(|panic| Err(format!("internal error: {}", panic_message(&panic))));
        if let Err(message) = result {
            if request.kind == "stream_audio" && !self.first_unknown_report(request) {
                return;
            }
            self.out.error(id, &message);
        }
    }

    fn dispatch(&self, r: &Request) -> Result<()> {
        let id = r.id.as_deref();
        match r.kind.as_str() {
            "hello" => {
                self.out.send(
                    "hello",
                    id,
                    json!({"version": ENGINE_VERSION, "protocol": PROTOCOL_VERSION}),
                );
                Ok(())
            }
            "list_models" => self.list_models(r),
            "download" => self.download(r),
            "delete" => self.delete(r),
            "load" => self.load(r),
            "unload" => self.unload(r),
            "transcribe" => self.transcribe(r),
            "stream_start" => self.stream_start(r),
            "stream_audio" => self.stream_audio(r),
            "stream_end" => self.stream_end(r),
            "shutdown" => {
                // `main` exits after this reply.
                self.out.ok(id, json!({}));
                Ok(())
            }
            other => Err(format!("unknown request type {other}")),
        }
    }

    /// An unknown session in `stream_audio` is reported once, not for every frame.
    fn first_unknown_report(&self, request: &Request) -> bool {
        match request.opt_u64("session") {
            Some(session) => lock(&self.unknown_sessions).insert(session),
            None => true,
        }
    }

    // -- Status -------------------------------------------------------------

    fn state_of(&self, model: &str, engine: &EngineKind) -> &'static str {
        let state = lock(&self.state);
        if state.loaded.contains_key(model) {
            "ready"
        } else if state.downloading.contains(model) {
            "downloading"
        } else if self.store.is_downloaded(model, engine) {
            "downloaded"
        } else {
            "not_downloaded"
        }
    }

    /// One `model_state` per model, from disk only. A model this engine cannot
    /// run is `failed` with the reason; the other models still get their state.
    fn list_models(&self, r: &Request) -> Result<()> {
        let id = r.id.as_deref();
        for (model, engine) in r.models()? {
            match engine.and_then(|e| Family::of(&e).map(|_| e)) {
                Ok(engine) => {
                    lock(&self.state)
                        .registry
                        .insert(model.clone(), engine.clone());
                    self.out
                        .model_state(id, &model, self.state_of(&model, &engine));
                }
                Err(message) => {
                    self.out.send(
                        "model_state",
                        id,
                        json!({"model": model, "state": "failed", "message": message}),
                    );
                }
            }
        }
        self.out.ok(id, json!({}));
        Ok(())
    }

    // -- Download -----------------------------------------------------------

    fn acquire(&self, model: &str) -> Result<()> {
        if lock(&self.state).busy.insert(model.to_string()) {
            Ok(())
        } else {
            Err(format!("model {model} is busy"))
        }
    }

    fn release(&self, model: &str) {
        let mut state = lock(&self.state);
        state.busy.remove(model);
        state.downloading.remove(model);
    }

    fn download(&self, r: &Request) -> Result<()> {
        let id = r.id.as_deref();
        let model = r.str("model")?;
        let engine = r.engine()?;
        let files = remote_files(&engine, &self.endpoint)?;
        let folder = self.store.folder(model)?;
        lock(&self.state)
            .registry
            .insert(model.to_string(), engine.clone());
        if self.store.is_downloaded(model, &engine) {
            self.out.model_state(id, model, "downloaded");
            self.out.ok(id, json!({}));
            return Ok(());
        }
        self.acquire(model)?;
        lock(&self.state).downloading.insert(model.to_string());

        let mut throttle = Throttle::protocol();
        let mut last = (0u64, r.opt_u64("size_bytes").unwrap_or(0));
        let report = |throttle: &mut Throttle, done: u64, total: u64, force: bool| {
            if let Some(p) = throttle.check(done, total, Instant::now(), force) {
                let fields = json!({"model": model, "fraction": p.fraction, "bytes_done": p.bytes_done, "bytes_total": p.bytes_total});
                self.out.send("download_progress", id, fields.clone());
                let mut state = fields;
                state["state"] = json!("downloading");
                self.out.send("model_state", id, state);
            }
        };
        report(&mut throttle, 0, last.1, true);
        let started = Instant::now();
        let result = self
            .downloader
            .fetch_all(
                &files,
                &folder,
                r.opt_u64("size_bytes"),
                &mut |done, total| {
                    last = (done, total);
                    report(&mut throttle, done, total, false);
                },
            )
            .and_then(|()| self.store.mark_downloaded(model))
            .and_then(|()| {
                if self.store.is_downloaded(model, &engine) {
                    Ok(())
                } else {
                    Err("download finished but the files are incomplete".to_string())
                }
            });
        self.release(model);
        match result {
            Ok(()) => {
                log::info!(
                    "downloaded {model}: {} bytes in {} s",
                    last.0,
                    started.elapsed().as_secs()
                );
                report(&mut throttle, last.1.max(last.0), last.1.max(last.0), true);
                self.out.model_state(id, model, "downloaded");
                self.out.ok(id, json!({}));
                Ok(())
            }
            Err(message) => {
                self.out.send(
                    "model_state",
                    id,
                    json!({"model": model, "state": "failed", "message": message}),
                );
                Err(message)
            }
        }
    }

    // -- Delete -------------------------------------------------------------

    fn delete(&self, r: &Request) -> Result<()> {
        let id = r.id.as_deref();
        let model = r.str("model")?;
        self.store.folder(model)?;
        if let Some(Ok(engine)) = r.opt_engine() {
            lock(&self.state).registry.insert(model.to_string(), engine);
        }
        self.acquire(model)?;
        self.stop_sessions_of(model);
        lock(&self.state).loaded.remove(model);
        let result = self.store.delete(model);
        self.release(model);
        result?;
        self.out.model_state(id, model, "not_downloaded");
        self.out.ok(id, json!({}));
        Ok(())
    }

    fn stop_sessions_of(&self, model: &str) {
        let stopped: Vec<Live> = {
            let mut sessions = lock(&self.sessions);
            let ids: Vec<u64> = sessions
                .iter()
                .filter(|(_, l)| l.model == model)
                .map(|(id, _)| *id)
                .collect();
            ids.into_iter()
                .filter_map(|id| sessions.remove(&id))
                .collect()
        };
        for live in stopped {
            live.session.finish(true);
        }
    }

    // -- Load ---------------------------------------------------------------

    fn load(&self, r: &Request) -> Result<()> {
        let id = r.id.as_deref();
        let model = r.str("model")?;
        let engine = r.engine()?;
        Family::of(&engine)?;
        self.store.folder(model)?;
        lock(&self.state)
            .registry
            .insert(model.to_string(), engine.clone());
        if lock(&self.state).loaded.contains_key(model) {
            self.out.send(
                "model_state",
                id,
                json!({"model": model, "state": "ready", "load_ms": 0}),
            );
        } else {
            self.ensure_loaded(id, model)?;
        }
        self.out.ok(id, json!({}));
        Ok(())
    }

    /// The loaded model, loading it first when it is on disk.
    fn ensure_loaded(&self, id: Option<&str>, model: &str) -> Result<Arc<Slot>> {
        if let Some(slot) = lock(&self.state).loaded.get(model) {
            return Ok(Arc::clone(slot));
        }
        let _one_load = lock(&self.load_lock);
        let engine = {
            let state = lock(&self.state);
            if let Some(slot) = state.loaded.get(model) {
                return Ok(Arc::clone(slot));
            }
            state.registry.get(model).cloned().ok_or_else(|| {
                format!("unknown model {model}: send list_models, download, or load first")
            })?
        };
        let family = Family::of(&engine)?;
        if !self.store.is_downloaded(model, &engine) {
            return Err(format!("model {model} is not downloaded"));
        }
        self.acquire(model)?;
        self.out.model_state(id, model, "optimizing");
        let started = Instant::now();
        let folder = self.store.folder(model)?;
        let loaded = catch_unwind(AssertUnwindSafe(|| (self.loader)(&folder, &engine)))
            .unwrap_or_else(|panic| {
                Err(format!(
                    "loading {model} crashed: {}",
                    panic_message(&panic)
                ))
            });
        self.release(model);
        match loaded {
            Ok(recognizer) => {
                let load_ms = started.elapsed().as_millis() as u64;
                log::info!("loaded {model} in {load_ms} ms");
                let slot = Arc::new(Slot {
                    family,
                    recognizer: Mutex::new(recognizer),
                });
                lock(&self.state)
                    .loaded
                    .insert(model.to_string(), Arc::clone(&slot));
                self.out.send(
                    "model_state",
                    id,
                    json!({"model": model, "state": "ready", "load_ms": load_ms}),
                );
                Ok(slot)
            }
            Err(message) => {
                self.out.send(
                    "model_state",
                    id,
                    json!({"model": model, "state": "failed", "message": message}),
                );
                Err(message)
            }
        }
    }

    /// Take a model out of memory. Its files stay on disk.
    fn unload(&self, r: &Request) -> Result<()> {
        let id = r.id.as_deref();
        let model = r.str("model")?;
        self.acquire(model)?;
        self.stop_sessions_of(model);
        let removed = lock(&self.state).loaded.remove(model);
        self.release(model);
        if removed.is_some() {
            log::info!("unloaded {model}");
            self.out.model_state(id, model, "downloaded");
        }
        self.out.ok(id, json!({}));
        Ok(())
    }

    // -- Transcribe ---------------------------------------------------------

    fn transcribe(&self, r: &Request) -> Result<()> {
        let id = r.id.as_deref();
        let model = r.str("model")?;
        let path = r.str("path")?;
        let language = r.opt_str("language").unwrap_or("auto");
        let vocabulary = r.strings("vocabulary");
        // Count the final pass first, so a running preview stops starting decodes.
        let _final = FinalPass::start(&self.finals);
        let samples = audio::read_wav(Path::new(path))?;
        if samples.is_empty() {
            return Err("audio file has no samples".into());
        }
        let slot = self.ensure_loaded(id, model)?;
        let started = Instant::now();
        let text = {
            let mut recognizer = lock(&slot.recognizer);
            catch_unwind(AssertUnwindSafe(|| {
                recognizer.transcribe(&samples, language, &vocabulary)
            }))
            .unwrap_or_else(|panic| {
                Err(format!("transcription crashed: {}", panic_message(&panic)))
            })?
        };
        let elapsed_ms = started.elapsed().as_millis() as u64;
        log::info!(
            "transcribed {} ms of audio with {model} in {elapsed_ms} ms",
            samples.len() / audio::ms(1)
        );
        self.out.send(
            "final",
            id,
            json!({"text": text.trim(), "elapsed_ms": elapsed_ms}),
        );
        Ok(())
    }

    // -- Live preview -------------------------------------------------------

    fn stream_start(&self, r: &Request) -> Result<()> {
        let id = r.id.as_deref();
        let session = r.u64("session")?;
        let model = r.str("model")?;
        let language = r.opt_str("language").unwrap_or("auto").to_string();
        let slot = self.ensure_loaded(id, model)?;
        if !slot.family.has_preview() {
            return Err(format!("model {model} has no live preview"));
        }
        // One dictation at a time: a new session replaces older ones.
        let older: Vec<Live> = lock(&self.sessions).drain().map(|(_, live)| live).collect();
        for live in older {
            live.session.finish(true);
        }
        let out = self.out.clone();
        let finals = Arc::clone(&self.finals);
        let decoder = Box::new(SlotDecoder { slot, language });
        let live = Session::start(
            self.preview.clone(),
            decoder,
            move || finals.load(Ordering::SeqCst) > 0,
            move |committed, tentative| {
                out.send(
                    "partial",
                    None,
                    json!({"session": session, "committed": committed, "tentative": tentative}),
                );
            },
        );
        lock(&self.sessions).insert(
            session,
            Live {
                model: model.to_string(),
                session: live,
            },
        );
        lock(&self.unknown_sessions).remove(&session);
        self.out.ok(id, json!({}));
        Ok(())
    }

    fn stream_audio(&self, r: &Request) -> Result<()> {
        let session = r.u64("session")?;
        let samples = audio::pcm_from_base64(r.str("pcm")?)?;
        match lock(&self.sessions).get(&session) {
            Some(live) => {
                live.session.push(&samples);
                Ok(())
            }
            None => Err(format!("unknown stream session {session}")),
        }
    }

    fn stream_end(&self, r: &Request) -> Result<()> {
        let id = r.id.as_deref();
        let session = r.u64("session")?;
        let live = lock(&self.sessions)
            .remove(&session)
            .ok_or_else(|| format!("unknown stream session {session}"))?;
        // A final pass that waits for the model goes first; the preview then
        // ends with the text it has.
        let busy = self.finals.load(Ordering::SeqCst) > 0;
        let (partial, text) = live.session.finish(busy);
        if let Some(text) = partial {
            self.out.send(
                "partial",
                None,
                json!({"session": session, "committed": text, "tentative": ""}),
            );
        }
        self.out.ok(id, json!({"session": session, "text": text}));
        Ok(())
    }
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Captured;
    use serde_json::Value;
    use std::time::Duration;

    /// Says how many samples it got. `slow` sleeps 300 ms per call.
    struct Fake {
        slow: bool,
    }

    impl Recognizer for Fake {
        fn transcribe(
            &mut self,
            samples: &[f32],
            language: &str,
            vocabulary: &[String],
        ) -> Result<String> {
            if self.slow {
                std::thread::sleep(Duration::from_millis(300));
            }
            if samples.iter().any(|s| s.is_nan()) {
                panic!("NaN audio");
            }
            Ok(format!(
                "{} samples {language} {}",
                samples.len(),
                vocabulary.join("+")
            ))
        }
    }

    fn fake_loader(slow: bool) -> Box<Loader> {
        Box::new(move |folder: &Path, engine: &EngineKind| {
            Family::of(engine)?;
            if folder.join("broken").exists() {
                return Err("the model files are broken".into());
            }
            Ok(Box::new(Fake { slow }) as Box<dyn Recognizer>)
        })
    }

    struct Rig {
        engine: Arc<Engine>,
        captured: Captured,
        dir: tempfile::TempDir,
    }

    fn rig_with(slow: bool, endpoint: &str) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let captured = Captured::default();
        let mut config = Config::new(dir.path().to_path_buf());
        config.hf_endpoint = endpoint.into();
        config.preview.interval = Duration::from_millis(20);
        let engine = Engine::new(captured.output(), config, fake_loader(slow));
        Rig {
            engine,
            captured,
            dir,
        }
    }

    fn rig() -> Rig {
        rig_with(false, "http://127.0.0.1:9")
    }

    impl Rig {
        fn send(&self, line: Value) -> Vec<Value> {
            let before = self.captured.messages().len();
            self.engine
                .handle(&Request::parse(&line.to_string()).unwrap());
            self.captured.messages()[before..].to_vec()
        }

        /// Put the files of a whisper model on disk, as a finished download.
        fn install(&self, model: &str) {
            let folder = self.dir.path().join(model);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("ggml-tiny.bin"), b"x").unwrap();
            std::fs::write(folder.join(crate::store::MARKER), b"").unwrap();
        }

        fn install_parakeet(&self, model: &str) {
            let folder = self.dir.path().join(model);
            std::fs::create_dir_all(&folder).unwrap();
            for file in crate::store::PARAKEET_FILES {
                std::fs::write(folder.join(file), b"x").unwrap();
            }
            std::fs::write(folder.join(crate::store::MARKER), b"").unwrap();
        }

        fn wav(&self, samples: usize) -> String {
            let path = self.dir.path().join(format!("clip-{samples}.wav"));
            std::fs::write(&path, sayso_core::stt::encode_wav(&vec![1000i16; samples])).unwrap();
            path.to_string_lossy().into_owned()
        }
    }

    fn whisper() -> Value {
        json!({"kind": "whisper_cpp", "file": "ggml-tiny.bin"})
    }

    fn parakeet() -> Value {
        json!({"kind": "onnx", "family": "parakeet", "repo": "istupakov/parakeet-tdt-0.6b-v3-onnx"})
    }

    fn types(messages: &[Value]) -> Vec<String> {
        messages
            .iter()
            .map(|m| m["type"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn hello_and_unknown_requests() {
        let rig = rig();
        let reply = rig.send(json!({"v": 1, "type": "hello", "id": "r1"}));
        assert_eq!(reply[0]["type"], "hello");
        assert_eq!(reply[0]["protocol"], 1);
        assert_eq!(reply[0]["id"], "r1");
        let reply = rig.send(json!({"v": 1, "type": "fly", "id": "r2"}));
        assert_eq!(reply[0]["type"], "error");
        assert_eq!(reply[0]["message"], "unknown request type fly");
        assert_eq!(reply[0]["id"], "r2");
    }

    #[test]
    fn list_models_checks_the_disk_and_flags_foreign_kinds() {
        let rig = rig();
        rig.install("whisper-tiny");
        let reply = rig.send(json!({"type": "list_models", "id": "r1", "models": [
            {"id": "whisper-tiny", "engine": whisper()},
            {"id": "parakeet-tdt-v3", "engine": parakeet()},
            {"id": "parakeet-unified-en", "engine": {"kind": "parakeet_unified", "streaming_tier": "70_2_2"}},
            {"id": "weird", "engine": {"kind": "teleport"}},
        ]}));
        assert_eq!(
            types(&reply),
            [
                "model_state",
                "model_state",
                "model_state",
                "model_state",
                "ok"
            ]
        );
        assert_eq!(reply[0]["state"], "downloaded");
        assert_eq!(reply[1]["state"], "not_downloaded");
        assert_eq!(reply[2]["state"], "failed");
        assert!(
            reply[2]["message"]
                .as_str()
                .unwrap()
                .contains("parakeet_unified")
        );
        assert_eq!(reply[3]["message"], "unknown engine kind teleport");
        assert!(reply.iter().all(|m| m["id"] == "r1"));

        let reply = rig.send(json!({"type": "list_models", "id": "r2"}));
        assert_eq!(reply[0]["type"], "error");
    }

    #[test]
    fn load_then_transcribe_and_ready_in_the_list() {
        let rig = rig();
        rig.install("whisper-tiny");
        let reply = rig.send(
            json!({"type": "load", "id": "r1", "model": "whisper-tiny", "engine": whisper()}),
        );
        assert_eq!(types(&reply), ["model_state", "model_state", "ok"]);
        assert_eq!(reply[0]["state"], "optimizing");
        assert_eq!(reply[1]["state"], "ready");
        assert!(reply[1]["load_ms"].is_u64());
        let again = rig.send(
            json!({"type": "load", "id": "r2", "model": "whisper-tiny", "engine": whisper()}),
        );
        assert_eq!(types(&again), ["model_state", "ok"]);
        assert_eq!(again[0]["load_ms"], 0);

        let path = rig.wav(16_000);
        let reply = rig.send(json!({"type": "transcribe", "id": "r3", "model": "whisper-tiny", "path": path, "language": "de", "vocabulary": ["Sayso", "GitHub"]}));
        assert_eq!(types(&reply), ["final"]);
        assert_eq!(reply[0]["text"], "16000 samples de Sayso+GitHub");
        assert!(reply[0]["elapsed_ms"].is_u64());

        let list = rig.send(json!({"type": "list_models", "id": "r4", "models": [{"id": "whisper-tiny", "engine": whisper()}]}));
        assert_eq!(list[0]["state"], "ready");
    }

    #[test]
    fn transcribe_loads_on_demand_and_reports_bad_input() {
        let rig = rig();
        rig.install("whisper-tiny");
        let path = rig.wav(8000);
        // Not in the registry yet.
        let reply = rig.send(json!({"type": "transcribe", "id": "r1", "model": "whisper-tiny", "path": path, "language": "auto"}));
        assert_eq!(reply[0]["type"], "error");
        assert!(
            reply[0]["message"]
                .as_str()
                .unwrap()
                .contains("unknown model")
        );

        rig.send(json!({"type": "list_models", "id": "r2", "models": [{"id": "whisper-tiny", "engine": whisper()}]}));
        let reply = rig
            .send(json!({"type": "transcribe", "id": "r3", "model": "whisper-tiny", "path": path}));
        assert_eq!(
            types(&reply),
            ["model_state", "model_state", "final"],
            "{reply:?}"
        );
        assert_eq!(reply[2]["text"], "8000 samples auto");

        let reply = rig.send(
            json!({"type": "transcribe", "id": "r4", "model": "whisper-tiny", "path": "nope.wav"}),
        );
        assert_eq!(reply[0]["type"], "error");
        let empty = rig.wav(0);
        let reply = rig.send(
            json!({"type": "transcribe", "id": "r5", "model": "whisper-tiny", "path": empty}),
        );
        assert_eq!(reply[0]["message"], "audio file has no samples");
        let reply = rig.send(json!({"type": "transcribe", "id": "r6", "model": "whisper-tiny"}));
        assert_eq!(reply[0]["message"], "missing string field path");
    }

    #[test]
    fn load_errors() {
        let rig = rig();
        let reply = rig.send(
            json!({"type": "load", "id": "r1", "model": "whisper-tiny", "engine": whisper()}),
        );
        assert_eq!(reply[0]["message"], "model whisper-tiny is not downloaded");
        let reply = rig.send(
            json!({"type": "load", "id": "r2", "model": "x", "engine": {"kind": "apple_speech"}}),
        );
        assert!(
            reply[0]["message"]
                .as_str()
                .unwrap()
                .contains("does not run")
        );
        let reply =
            rig.send(json!({"type": "load", "id": "r3", "model": "../evil", "engine": whisper()}));
        assert!(
            reply[0]["message"]
                .as_str()
                .unwrap()
                .contains("bad model id")
        );
        rig.install("whisper-tiny");
        std::fs::write(rig.dir.path().join("whisper-tiny/broken"), b"").unwrap();
        let reply = rig.send(
            json!({"type": "load", "id": "r4", "model": "whisper-tiny", "engine": whisper()}),
        );
        assert_eq!(types(&reply), ["model_state", "model_state", "error"]);
        assert_eq!(reply[1]["state"], "failed");
    }

    #[test]
    fn a_panic_becomes_an_error_and_the_model_stays_usable() {
        let rig = rig();
        rig.install("whisper-tiny");
        rig.send(json!({"type": "load", "id": "r1", "model": "whisper-tiny", "engine": whisper()}));
        let path = rig.dir.path().join("nan.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        writer.write_sample(f32::NAN).unwrap();
        writer.finalize().unwrap();
        let reply = rig
            .send(json!({"type": "transcribe", "id": "r2", "model": "whisper-tiny", "path": path}));
        assert_eq!(reply[0]["type"], "error");
        assert!(
            reply[0]["message"].as_str().unwrap().contains("crashed"),
            "{reply:?}"
        );
        let ok = rig.send(json!({"type": "transcribe", "id": "r3", "model": "whisper-tiny", "path": rig.wav(100)}));
        assert_eq!(ok[0]["type"], "final");
    }

    #[test]
    fn delete_unloads_and_removes_the_folder() {
        let rig = rig();
        rig.install("whisper-tiny");
        rig.send(json!({"type": "load", "id": "r1", "model": "whisper-tiny", "engine": whisper()}));
        let reply = rig.send(
            json!({"type": "delete", "id": "r2", "model": "whisper-tiny", "engine": whisper()}),
        );
        assert_eq!(types(&reply), ["model_state", "ok"]);
        assert_eq!(reply[0]["state"], "not_downloaded");
        assert!(!rig.dir.path().join("whisper-tiny").exists());
        let list = rig.send(json!({"type": "list_models", "id": "r3", "models": [{"id": "whisper-tiny", "engine": whisper()}]}));
        assert_eq!(list[0]["state"], "not_downloaded");
        // Deleting what is not there is fine.
        let again = rig.send(json!({"type": "delete", "id": "r4", "model": "whisper-tiny"}));
        assert_eq!(types(&again), ["model_state", "ok"]);
    }

    #[test]
    fn unload_takes_a_model_out_of_memory_and_keeps_the_files() {
        let rig = rig();
        rig.install("whisper-tiny");
        rig.install_parakeet("parakeet-tdt-v3");
        let list = json!({"type": "list_models", "id": "l", "models": [
            {"id": "whisper-tiny", "engine": whisper()},
            {"id": "parakeet-tdt-v3", "engine": parakeet()},
        ]});
        rig.send(json!({"type": "load", "id": "r1", "model": "whisper-tiny", "engine": whisper()}));
        rig.send(
            json!({"type": "load", "id": "r2", "model": "parakeet-tdt-v3", "engine": parakeet()}),
        );
        // A live preview session of the model ends with the unload.
        rig.send(
            json!({"type": "stream_start", "id": "r3", "session": 5, "model": "parakeet-tdt-v3"}),
        );

        let reply = rig.send(json!({"type": "unload", "id": "r4", "model": "parakeet-tdt-v3"}));
        assert_eq!(types(&reply), ["model_state", "ok"]);
        assert_eq!(reply[0]["state"], "downloaded");
        assert_eq!(reply[0]["id"], "r4");
        let states = rig.send(list);
        assert_eq!(states[0]["state"], "ready", "the other model stays");
        assert_eq!(states[1]["state"], "downloaded");
        assert!(rig.dir.path().join("parakeet-tdt-v3").exists());
        let end = rig.send(json!({"type": "stream_end", "id": "r5", "session": 5}));
        assert_eq!(end[0]["message"], "unknown stream session 5");

        // Not in memory: `ok`, and no state change.
        let again = rig.send(json!({"type": "unload", "id": "r6", "model": "parakeet-tdt-v3"}));
        assert_eq!(types(&again), ["ok"]);
        // The next load reads the files again.
        let load = rig.send(
            json!({"type": "load", "id": "r7", "model": "parakeet-tdt-v3", "engine": parakeet()}),
        );
        assert_eq!(types(&load), ["model_state", "model_state", "ok"]);
        assert_eq!(load[0]["state"], "optimizing");
        let bad = rig.send(json!({"type": "unload", "id": "r8"}));
        assert_eq!(bad[0]["message"], "missing string field model");
    }

    #[test]
    fn download_writes_files_then_the_marker() {
        let body = vec![5u8; 2000];
        let files: Vec<(&'static str, Vec<u8>)> = vec![("ggml-tiny.bin", body.clone())];
        let server = crate::download::tests::serve(files);
        let rig = rig_with(false, &server.url);
        let reply = rig.send(json!({"type": "download", "id": "r1", "model": "whisper-tiny", "engine": whisper(), "size_bytes": 1000}));
        let kinds = types(&reply);
        assert_eq!(kinds.first().map(String::as_str), Some("download_progress"));
        assert_eq!(kinds.last().map(String::as_str), Some("ok"));
        let done: Vec<&Value> = reply
            .iter()
            .filter(|m| m["type"] == "download_progress")
            .collect();
        assert_eq!(done.last().unwrap()["fraction"], 1.0);
        assert_eq!(
            done.last().unwrap()["bytes_total"],
            2000,
            "Content-Length wins over size_bytes"
        );
        assert!(
            reply
                .iter()
                .any(|m| m["type"] == "model_state" && m["state"] == "downloaded")
        );
        assert_eq!(
            std::fs::read(rig.dir.path().join("whisper-tiny/ggml-tiny.bin")).unwrap(),
            body
        );
        assert!(
            rig.dir
                .path()
                .join("whisper-tiny")
                .join(crate::store::MARKER)
                .exists()
        );

        // Downloaded already: no request.
        server.requests.lock().unwrap().clear();
        let again = rig.send(
            json!({"type": "download", "id": "r2", "model": "whisper-tiny", "engine": whisper()}),
        );
        assert_eq!(types(&again), ["model_state", "ok"]);
        assert!(server.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failed_download_leaves_no_marker() {
        let server =
            crate::download::tests::serve(vec![("encoder-model.int8.onnx", vec![1u8; 100])]);
        let rig = rig_with(false, &server.url);
        let reply = rig.send(json!({"type": "download", "id": "r1", "model": "parakeet-tdt-v3", "engine": parakeet()}));
        assert_eq!(reply.last().unwrap()["type"], "error");
        assert!(
            reply
                .iter()
                .any(|m| m["type"] == "model_state" && m["state"] == "failed")
        );
        let folder = rig.dir.path().join("parakeet-tdt-v3");
        assert!(
            folder.join("encoder-model.int8.onnx").exists(),
            "the good file stays for next time"
        );
        assert!(!folder.join(crate::store::MARKER).exists());
        let list = rig.send(json!({"type": "list_models", "id": "r2", "models": [{"id": "parakeet-tdt-v3", "engine": parakeet()}]}));
        assert_eq!(list[0]["state"], "not_downloaded");
        let bad = rig.send(
            json!({"type": "download", "id": "r3", "model": "m", "engine": {"kind": "cohere"}}),
        );
        assert!(bad[0]["message"].as_str().unwrap().contains("cohere"));
    }

    fn pcm(samples: usize) -> String {
        use base64::Engine as _;
        let bytes: Vec<u8> = (0..samples).flat_map(|_| 3000i16.to_le_bytes()).collect();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn stream_sends_partials_and_ends_with_the_text() {
        let rig = rig();
        rig.install_parakeet("parakeet-tdt-v3");
        rig.send(json!({"type": "list_models", "id": "r0", "models": [{"id": "parakeet-tdt-v3", "engine": parakeet()}]}));
        let start = rig.send(json!({"type": "stream_start", "id": "r1", "session": 7, "model": "parakeet-tdt-v3", "language": "en"}));
        assert_eq!(start.last().unwrap()["type"], "ok");
        for _ in 0..10 {
            rig.send(json!({"type": "stream_audio", "session": 7, "pcm": pcm(2560)}));
            std::thread::sleep(Duration::from_millis(30));
        }
        let end = rig.send(json!({"type": "stream_end", "id": "r2", "session": 7}));
        let ok = end.last().unwrap();
        assert_eq!(ok["type"], "ok");
        assert_eq!(ok["text"], "25600 samples en");
        let all = rig.captured.messages();
        let partials: Vec<&Value> = all.iter().filter(|m| m["type"] == "partial").collect();
        assert!(partials.len() >= 2, "{partials:?}");
        assert!(
            partials
                .iter()
                .all(|p| p["session"] == 7 && p.get("id").is_none())
        );
        let last = partials.last().unwrap();
        assert_eq!(
            format!(
                "{} {}",
                last["committed"].as_str().unwrap(),
                last["tentative"].as_str().unwrap()
            )
            .trim(),
            "25600 samples en"
        );

        let again = rig.send(json!({"type": "stream_end", "id": "r3", "session": 7}));
        assert_eq!(again[0]["message"], "unknown stream session 7");
    }

    #[test]
    fn stream_errors() {
        let rig = rig();
        rig.install("whisper-tiny");
        rig.send(json!({"type": "list_models", "id": "r0", "models": [{"id": "whisper-tiny", "engine": whisper()}]}));
        let reply = rig.send(
            json!({"type": "stream_start", "id": "r1", "session": 1, "model": "whisper-tiny"}),
        );
        assert_eq!(
            reply.last().unwrap()["message"],
            "model whisper-tiny has no live preview"
        );
        let reply =
            rig.send(json!({"type": "stream_start", "id": "r2", "session": 1, "model": "nobody"}));
        assert!(
            reply[0]["message"]
                .as_str()
                .unwrap()
                .contains("unknown model")
        );
        // Audio for an unknown session: one error without an id, then silence.
        let first = rig.send(json!({"type": "stream_audio", "session": 99, "pcm": pcm(10)}));
        assert_eq!(first.len(), 1);
        assert!(first[0].get("id").is_none());
        let second = rig.send(json!({"type": "stream_audio", "session": 99, "pcm": pcm(10)}));
        assert!(second.is_empty());
    }

    #[test]
    fn a_final_pass_is_not_held_up_by_the_preview() {
        let rig = rig_with(true, "http://127.0.0.1:9");
        rig.install_parakeet("parakeet-tdt-v3");
        rig.send(json!({"type": "list_models", "id": "r0", "models": [{"id": "parakeet-tdt-v3", "engine": parakeet()}]}));
        rig.send(
            json!({"type": "stream_start", "id": "r1", "session": 3, "model": "parakeet-tdt-v3"}),
        );
        for _ in 0..20 {
            rig.send(json!({"type": "stream_audio", "session": 3, "pcm": pcm(1600)}));
        }
        std::thread::sleep(Duration::from_millis(100)); // A slow preview decode runs now.
        let engine = Arc::clone(&rig.engine);
        let path = rig.wav(16_000);
        let started = Instant::now();
        let final_pass = std::thread::spawn(move || {
            engine.handle(&Request::parse(&json!({"type": "transcribe", "id": "f", "model": "parakeet-tdt-v3", "path": path}).to_string()).unwrap());
            started.elapsed()
        });
        std::thread::sleep(Duration::from_millis(20));
        let end_started = Instant::now();
        let end = rig.send(json!({"type": "stream_end", "id": "r2", "session": 3}));
        assert_eq!(end.last().unwrap()["type"], "ok");
        let end_took = end_started.elapsed();
        let final_took = final_pass.join().unwrap();
        // At most one preview decode (300 ms) runs before the final pass (300 ms).
        assert!(
            final_took < Duration::from_millis(900),
            "final pass took {final_took:?}"
        );
        assert!(
            end_took < Duration::from_millis(700),
            "stream_end took {end_took:?}"
        );
        assert!(
            rig.captured
                .messages()
                .iter()
                .any(|m| m["type"] == "final" && m["id"] == "f")
        );
    }
}
