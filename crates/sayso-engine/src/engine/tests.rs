//! Engine tests with a fake recognizer and a local HTTP server for the archives.

use super::*;
use crate::diarization::RawTurn;
use crate::partials::Segment;
use crate::protocol::tests::Captured;
use crate::recognizer::FinalPass;
use crate::vad::Detector;
use crate::vad::fake::{EnergyDetector, audio_of};
use base64::Engine as _;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// Fake recognizer
// ---------------------------------------------------------------------------

/// Loads the fake models. It checks that the folders exist.
struct FakeLoader {
    /// Loads of a whole model.
    loads: AtomicUsize,
    /// Loads of a final pass only.
    final_loads: AtomicUsize,
    /// The `speakers` of each `diarize` that reached a fake diarization model.
    diarized: Arc<Mutex<Vec<Option<u32>>>>,
}

impl Loader for FakeLoader {
    fn load_final_pass(
        &self,
        spec: &ModelSpec,
        _dirs: &ModelDirs,
    ) -> Result<Option<Arc<dyn FinalPass>>> {
        self.final_loads.fetch_add(1, Ordering::SeqCst);
        Ok(spec
            .recipe
            .final_pass()
            .then(|| Arc::new(FakeFinal) as Arc<dyn FinalPass>))
    }

    fn load(&self, spec: &ModelSpec, dirs: &ModelDirs) -> Result<Loaded> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        if !dirs.main.is_dir() {
            bail!("missing {}", dirs.main.display());
        }
        match spec.recipe.task() {
            Task::Vad => {
                return Ok(Loaded { vad: Some(Arc::new(FakeVad)), ..Loaded::default() });
            }
            Task::Diarization => {
                let diarizer = FakeDiarizer { calls: Arc::clone(&self.diarized) };
                return Ok(Loaded { diarizer: Some(Arc::new(diarizer)), ..Loaded::default() });
            }
            Task::Speech => {}
        }
        let final_pass: Option<Arc<dyn FinalPass>> = spec
            .recipe
            .final_pass()
            .then(|| Arc::new(FakeFinal) as Arc<dyn FinalPass>);
        let preview: Option<Arc<dyn LivePreview>> = dirs
            .stream
            .as_ref()
            .map(|_| Arc::new(FakePreview) as Arc<dyn LivePreview>);
        Ok(Loaded {
            final_pass,
            preview,
            ..Loaded::default()
        })
    }
}

/// Speech is a window with a root mean square above 0.01 (`vad::fake`).
struct FakeVad;

impl VadModel for FakeVad {
    fn detector(&self) -> Result<Box<dyn Detector>> {
        Ok(Box::new(EnergyDetector::new()))
    }
}

/// Two speakers with the numbers 9 and 4 in its own order. The turns of 9 overlap and are not
/// in order. With one speaker, one turn for the whole audio.
struct FakeDiarizer {
    calls: Arc<Mutex<Vec<Option<u32>>>>,
}

impl Diarizer for FakeDiarizer {
    fn diarize(&self, samples: &[f32], speakers: Option<u32>) -> Result<Vec<RawTurn>> {
        self.calls.lock().unwrap().push(speakers);
        if speakers == Some(1) {
            return Ok(vec![RawTurn { start: 0.0, end: samples.len() as f32 / 16_000.0, speaker: 7 }]);
        }
        let turn = |start, end, speaker| RawTurn { start, end, speaker };
        Ok(vec![turn(2.0, 3.0, 4), turn(0.0, 1.0, 9), turn(0.5, 1.5, 9), turn(2.5, 3.5, 9)])
    }
}

/// Says how many samples it heard and in which language.
struct FakeFinal;

impl FinalPass for FakeFinal {
    fn transcribe(&self, samples: &[f32], language: &str, vocabulary: &[String]) -> Result<String> {
        if vocabulary.is_empty() {
            Ok(format!("{} samples in {language}", samples.len()))
        } else {
            Ok(format!("{} samples in {language} with {}", samples.len(), vocabulary.join("+")))
        }
    }

    /// It finds German in audio above zero, English in audio below zero, and
    /// Cantonese in loud audio (a mean above 0.05). Silence has no text.
    fn transcribe_detailed(&self, samples: &[f32], language: &str, vocabulary: &[String], _times: bool) -> Result<crate::timing::Timed> {
        let sum: f32 = samples.iter().sum();
        let found = if sum > 0.05 * samples.len() as f32 {
            Some("yue")
        } else if sum > 0.0 {
            Some("de")
        } else if sum < 0.0 {
            Some("en")
        } else {
            None
        };
        let text = if samples.iter().all(|s| *s == 0.0) { String::new() } else { self.transcribe(samples, language, vocabulary)? };
        Ok(crate::timing::Timed { text, timing: crate::timing::Timing::None, language: found.map(str::to_string) })
    }
}

/// One word per loud chunk. A silent chunk ends the segment.
struct FakePreview;

impl LivePreview for FakePreview {
    fn start(&self) -> Result<Box<dyn PreviewStream>> {
        Ok(Box::new(FakeStream::default()))
    }
}

#[derive(Default)]
struct FakeStream {
    words: Vec<String>,
    silent: bool,
}

impl PreviewStream for FakeStream {
    fn accept(&mut self, samples: &[f32]) {
        self.silent = samples.iter().all(|s| *s == 0.0);
        if !self.silent {
            self.words.push(format!("w{}", self.words.len()));
        }
    }

    fn decode(&mut self) -> Segment {
        let segment = Segment {
            text: self.words.join(" "),
            endpoint: self.silent && !self.words.is_empty(),
        };
        if segment.endpoint {
            self.words.clear();
        }
        segment
    }

    fn finish(&mut self) -> Segment {
        let text = self.words.join(" ");
        self.words.clear();
        Segment {
            text,
            endpoint: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Archive server
// ---------------------------------------------------------------------------

/// A `.tar.bz2` with one top folder named after the archive.
fn archive(name: &str, files: &[&str]) -> Vec<u8> {
    let encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    for file in files {
        let body = format!("contents of {file}");
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("./{name}/{file}"), body.as_bytes())
            .unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

/// Serves `/files/<name>.tar.bz2`, and a single file such as `<repo>/resolve/<revision>/<file>` at
/// `/files/<path>`. `/redirect/<name>` redirects there, as GitHub and Hugging Face do.
struct Server {
    base: String,
    requests: Arc<AtomicUsize>,
}

fn serve_archives(archives: Vec<(String, Vec<u8>)>) -> Server {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&requests);
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            counter.fetch_add(1, Ordering::SeqCst);
            let url = request.url().to_string();
            if let Some(rest) = url.strip_prefix("/redirect/") {
                let location =
                    tiny_http::Header::from_bytes("Location", format!("/files/{rest}")).unwrap();
                let _ = request.respond(tiny_http::Response::empty(302).with_header(location));
                continue;
            }
            // The releases of the diarization models are beside the redirect folder: `/<release>/<file>`.
            let found = archives
                .iter()
                .find(|(name, _)| {
                    [format!("/files/{name}.tar.bz2"), format!("/files/{name}"), format!("/{name}.tar.bz2"), format!("/{name}")]
                        .contains(&url)
                })
                .map(|(_, bytes)| bytes.clone());
            let _ = match found {
                Some(bytes) => request.respond(tiny_http::Response::from_data(bytes)),
                None => request.respond(tiny_http::Response::empty(404)),
            };
        }
    });
    Server {
        base: format!("http://127.0.0.1:{port}/redirect"),
        requests,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct Harness {
    engine: Arc<Engine>,
    captured: Captured,
    loader: Arc<FakeLoader>,
    _dir: tempfile::TempDir,
}

fn harness(base_url: &str) -> Harness {
    harness_with(base_url, None, descriptors::all())
}

fn harness_with(base_url: &str, store: Option<Store>, descriptors: &'static Descriptors) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let captured = Captured::default();
    let out = Arc::new(Output::new(Box::new(captured.clone())));
    let loader = Arc::new(FakeLoader {
        loads: AtomicUsize::new(0),
        final_loads: AtomicUsize::new(0),
        diarized: Arc::new(Mutex::new(Vec::new())),
    });
    let config = Config {
        models_dir: dir.path().join("models"),
        base_url: base_url.to_string(),
        hf_endpoint: base_url.to_string(),
        decode_interval: Duration::ZERO,
        store,
        descriptors,
    };
    let engine = Arc::new(Engine::new(
        config,
        out,
        Arc::clone(&loader) as Arc<dyn Loader>,
    ));
    Harness {
        engine,
        captured,
        loader,
        _dir: dir,
    }
}

impl Harness {
    /// Run one request and return the messages it wrote.
    fn call(&self, request: Value) -> Vec<Value> {
        self.captured.clear();
        self.engine
            .handle(&Request::parse(&request.to_string()).unwrap());
        self.captured.messages()
    }

    fn stream(&self, worker: &mut StreamWorker, request: Value) -> Vec<Value> {
        self.captured.clear();
        worker.handle(&Request::parse(&request.to_string()).unwrap());
        self.captured.messages()
    }

    fn models_dir(&self) -> &std::path::Path {
        &self.engine.config.models_dir
    }
}

fn spec_json(recipe: &str, archive: &str, preview: Option<&str>) -> Value {
    json!({ "kind": "sherpa", "recipe": recipe, "archive": archive, "preview_archive": preview })
}

/// The `state` of each `model_state` message, in order.
fn states(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter(|m| m["type"] == "model_state")
        .map(|m| m["state"].as_str().unwrap().to_string())
        .collect()
}

fn last(messages: &[Value]) -> &Value {
    messages.last().expect("a reply")
}

fn pcm(samples: &[i16]) -> String {
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn write_wav(path: &std::path::Path, samples: &[i16]) {
    let pcm = sayso_core::stt::encode_wav(samples);
    std::fs::File::create(path)
        .unwrap()
        .write_all(&pcm)
        .unwrap();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn list_models_without_a_list_gives_the_catalog() {
    let h = harness("http://unused");
    let reply = h.call(json!({"v": 1, "type": "list_models", "id": "l"}));
    let ok = reply.last().unwrap();
    assert_eq!(ok["type"], "ok");
    let models = ok["models"].as_array().expect("the catalog");
    assert!(!models.is_empty());
    assert_eq!(ok["retired"], json!([]));
    let states: Vec<_> = reply.iter().filter(|m| m["type"] == "model_state").collect();
    assert_eq!(states.len(), models.len(), "one model_state for each model");
    assert!(states.iter().all(|m| m["state"] == "not_downloaded" && m["id"] == "l"));
    for entry in models {
        for field in ["id", "name", "engine", "size_bytes", "languages", "roles", "timestamps", "detects_language", "recommended"] {
            assert!(entry.get(field).is_some(), "{field} in {entry}");
        }
    }
}

#[test]
fn a_request_without_engine_takes_it_from_the_catalog() {
    let h = harness("http://unused");
    let id = h.engine.catalog["models"][0]["id"].as_str().unwrap().to_string();
    let reply = h.call(json!({"v": 1, "type": "list_models", "id": "l", "models": [{"id": id}]}));
    assert_eq!(reply[0]["state"], "not_downloaded");
    assert_eq!(reply.last().unwrap()["type"], "ok");
    assert!(reply.last().unwrap().get("models").is_none(), "only the whole catalog has the list");

    let reply = h.call(json!({"v": 1, "type": "load", "id": "o", "model": id}));
    let error = reply.last().unwrap();
    assert_eq!(error["type"], "error");
    assert!(error.get("code").is_none(), "the engine found the model: {error}");

    for kind in ["download", "load"] {
        let reply = h.call(json!({"v": 1, "type": kind, "id": "x", "model": "no-such-model"}));
        assert_eq!(reply.last().unwrap()["code"], "unknown_model", "{kind}: {reply:?}");
    }
    let reply = h.call(json!({"v": 1, "type": "list_models", "id": "l2", "models": [{"id": "no-such-model"}]}));
    assert_eq!(reply.last().unwrap()["code"], "unknown_model");
}

#[test]
fn hello_and_unknown_requests() {
    let h = harness("http://unused");
    let reply = h.call(json!({"v": 1, "type": "hello", "id": "r1"}));
    assert_eq!(reply[0]["type"], "hello");
    assert_eq!(reply[0]["id"], "r1");
    assert_eq!(reply[0]["protocol"], 1);
    assert_eq!(
        reply[0]["features"],
        json!(["catalog", "concurrent_streams", "detected_language", "diarization", "keep_preview", "segments", "store_v1", "unload", "vad"])
    );

    let prepared = h.call(json!({"v":1,"type":"prepare","id":"p","model":"m","preview":true,"vocabulary":["Sayso"]}));
    assert_eq!(prepared[0]["type"], "ok");

    let reply = h.call(json!({"v": 1, "type": "fly", "id": "r2"}));
    assert_eq!(
        reply[0],
        json!({"v": 1, "type": "error", "id": "r2", "message": "unknown request type fly"})
    );

    let reply = h.call(json!({"v": 1, "type": "load", "id": "r3"}));
    assert_eq!(reply[0]["message"], "missing string field model");
    // A model without `engine` must be in the catalog.
    let reply = h.call(json!({"v": 1, "type": "load", "id": "r4", "model": "m"}));
    assert_eq!(reply[0]["code"], "unknown_model");
}

#[test]
fn a_model_goes_from_not_downloaded_to_ready_and_back() {
    let main = "sherpa-onnx-fake-main";
    let preview = "sherpa-onnx-fake-stream";
    let server = serve_archives(vec![
        (
            main.into(),
            archive(
                main,
                &[
                    "encoder.int8.onnx",
                    "decoder.int8.onnx",
                    "joiner.int8.onnx",
                    "tokens.txt",
                ],
            ),
        ),
        (
            preview.into(),
            archive(
                preview,
                &["encoder.onnx", "decoder.onnx", "joiner.onnx", "tokens.txt"],
            ),
        ),
    ]);
    let h = harness(&server.base);
    let engine = spec_json("parakeet_tdt", main, Some(preview));

    // List: nothing on disk.
    let reply = h.call(json!({"v": 1, "type": "list_models", "id": "l1", "models": [{"id": "m", "engine": engine}]}));
    assert_eq!(states(&reply), ["not_downloaded"]);
    assert_eq!(reply[0]["id"], "l1");
    assert_eq!(last(&reply)["type"], "ok");

    // Download both archives through the redirect.
    let reply = h.call(json!({"v": 1, "type": "download", "id": "d1", "model": "m", "engine": engine, "size_bytes": 1000}));
    assert_eq!(last(&reply), &json!({"v": 1, "type": "ok", "id": "d1"}));
    let all = states(&reply);
    assert_eq!(all.first().map(String::as_str), Some("downloading"));
    assert_eq!(all.last().map(String::as_str), Some("downloaded"));
    let progress: Vec<&Value> = reply
        .iter()
        .filter(|m| m["type"] == "download_progress")
        .collect();
    assert_eq!(progress.first().unwrap()["fraction"], 0.0);
    assert_eq!(progress.last().unwrap()["fraction"], 1.0);
    assert!(progress.last().unwrap()["bytes_done"].as_u64().unwrap() > 0);
    assert_eq!(
        server.requests.load(Ordering::SeqCst),
        4,
        "two redirects and two files"
    );

    // The layout on disk.
    let folder = h.models_dir().join("m");
    assert!(folder.join(main).join("encoder.int8.onnx").is_file());
    assert!(folder.join(preview).join("tokens.txt").is_file());
    assert_eq!(
        std::fs::read_to_string(folder.join(".complete")).unwrap(),
        format!("{main}\n{preview}\n")
    );

    // A second download does nothing.
    let reply =
        h.call(json!({"v": 1, "type": "download", "id": "d2", "model": "m", "engine": engine}));
    assert_eq!(states(&reply), ["downloaded"]);
    assert_eq!(server.requests.load(Ordering::SeqCst), 4);

    let reply = h.call(json!({"v": 1, "type": "list_models", "id": "l2", "models": [{"id": "m", "engine": engine}]}));
    assert_eq!(states(&reply), ["downloaded"]);

    // Transcribe needs a load.
    let wav = h.models_dir().join("clip.wav");
    write_wav(&wav, &[1000; 16_000]);
    let transcribe = json!({"v": 1, "type": "transcribe", "id": "t1", "model": "m", "path": wav, "language": "en", "vocabulary": ["Sayso"]});
    let reply = h.call(transcribe.clone());
    assert_eq!(reply[0]["message"], "model m is not loaded");

    // Load, twice.
    let reply = h.call(json!({"v": 1, "type": "load", "id": "o1", "model": "m", "engine": engine}));
    assert_eq!(states(&reply), ["optimizing", "ready"]);
    assert!(reply[1]["load_ms"].is_u64());
    assert_eq!(last(&reply)["type"], "ok");
    let reply = h.call(json!({"v": 1, "type": "load", "id": "o2", "model": "m", "engine": engine}));
    assert_eq!(states(&reply), ["ready"]);
    assert_eq!(reply[0]["load_ms"], 0);
    assert_eq!(h.loader.loads.load(Ordering::SeqCst), 1);

    let reply = h.call(json!({"v": 1, "type": "list_models", "id": "l3", "models": [{"id": "m", "engine": engine}]}));
    assert_eq!(states(&reply), ["ready"]);

    // Final pass.
    let reply = h.call(transcribe);
    assert_eq!(reply.len(), 1);
    assert_eq!(reply[0]["type"], "final");
    assert_eq!(reply[0]["id"], "t1");
    assert_eq!(reply[0]["text"], "16000 samples in en with Sayso", "the dictionary words reach the model");
    assert!(reply[0]["elapsed_ms"].is_u64());

    // Delete.
    let reply =
        h.call(json!({"v": 1, "type": "delete", "id": "x1", "model": "m", "engine": engine}));
    assert_eq!(states(&reply), ["not_downloaded"]);
    assert_eq!(last(&reply)["type"], "ok");
    assert!(!folder.exists());
    let reply = h.call(json!({"v": 1, "type": "list_models", "id": "l4", "models": [{"id": "m", "engine": engine}]}));
    assert_eq!(states(&reply), ["not_downloaded"]);
}

#[test]
fn a_gguf_model_downloads_its_file_from_hugging_face() {
    let preview = "sherpa-onnx-fake-stream";
    let server = serve_archives(vec![
        (preview.into(), archive(preview, &["encoder.onnx", "decoder.onnx", "joiner.onnx", "tokens.txt"])),
        ("o/r/resolve/c0ffee/m.gguf".into(), b"gguf".to_vec()),
    ]);
    let h = harness(&server.base);
    let engine = json!({"kind": "parakeet_cpp", "repo": "o/r", "revision": "c0ffee", "file": "m.gguf", "preview_archive": preview});

    let reply = h.call(json!({"v": 1, "type": "download", "id": "d1", "model": "m", "engine": engine}));
    assert_eq!(last(&reply), &json!({"v": 1, "type": "ok", "id": "d1"}));
    assert_eq!(states(&reply).last().map(String::as_str), Some("downloaded"));
    let folder = h.models_dir().join("m");
    assert_eq!(std::fs::read(folder.join("c0ffee").join("m.gguf")).unwrap(), b"gguf");
    assert!(folder.join(preview).join("tokens.txt").is_file());

    // A file the repository does not have fails the download and marks nothing complete.
    let missing = json!({"kind": "parakeet_cpp", "repo": "o/r", "revision": "c0ffee", "file": "gone.gguf", "preview_archive": preview});
    let reply = h.call(json!({"v": 1, "type": "download", "id": "d2", "model": "n", "engine": missing}));
    assert_eq!(last(&reply)["type"], "error");
    assert!(!h.models_dir().join("n").join(".complete").exists());
}

#[test]
fn a_new_gguf_revision_downloads_again() {
    let server = serve_archives(vec![
        ("o/r/resolve/c0ffee/m.gguf".into(), b"old".to_vec()),
        ("o/r/resolve/beef/m.gguf".into(), b"new".to_vec()),
    ]);
    let h = harness(&server.base);
    for (id, revision) in [("d1", "c0ffee"), ("d2", "beef")] {
        let engine = json!({"kind": "parakeet_cpp", "repo": "o/r", "revision": revision, "file": "m.gguf", "preview_archive": null});
        let reply = h.call(json!({"v": 1, "type": "download", "id": id, "model": "m", "engine": engine}));
        assert_eq!(last(&reply)["type"], "ok", "{reply:?}");
    }
    assert_eq!(std::fs::read(h.models_dir().join("m").join("beef").join("m.gguf")).unwrap(), b"new");
}

#[test]
fn a_whisper_cpp_model_downloads_its_file_and_gets_the_dictionary_words() {
    let server = serve_archives(vec![("ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin".into(), b"ggml".to_vec())]);
    let h = harness(&server.base);
    let engine = json!({"kind": "whisper_cpp", "file": "ggml-tiny.bin"});

    let reply = h.call(json!({"v": 1, "type": "download", "id": "d1", "model": "w", "engine": engine}));
    assert_eq!(last(&reply)["type"], "ok", "{reply:?}");
    assert_eq!(std::fs::read(h.models_dir().join("w").join("main").join("ggml-tiny.bin")).unwrap(), b"ggml");

    let reply = h.call(json!({"v": 1, "type": "load", "id": "o1", "model": "w", "engine": engine}));
    assert_eq!(states(&reply), ["optimizing", "ready"]);
    let wav = h.models_dir().join("clip.wav");
    write_wav(&wav, &[1000; 16_000]);
    let reply = h.call(json!({"v": 1, "type": "transcribe", "id": "t1", "model": "w", "path": wav, "language": "de", "vocabulary": ["Sayso", "GitHub"]}));
    assert_eq!(reply[0]["text"], "16000 samples in de with Sayso+GitHub");
}

/// A model folder as the Windows engine before 0.12 left it: the files, then the marker.
fn old_windows_folder(h: &Harness, model: &str, files: &[&str]) {
    let folder = h.models_dir().join(model);
    std::fs::create_dir_all(&folder).unwrap();
    for file in files {
        std::fs::write(folder.join(file), b"old").unwrap();
    }
    std::fs::write(folder.join(".sayso-downloaded"), b"").unwrap();
}

#[test]
fn whisper_files_of_the_old_windows_engine_stay_and_parakeet_files_need_a_new_download() {
    let main = "sherpa-onnx-fake-main";
    let server = serve_archives(vec![(
        main.into(),
        archive(main, &["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"]),
    )]);
    let h = harness(&server.base);
    let whisper = json!({"kind": "whisper_cpp", "file": "ggml-tiny.bin"});
    let parakeet = json!({"kind": "sherpa", "recipe": "parakeet_tdt", "archive": main, "preview_archive": null, "redecode_preview": true});
    old_windows_folder(&h, "whisper-tiny", &["ggml-tiny.bin"]);
    old_windows_folder(&h, "parakeet-tdt-v3", &["encoder-model.int8.onnx", "decoder_joint-model.int8.onnx", "nemo128.onnx", "vocab.txt"]);

    let list = json!({"v": 1, "type": "list_models", "id": "l", "models": [
        {"id": "whisper-tiny", "engine": whisper},
        {"id": "parakeet-tdt-v3", "engine": parakeet},
    ]});
    let reply = h.call(list.clone());
    assert_eq!(states(&reply), ["downloaded", "failed"]);
    assert_eq!(reply[1]["reason"], "format_changed");
    assert_eq!(server.requests.load(Ordering::SeqCst), 0, "the Whisper file did not download again");

    // The download replaces the old files.
    let reply = h.call(json!({"v": 1, "type": "download", "id": "d", "model": "parakeet-tdt-v3", "engine": parakeet}));
    assert_eq!(last(&reply)["type"], "ok", "{reply:?}");
    let mut left: Vec<String> = std::fs::read_dir(h.models_dir().join("parakeet-tdt-v3"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, [".complete", main]);
    assert_eq!(states(&h.call(list)), ["downloaded", "downloaded"]);
}

#[test]
fn a_redecode_model_gives_its_live_preview_from_the_final_pass() {
    let h = harness("http://unused");
    let parakeet = json!({"kind": "sherpa", "recipe": "parakeet_tdt", "archive": "main", "preview_archive": null, "redecode_preview": true});
    install(&h, "p", &parakeet);
    h.call(json!({"v": 1, "type": "load", "id": "o", "model": "p", "engine": parakeet}));
    let mut worker = StreamWorker::new(Arc::clone(&h.engine));
    let reply = h.stream(&mut worker, json!({"v": 1, "type": "stream_start", "id": "s1", "session": 3, "model": "p", "language": "en"}));
    assert_eq!(reply, [json!({"v": 1, "type": "ok", "id": "s1"})]);
    h.stream(&mut worker, json!({"v": 1, "type": "stream_audio", "session": 3, "pcm": pcm(&[1000; 8000])}));
    let reply = h.stream(&mut worker, json!({"v": 1, "type": "stream_end", "id": "e1", "session": 3}));
    // The end of the stream decodes the audio with the final pass.
    assert_eq!(last(&reply)["text"], "8000 samples in auto");

    // The preview is the final pass, so `keep_preview` cannot keep a part: all of the model leaves.
    let reply = h.call(json!({"v": 1, "type": "unload", "id": "u", "model": "p", "keep_preview": true}));
    assert_eq!(states(&reply), ["downloaded"]);
    assert!(reply[0].get("preview").is_none());
    let reply = h.stream(&mut worker, json!({"v": 1, "type": "stream_start", "id": "s2", "session": 4, "model": "p"}));
    assert_eq!(reply[0]["message"], "model p is not loaded");
}

#[test]
fn a_failed_download_reports_failed_and_leaves_nothing_complete() {
    let server = serve_archives(vec![]);
    let h = harness(&server.base);
    let engine = spec_json("whisper", "sherpa-onnx-missing", None);
    let reply =
        h.call(json!({"v": 1, "type": "download", "id": "d1", "model": "m", "engine": engine}));
    assert_eq!(states(&reply), ["downloading", "failed"]);
    let failed = reply.iter().find(|m| m["state"] == "failed").unwrap();
    assert_eq!(failed["reason"], "download_failed");
    let error = last(&reply);
    assert_eq!(error["type"], "error");
    assert_eq!(error["id"], "d1");
    assert!(
        error["message"].as_str().unwrap().contains("404"),
        "{error}"
    );
    let folder = h.models_dir().join("m");
    assert!(!folder.join(".complete").exists());
    assert!(!folder.join(".partial-sherpa-onnx-missing").exists());

    let reply = h.call(json!({"v": 1, "type": "load", "id": "o1", "model": "m", "engine": engine}));
    assert_eq!(reply[0]["message"], "model m is not downloaded");
}

#[test]
fn an_archive_without_the_model_files_fails_the_download() {
    let name = "sherpa-onnx-empty-whisper";
    let server = serve_archives(vec![(name.into(), archive(name, &["README.md"]))]);
    let h = harness(&server.base);
    let engine = spec_json("whisper", name, None);
    let reply =
        h.call(json!({"v": 1, "type": "download", "id": "d1", "model": "m", "engine": engine}));
    assert_eq!(states(&reply).last().unwrap(), "failed");
    let failed = reply.iter().find(|m| m["state"] == "failed").unwrap();
    assert_eq!(failed["reason"], "incomplete_download");
    assert!(
        last(&reply)["message"]
            .as_str()
            .unwrap()
            .starts_with("no tokens.txt"),
        "{reply:?}"
    );
}

/// Put the archive folders and the marker in place, as a download would.
fn install(h: &Harness, model: &str, spec: &Value) {
    let spec = ModelSpec::from_json(spec).unwrap();
    for archive in spec.archives() {
        std::fs::create_dir_all(h.engine.store.archive_dir(model, archive).unwrap()).unwrap();
    }
    h.engine.store.mark_downloaded(model, &spec).unwrap();
}

#[test]
fn unload_takes_a_model_out_of_memory_and_keeps_the_files() {
    let h = harness("http://unused");
    let whisper = spec_json("whisper", "w", None);
    let streaming = spec_json("zipformer_streaming", "stream", None);
    install(&h, "w", &whisper);
    install(&h, "s", &streaming);
    let list = json!({"v": 1, "type": "list_models", "id": "l1", "models": [{"id": "w", "engine": whisper}, {"id": "s", "engine": streaming}]});
    h.call(json!({"v": 1, "type": "load", "id": "o1", "model": "w", "engine": whisper}));
    h.call(json!({"v": 1, "type": "load", "id": "o2", "model": "s", "engine": streaming}));
    assert_eq!(states(&h.call(list.clone())), ["ready", "ready"]);

    // Only the named model leaves memory. Its files stay.
    let reply = h.call(json!({"v": 1, "type": "unload", "id": "u1", "model": "w"}));
    assert_eq!(
        reply,
        [
            json!({"v": 1, "type": "model_state", "id": "u1", "model": "w", "state": "downloaded"}),
            json!({"v": 1, "type": "ok", "id": "u1"}),
        ]
    );
    assert_eq!(states(&h.call(list.clone())), ["downloaded", "ready"]);
    assert!(h.engine.store.archive_dir("w", "w").unwrap().is_dir());

    // A final pass needs a new load.
    let wav = h.models_dir().join("clip.wav");
    write_wav(&wav, &[1000; 1600]);
    let transcribe = json!({"v": 1, "type": "transcribe", "id": "t1", "model": "w", "path": wav, "language": "en"});
    assert_eq!(
        h.call(transcribe.clone())[0]["message"],
        "model w is not loaded"
    );
    let reply =
        h.call(json!({"v": 1, "type": "load", "id": "o3", "model": "w", "engine": whisper}));
    assert_eq!(states(&reply), ["optimizing", "ready"]);
    assert_eq!(h.loader.loads.load(Ordering::SeqCst), 3);
    assert_eq!(h.call(transcribe)[0]["type"], "final");

    // A model that is not in memory: `ok`, and no state change.
    h.call(json!({"v": 1, "type": "unload", "id": "u2", "model": "w"}));
    let reply = h.call(json!({"v": 1, "type": "unload", "id": "u3", "model": "w"}));
    assert_eq!(reply, [json!({"v": 1, "type": "ok", "id": "u3"})]);
    let reply = h.call(json!({"v": 1, "type": "unload", "id": "u4"}));
    assert_eq!(reply[0]["message"], "missing string field model");
}

#[test]
fn unload_with_keep_preview_drops_only_the_final_pass() {
    let h = harness("http://unused");
    // One model with a final pass and a live preview of its own.
    let both = spec_json("parakeet_tdt", "main", Some("stream"));
    install(&h, "p", &both);
    let list =
        json!({"v": 1, "type": "list_models", "id": "l", "models": [{"id": "p", "engine": both}]});
    let load = json!({"v": 1, "type": "load", "id": "o", "model": "p", "engine": both});
    h.call(load.clone());
    let wav = h.models_dir().join("clip.wav");
    write_wav(&wav, &[1000; 1600]);
    let transcribe = json!({"v": 1, "type": "transcribe", "id": "t", "model": "p", "path": wav, "language": "en"});

    let reply =
        h.call(json!({"v": 1, "type": "unload", "id": "u1", "model": "p", "keep_preview": true}));
    assert_eq!(
        reply,
        [
            json!({"v": 1, "type": "model_state", "id": "u1", "model": "p", "state": "downloaded", "preview": true}),
            json!({"v": 1, "type": "ok", "id": "u1"}),
        ]
    );
    let reply = h.call(list.clone());
    assert_eq!(states(&reply), ["downloaded"]);
    assert_eq!(reply[0]["preview"], true);
    // A second request changes nothing.
    let reply =
        h.call(json!({"v": 1, "type": "unload", "id": "u2", "model": "p", "keep_preview": true}));
    assert_eq!(reply, [json!({"v": 1, "type": "ok", "id": "u2"})]);

    // The final pass is out: an error, not a crash.
    assert_eq!(
        h.call(transcribe.clone())[0]["message"],
        "model p is not loaded"
    );
    // A stream starts and runs while the final pass is out.
    let mut worker = StreamWorker::new(Arc::clone(&h.engine));
    let reply = h.stream(&mut worker, json!({"v": 1, "type": "stream_start", "id": "s", "session": 3, "model": "p", "language": "en"}));
    assert_eq!(reply, [json!({"v": 1, "type": "ok", "id": "s"})]);
    let audio = json!({"v": 1, "type": "stream_audio", "session": 3, "pcm": pcm(&[1000; 320])});
    assert_eq!(h.stream(&mut worker, audio.clone())[0]["tentative"], "w0");

    // The load brings back the final pass only. The stream keeps its text.
    let reply = h.call(load.clone());
    assert_eq!(states(&reply), ["optimizing", "ready"]);
    assert!(reply[1].get("preview").is_none());
    assert_eq!(
        h.loader.loads.load(Ordering::SeqCst),
        1,
        "the live preview did not load again"
    );
    assert_eq!(h.loader.final_loads.load(Ordering::SeqCst), 1);
    assert_eq!(h.stream(&mut worker, audio)[0]["tentative"], "w0 w1");
    assert_eq!(states(&h.call(list.clone())), ["ready"]);
    assert_eq!(h.call(transcribe)[0]["type"], "final");
    assert_eq!(states(&h.call(load)), ["ready"]);

    // Without `keep_preview` the whole model leaves.
    let reply = h.call(json!({"v": 1, "type": "unload", "id": "u3", "model": "p"}));
    assert_eq!(states(&reply), ["downloaded"]);
    assert!(reply[0].get("preview").is_none());
    let start = json!({"v": 1, "type": "stream_start", "id": "s2", "session": 4, "model": "p"});
    assert_eq!(
        h.stream(&mut worker, start)[0]["message"],
        "model p is not loaded"
    );

    // A partial unload, then a full unload.
    h.call(json!({"v": 1, "type": "load", "id": "o2", "model": "p", "engine": both}));
    h.call(json!({"v": 1, "type": "unload", "id": "u4", "model": "p", "keep_preview": true}));
    let reply = h.call(json!({"v": 1, "type": "unload", "id": "u5", "model": "p"}));
    assert_eq!(states(&reply), ["downloaded"]);
    let reply = h.call(list);
    assert!(reply[0].get("preview").is_none());
}

#[test]
fn keep_preview_changes_nothing_for_a_model_with_one_part() {
    let h = harness("http://unused");
    // A final pass only: a full unload.
    let whisper = spec_json("whisper", "w", None);
    install(&h, "w", &whisper);
    h.call(json!({"v": 1, "type": "load", "id": "o1", "model": "w", "engine": whisper}));
    let reply =
        h.call(json!({"v": 1, "type": "unload", "id": "u1", "model": "w", "keep_preview": true}));
    assert_eq!(states(&reply), ["downloaded"]);
    assert!(reply[0].get("preview").is_none());
    // A live preview only: it stays in memory.
    let streaming = spec_json("zipformer_streaming", "stream", None);
    install(&h, "s", &streaming);
    h.call(json!({"v": 1, "type": "load", "id": "o2", "model": "s", "engine": streaming}));
    let reply =
        h.call(json!({"v": 1, "type": "unload", "id": "u2", "model": "s", "keep_preview": true}));
    assert_eq!(reply, [json!({"v": 1, "type": "ok", "id": "u2"})]);
    let reply = h.call(json!({"v": 1, "type": "list_models", "id": "l", "models": [{"id": "s", "engine": streaming}]}));
    assert_eq!(states(&reply), ["ready"]);
}

#[test]
fn serve_runs_unload_before_the_load_that_follows() {
    let h = harness("http://unused");
    let whisper = spec_json("whisper", "w", None);
    install(&h, "w", &whisper);
    h.call(json!({"v": 1, "type": "load", "id": "o1", "model": "w", "engine": whisper}));
    h.captured.clear();
    let input = [
        json!({"v": 1, "type": "unload", "id": "u", "model": "w"}).to_string(),
        json!({"v": 1, "type": "load", "id": "o", "model": "w", "engine": whisper}).to_string(),
    ]
    .join("\n");
    serve(std::io::Cursor::new(input), Arc::clone(&h.engine));
    let deadline = Instant::now() + Duration::from_secs(5);
    let done = |messages: &[Value]| messages.iter().any(|m| m["id"] == "o" && m["type"] == "ok");
    while !done(&h.captured.messages()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let messages = h.captured.messages();
    assert!(done(&messages), "{messages:?}");
    // The load ran again. It did not answer from the model that the unload removed.
    assert_eq!(states(&messages), ["downloaded", "optimizing", "ready"]);
    assert_eq!(h.loader.loads.load(Ordering::SeqCst), 2);
}

#[test]
fn streams_send_partials_and_end_with_the_text() {
    let h = harness("http://unused");
    let streaming = spec_json("zipformer_streaming", "stream", None);
    install(&h, "s", &streaming);
    h.call(json!({"v": 1, "type": "load", "id": "o1", "model": "s", "engine": streaming}));
    let mut worker = StreamWorker::new(Arc::clone(&h.engine));

    let reply = h.stream(&mut worker, json!({"v": 1, "type": "stream_start", "id": "s1", "session": 7, "model": "s", "language": "en"}));
    assert_eq!(reply, [json!({"v": 1, "type": "ok", "id": "s1"})]);

    let loud = pcm(&[1000; 320]);
    let silent = pcm(&[0; 320]);
    let audio = |pcm: &str| json!({"v": 1, "type": "stream_audio", "session": 7, "pcm": pcm});
    let reply = h.stream(&mut worker, audio(&loud));
    assert_eq!(
        reply,
        [json!({"v": 1, "type": "partial", "session": 7, "committed": "", "tentative": "w0"})]
    );
    let reply = h.stream(&mut worker, audio(&loud));
    assert_eq!(reply[0]["tentative"], "w0 w1");
    let reply = h.stream(&mut worker, audio(&silent));
    assert_eq!(reply[0]["committed"], "w0 w1");
    assert_eq!(reply[0]["tentative"], "");
    let reply = h.stream(&mut worker, audio(&silent));
    assert!(reply.is_empty(), "no change, no partial: {reply:?}");
    h.stream(&mut worker, audio(&loud));

    let reply = h.stream(
        &mut worker,
        json!({"v": 1, "type": "stream_end", "id": "e1", "session": 7}),
    );
    assert_eq!(reply[0]["type"], "partial");
    assert_eq!(reply[0]["committed"], "w0 w1 w0");
    assert_eq!(
        reply[1],
        json!({"v": 1, "type": "ok", "id": "e1", "session": 7, "text": "w0 w1 w0"})
    );

    // The session is gone.
    let reply = h.stream(&mut worker, audio(&loud));
    assert_eq!(reply[0]["message"], "unknown stream session 7");
    // A streaming model has no final pass.
    let wav = h.models_dir().join("clip.wav");
    write_wav(&wav, &[1000; 160]);
    let reply =
        h.call(json!({"v": 1, "type": "transcribe", "id": "t1", "model": "s", "path": wav}));
    assert_eq!(reply[0]["message"], "model s has no final pass");
}

#[test]
fn streams_on_one_model_run_at_the_same_time() {
    let h = harness("http://unused");
    let streaming = spec_json("zipformer_streaming", "stream", None);
    install(&h, "s", &streaming);
    h.call(json!({"v": 1, "type": "load", "id": "o1", "model": "s", "engine": streaming}));
    let mut worker = StreamWorker::new(Arc::clone(&h.engine));
    for session in [1, 2, 3] {
        let reply = h.stream(&mut worker, json!({"v": 1, "type": "stream_start", "id": format!("s{session}"), "session": session, "model": "s"}));
        assert_eq!(reply[0]["type"], "ok", "{reply:?}");
    }
    let loud = pcm(&[1000; 320]);
    for session in [1, 2, 3] {
        let reply = h.stream(&mut worker, json!({"v": 1, "type": "stream_audio", "session": session, "pcm": loud}));
        assert_eq!(reply[0]["session"], session, "each stream keeps its own text: {reply:?}");
        assert_eq!(reply[0]["tentative"], "w0");
    }
    for session in [3, 1, 2] {
        let reply = h.stream(&mut worker, json!({"v": 1, "type": "stream_end", "id": format!("e{session}"), "session": session}));
        assert_eq!(reply.last().unwrap()["text"], "w0", "{reply:?}");
    }
}

#[test]
fn a_stream_past_the_limit_gets_too_many_streams() {
    let h = harness("http://unused");
    let streaming = spec_json("zipformer_streaming", "stream", None);
    install(&h, "s", &streaming);
    h.call(json!({"v": 1, "type": "load", "id": "o1", "model": "s", "engine": streaming}));
    let mut worker = StreamWorker::new(Arc::clone(&h.engine));
    let start = |session: u64| json!({"v": 1, "type": "stream_start", "id": "s", "session": session, "model": "s"});
    for session in 0..MAX_STREAMS as u64 {
        assert_eq!(h.stream(&mut worker, start(session))[0]["type"], "ok");
    }
    let reply = h.stream(&mut worker, start(99));
    assert_eq!(reply[0]["type"], "error");
    assert_eq!(reply[0]["code"], "too_many_streams");
    // The same session id again replaces its stream and needs no new place.
    assert_eq!(h.stream(&mut worker, start(0))[0]["type"], "ok");
}

#[test]
fn transcribe_with_segments_gives_one_segment_for_each_untimed_piece() {
    let h = harness("http://unused");
    let whisper = spec_json("whisper", "w", None);
    install(&h, "w", &whisper);
    h.call(json!({"v": 1, "type": "load", "id": "o1", "model": "w", "engine": whisper}));
    let wav = h.models_dir().join("long.wav");
    write_wav(&wav, &vec![1000; 16_000 * 40]);
    let reply = h.call(json!({"v": 1, "type": "transcribe", "id": "t1", "model": "w", "path": wav, "language": "en", "segments": true}));
    let segments = reply[0]["segments"].as_array().expect("segments");
    assert_eq!(segments.len(), 2, "{reply:?}");
    assert_eq!(segments[0]["start_ms"], 0);
    assert_eq!(segments[0]["end_ms"], segments[1]["start_ms"]);
    assert_eq!(segments[1]["end_ms"], 40_000);
    assert!(segments[0]["text"].as_str().unwrap().ends_with("samples in en"));
    assert!(segments[0].get("words").is_none());

    let reply = h.call(json!({"v": 1, "type": "transcribe", "id": "t2", "model": "w", "path": wav, "language": "en"}));
    assert!(reply[0].get("segments").is_none(), "only when asked");
}

#[test]
fn stream_start_needs_a_loaded_model_with_a_live_preview() {
    let h = harness("http://unused");
    let batch = spec_json("sense_voice", "batch", None);
    install(&h, "b", &batch);
    let mut worker = StreamWorker::new(Arc::clone(&h.engine));
    let start = json!({"v": 1, "type": "stream_start", "id": "s1", "session": 1, "model": "b", "language": "en"});
    assert_eq!(
        h.stream(&mut worker, start.clone())[0]["message"],
        "model b is not loaded"
    );
    h.call(json!({"v": 1, "type": "load", "id": "o1", "model": "b", "engine": batch}));
    assert_eq!(
        h.stream(&mut worker, start)[0]["message"],
        "model b has no live preview"
    );
}

#[test]
fn long_recordings_are_cut_into_pieces() {
    let h = harness("http://unused");
    let whisper = spec_json("whisper", "w", None);
    install(&h, "w", &whisper);
    h.call(json!({"v": 1, "type": "load", "id": "o1", "model": "w", "engine": whisper}));
    let wav = h.models_dir().join("long.wav");
    write_wav(&wav, &vec![1000; 16_000 * 40]);
    let reply = h.call(json!({"v": 1, "type": "transcribe", "id": "t1", "model": "w", "path": wav, "language": "auto"}));
    let text = reply[0]["text"].as_str().unwrap();
    assert_eq!(text.matches("samples in auto").count(), 2, "{text}");
}

/// `seconds` of audio at `level`.
fn level(level: i16, seconds: usize) -> Vec<i16> {
    vec![level; 16_000 * seconds]
}

/// A transcribe of `samples` with `language`, by a loaded Whisper model.
fn transcribe_whisper(samples: &[i16], language: &str) -> Value {
    let h = harness("http://unused");
    let whisper = spec_json("whisper", "w", None);
    install(&h, "w", &whisper);
    h.call(json!({"v": 1, "type": "load", "id": "o1", "model": "w", "engine": whisper}));
    let wav = h.models_dir().join("clip.wav");
    write_wav(&wav, samples);
    h.call(json!({"v": 1, "type": "transcribe", "id": "t1", "model": "w", "path": wav, "language": language}))[0].clone()
}

#[test]
fn auto_gives_the_language_of_the_most_audio() {
    // Whisper takes 28 s pieces. The cut goes in the second of silence after
    // 24 s of German. The English after it is cut into two pieces.
    let mut german_first = level(1000, 24);
    german_first.extend(level(0, 1));
    german_first.extend(level(-1000, 30));
    let reply = transcribe_whisper(&german_first, "auto");
    assert_eq!(reply["text"].as_str().unwrap().matches("samples in auto").count(), 3, "{reply}");
    assert_eq!(reply["language"], "en", "30 s of English against 24 s of German: {reply}");

    let mut more_german = level(1000, 24);
    more_german.extend(level(0, 1));
    more_german.extend(level(-1000, 10));
    let reply = transcribe_whisper(&more_german, "");
    assert_eq!(reply["language"], "de", "an empty language is auto: {reply}");
}

#[test]
fn a_named_language_or_no_text_gives_no_language() {
    for named in ["en", "de", "en-US"] {
        let reply = transcribe_whisper(&level(1000, 2), named);
        assert_eq!(reply["type"], "final");
        assert!(reply.get("language").is_none(), "{named}: {reply}");
    }
    let reply = transcribe_whisper(&level(0, 2), "auto");
    assert_eq!(reply["text"], "");
    assert!(reply.get("language").is_none(), "{reply}");
}

#[test]
fn the_catalog_decides_which_models_report_the_language() {
    let h = harness("http://unused");
    let engine_of = |id: &str| {
        h.engine.catalog["models"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap()["engine"].clone()
    };
    let wav = h.models_dir().join("clip.wav");
    std::fs::create_dir_all(h.models_dir()).unwrap();
    let transcribe = |model: &str, samples: &[i16]| {
        write_wav(&wav, samples);
        h.call(json!({"v": 1, "type": "transcribe", "id": "t", "model": model, "path": wav, "language": "auto"}))[0].clone()
    };
    for model in ["whisper-small", "sensevoice-small", "moonshine-base-en"] {
        let engine = engine_of(model);
        install(&h, model, &engine);
        let reply = h.call(json!({"v": 1, "type": "load", "id": "o", "model": model, "engine": engine}));
        assert_eq!(last(&reply)["type"], "ok", "{reply:?}");
    }
    // Whisper finds the language, but only codes of its catalog languages go out.
    assert_eq!(transcribe("whisper-small", &level(-1000, 1))["language"], "en");
    let reply = transcribe("whisper-small", &level(3000, 1));
    assert!(reply.get("language").is_none(), "yue is not a language of Whisper small: {reply}");
    // The catalog says that SenseVoice on sherpa-onnx and Moonshine (one language) do not find it.
    for model in ["sensevoice-small", "moonshine-base-en"] {
        let reply = transcribe(model, &level(-1000, 1));
        assert!(reply["text"].as_str().is_some_and(|t| !t.is_empty()), "{reply}");
        assert!(reply.get("language").is_none(), "{model}: {reply}");
    }
}

#[test]
fn serve_routes_requests_and_stops_on_shutdown() {
    let h = harness("http://unused");
    let input = [
        r#"{"v":1,"type":"hello","id":"a"}"#,
        "",
        "garbage",
        r#"{"v":1,"type":"stream_end","id":"b","session":4}"#,
        r#"{"v":1,"type":"shutdown","id":"c"}"#,
        r#"{"v":1,"type":"hello","id":"never"}"#,
    ]
    .join("\n");
    serve(std::io::Cursor::new(input), Arc::clone(&h.engine));
    // The hello and stream threads may still be writing. Wait for them.
    let deadline = Instant::now() + Duration::from_secs(5);
    while h.captured.messages().len() < 4 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let messages = h.captured.messages();
    let by_id = |id: &str| messages.iter().find(|m| m["id"] == id).cloned();
    assert_eq!(by_id("a").unwrap()["type"], "hello");
    assert_eq!(by_id("b").unwrap()["message"], "unknown stream session 4");
    assert_eq!(by_id("c").unwrap()["type"], "ok");
    assert!(by_id("never").is_none());
    assert!(
        messages
            .iter()
            .any(|m| m["message"] == "invalid JSON line" && m.get("id").is_none())
    );
}

// ---------------------------------------------------------------------------
// VAD
// ---------------------------------------------------------------------------

fn silero() -> Value {
    json!({"kind": "sherpa_vad", "recipe": "silero", "file": "silero_vad.onnx"})
}

fn diarization_engine() -> Value {
    json!({"kind": "sherpa_diarization", "segmentation": "seg-archive", "embedding": "speaker.onnx"})
}

/// 16 kHz samples of loud and quiet pieces: `(milliseconds, loud)`.
fn clip(pieces: &[(usize, bool)]) -> Vec<i16> {
    audio_of(pieces).iter().map(|s| (s * 32767.0) as i16).collect()
}

fn load_model(h: &Harness, model: &str, engine: &Value) {
    install(h, model, engine);
    let reply = h.call(json!({"v": 1, "type": "load", "id": "load", "model": model, "engine": engine}));
    assert_eq!(last(&reply)["type"], "ok", "{reply:?}");
}

#[test]
fn a_vad_model_downloads_one_file_loads_and_is_deleted() {
    let server = serve_archives(vec![("silero_vad.onnx".into(), b"onnx".to_vec())]);
    let h = harness(&server.base);
    let engine = silero();
    let list = json!({"v": 1, "type": "list_models", "id": "l1", "models": [{"id": "v", "engine": engine}]});
    assert_eq!(states(&h.call(list.clone())), ["not_downloaded"]);

    let reply = h.call(json!({"v": 1, "type": "download", "id": "d1", "model": "v", "engine": engine}));
    assert_eq!(last(&reply), &json!({"v": 1, "type": "ok", "id": "d1"}));
    assert_eq!(states(&reply).last().map(String::as_str), Some("downloaded"));
    assert_eq!(std::fs::read(h.models_dir().join("v/silero_vad/silero_vad.onnx")).unwrap(), b"onnx");
    assert_eq!(states(&h.call(list.clone())), ["downloaded"]);

    let reply = h.call(json!({"v": 1, "type": "load", "id": "o1", "model": "v", "engine": engine}));
    assert_eq!(states(&reply), ["optimizing", "ready"]);
    assert_eq!(states(&h.call(list.clone())), ["ready"]);
    let reply = h.call(json!({"v": 1, "type": "unload", "id": "u1", "model": "v"}));
    assert_eq!(states(&reply), ["downloaded"]);
    h.call(json!({"v": 1, "type": "load", "id": "o2", "model": "v", "engine": engine}));

    let reply = h.call(json!({"v": 1, "type": "delete", "id": "x1", "model": "v"}));
    assert_eq!(states(&reply), ["not_downloaded"]);
    assert!(!h.models_dir().join("v").exists());
    assert_eq!(states(&h.call(list)), ["not_downloaded"]);
}

#[test]
fn an_empty_vad_file_fails_the_download_as_incomplete() {
    let server = serve_archives(vec![("silero_vad.onnx".into(), Vec::new())]);
    let h = harness(&server.base);
    let reply = h.call(json!({"v": 1, "type": "download", "id": "d1", "model": "v", "engine": silero()}));
    assert_eq!(last(&reply)["type"], "error");
    let failed = reply.iter().find(|m| m["state"] == "failed").expect("model_state failed");
    assert_eq!(failed["reason"], "incomplete_download");
    assert!(!h.models_dir().join("v/.complete").exists());
}

#[test]
fn a_diarization_model_downloads_from_its_two_releases() {
    let seg = "seg-archive";
    let server = serve_archives(vec![
        (
            "speaker-segmentation-models/seg-archive".into(),
            archive(seg, &["model.onnx", "model.int8.onnx"]),
        ),
        ("speaker-recongition-models/speaker.onnx".into(), b"embedding".to_vec()),
    ]);
    let h = harness(&server.base);
    let engine = diarization_engine();
    let reply = h.call(json!({"v": 1, "type": "download", "id": "d1", "model": "d", "engine": engine}));
    assert_eq!(last(&reply)["type"], "ok", "{reply:?}");
    let folder = h.models_dir().join("d");
    assert!(folder.join("seg-archive/model.onnx").is_file());
    assert_eq!(std::fs::read(folder.join("speaker/speaker.onnx")).unwrap(), b"embedding");
    assert_eq!(std::fs::read_to_string(folder.join(".complete")).unwrap(), "seg-archive\nspeaker\n");

    let list = json!({"v": 1, "type": "list_models", "id": "l1", "models": [{"id": "d", "engine": engine}]});
    assert_eq!(states(&h.call(list.clone())), ["downloaded"]);
    let reply = h.call(json!({"v": 1, "type": "load", "id": "o1", "model": "d", "engine": engine}));
    assert_eq!(states(&reply), ["optimizing", "ready"]);

    h.call(json!({"v": 1, "type": "unload", "id": "u1", "model": "d"}));
    let other = json!({"kind": "sherpa_diarization", "segmentation": "seg-archive", "embedding": "other.onnx"});
    let list_other = json!({"v": 1, "type": "list_models", "id": "l2", "models": [{"id": "d", "engine": other}]});
    assert_eq!(states(&h.call(list_other)), ["not_downloaded"], "another embedding model is another download");
    h.call(json!({"v": 1, "type": "delete", "id": "x1", "model": "d"}));
    assert_eq!(states(&h.call(list)), ["not_downloaded"]);
}

#[test]
fn a_diarization_archive_without_the_segmentation_model_fails_the_download() {
    let server = serve_archives(vec![
        ("speaker-segmentation-models/seg-archive".into(), archive("seg-archive", &["README.md"])),
        ("speaker-recongition-models/speaker.onnx".into(), b"embedding".to_vec()),
    ]);
    let h = harness(&server.base);
    let reply = h.call(json!({"v": 1, "type": "download", "id": "d1", "model": "d", "engine": diarization_engine()}));
    assert_eq!(last(&reply)["type"], "error");
    assert!(!h.models_dir().join("d/.complete").exists());
}

#[test]
fn the_vad_request_and_a_vad_session_find_the_same_parts() {
    let h = harness("http://unused");
    load_model(&h, "v", &silero());
    let samples = clip(&[(800, false), (2500, true), (1200, false), (600, true), (300, false), (900, true), (1500, false)]);
    let wav = h.models_dir().join("speech.wav");
    write_wav(&wav, &samples);

    let reply = h.call(json!({"v": 1, "type": "vad", "id": "v1", "model": "v", "path": wav}));
    assert_eq!(reply.len(), 1, "{reply:?}");
    let speech = reply[0]["speech"].as_array().expect("speech");
    assert_eq!(speech.len(), 2, "the 300 ms pause does not end the second part: {reply:?}");
    assert_eq!(reply[0]["id"], "v1");

    let mut worker = StreamWorker::new(Arc::clone(&h.engine));
    let reply = h.stream(&mut worker, json!({"v": 1, "type": "vad_start", "id": "s1", "session": 9, "model": "v"}));
    assert_eq!(reply, [json!({"v": 1, "type": "ok", "id": "s1"})]);
    let mut events = Vec::new();
    for chunk in samples.chunks(2560) {
        events.extend(h.stream(&mut worker, json!({"v": 1, "type": "stream_audio", "session": 9, "pcm": pcm(chunk)})));
    }
    let end = h.stream(&mut worker, json!({"v": 1, "type": "stream_end", "id": "e1", "session": 9}));
    // The last speech_end comes before the reply, and nothing comes after it.
    assert_eq!(last(&end), &json!({"v": 1, "type": "ok", "id": "e1", "session": 9}));
    events.extend(end[..end.len() - 1].iter().cloned());
    assert!(events.iter().all(|m| m.get("id").is_none() && m["session"] == 9), "{events:?}");

    let types: Vec<&str> = events.iter().map(|m| m["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["speech_start", "speech_end", "speech_start", "speech_end"], "{events:?}");
    let ends: Vec<&Value> = events.iter().filter(|m| m["type"] == "speech_end").collect();
    for (part, event) in speech.iter().zip(&ends) {
        assert_eq!(event["start_ms"], part["start_ms"]);
        assert_eq!(event["end_ms"], part["end_ms"]);
    }
    for pair in events.chunks(2) {
        assert_eq!(pair[0]["start_ms"], pair[1]["start_ms"], "speech_end repeats the start of its speech_start");
    }

    // The session is gone.
    let reply = h.stream(&mut worker, json!({"v": 1, "type": "stream_audio", "session": 9, "pcm": pcm(&[0; 160])}));
    assert_eq!(reply[0]["message"], "unknown stream session 9");
}

#[test]
fn stream_end_closes_speech_that_is_still_open() {
    let h = harness("http://unused");
    load_model(&h, "v", &silero());
    let mut worker = StreamWorker::new(Arc::clone(&h.engine));
    h.stream(&mut worker, json!({"v": 1, "type": "vad_start", "id": "s1", "session": 1, "model": "v"}));
    let samples = clip(&[(500, false), (1500, true)]);
    let mut events = Vec::new();
    for chunk in samples.chunks(2560) {
        events.extend(h.stream(&mut worker, json!({"v": 1, "type": "stream_audio", "session": 1, "pcm": pcm(chunk)})));
    }
    let types: Vec<&str> = events.iter().map(|m| m["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["speech_start"], "the speech has no pause after it yet");
    let end = h.stream(&mut worker, json!({"v": 1, "type": "stream_end", "id": "e1", "session": 1}));
    assert_eq!(end.len(), 2, "{end:?}");
    assert_eq!(end[0]["type"], "speech_end");
    assert_eq!(end[0]["start_ms"], events[0]["start_ms"]);
    assert!(end[0]["end_ms"].as_u64().unwrap() <= 2000, "{end:?}");
    assert_eq!(end[1]["type"], "ok");
}

#[test]
fn a_vad_request_on_audio_without_speech_gets_an_empty_list() {
    let h = harness("http://unused");
    load_model(&h, "v", &silero());
    let wav = h.models_dir().join("quiet.wav");
    write_wav(&wav, &clip(&[(2000, false)]));
    let reply = h.call(json!({"v": 1, "type": "vad", "id": "v1", "model": "v", "path": wav}));
    assert_eq!(reply[0]["speech"], json!([]));
    let missing = h.models_dir().join("missing.wav");
    let reply = h.call(json!({"v": 1, "type": "vad", "id": "v2", "model": "v", "path": missing}));
    assert_eq!(reply[0]["type"], "error");
}

#[test]
fn vad_sessions_count_toward_the_stream_limit() {
    let h = harness("http://unused");
    let streaming = spec_json("zipformer_streaming", "stream", None);
    load_model(&h, "s", &streaming);
    load_model(&h, "v", &silero());
    let mut worker = StreamWorker::new(Arc::clone(&h.engine));
    let preview = |session: u64| json!({"v": 1, "type": "stream_start", "id": "s", "session": session, "model": "s"});
    let vad = |session: u64| json!({"v": 1, "type": "vad_start", "id": "s", "session": session, "model": "v"});
    for session in 0..4 {
        assert_eq!(h.stream(&mut worker, preview(session))[0]["type"], "ok");
        assert_eq!(h.stream(&mut worker, vad(100 + session))[0]["type"], "ok");
    }
    assert_eq!(MAX_STREAMS, 8, "the test opens eight streams");
    for reply in [h.stream(&mut worker, vad(200)), h.stream(&mut worker, preview(200))] {
        assert_eq!(reply[0]["type"], "error");
        assert_eq!(reply[0]["code"], "too_many_streams");
    }
    // The same session id again takes its own place. A session that ends gives its place back.
    assert_eq!(h.stream(&mut worker, vad(100))[0]["type"], "ok");
    h.stream(&mut worker, json!({"v": 1, "type": "stream_end", "id": "e", "session": 101}));
    assert_eq!(h.stream(&mut worker, vad(200))[0]["type"], "ok");
}

#[test]
fn a_request_with_the_wrong_kind_of_model_gets_an_error() {
    let h = harness("http://unused");
    let speech = spec_json("zipformer_streaming", "stream", None);
    let batch = spec_json("sense_voice", "batch", None);
    load_model(&h, "s", &speech);
    load_model(&h, "b", &batch);
    load_model(&h, "v", &silero());
    load_model(&h, "d", &diarization_engine());
    let wav = h.models_dir().join("clip.wav");
    write_wav(&wav, &clip(&[(1000, true)]));
    let mut worker = StreamWorker::new(Arc::clone(&h.engine));

    let message = |reply: Vec<Value>| {
        assert_eq!(reply.len(), 1, "{reply:?}");
        assert_eq!(reply[0]["type"], "error", "{reply:?}");
        reply[0]["message"].as_str().unwrap().to_string()
    };
    let transcribe = |model: &str| json!({"v": 1, "type": "transcribe", "id": "t", "model": model, "path": wav});
    let vad = |model: &str| json!({"v": 1, "type": "vad", "id": "t", "model": model, "path": wav});
    let diarize = |model: &str| json!({"v": 1, "type": "diarize", "id": "t", "model": model, "path": wav});
    let stream_start = |model: &str| json!({"v": 1, "type": "stream_start", "id": "t", "session": 1, "model": model});
    let vad_start = |model: &str| json!({"v": 1, "type": "vad_start", "id": "t", "session": 1, "model": model});

    assert_eq!(message(h.call(transcribe("v"))), "model v is a VAD model, not a speech model");
    assert_eq!(message(h.call(transcribe("d"))), "model d is a diarization model, not a speech model");
    assert_eq!(message(h.stream(&mut worker, stream_start("v"))), "model v is a VAD model, not a speech model");
    assert_eq!(message(h.stream(&mut worker, stream_start("d"))), "model d is a diarization model, not a speech model");
    for speech_model in ["s", "b"] {
        assert_eq!(message(h.call(vad(speech_model))), format!("model {speech_model} is a speech model, not a VAD model"));
        assert_eq!(
            message(h.stream(&mut worker, vad_start(speech_model))),
            format!("model {speech_model} is a speech model, not a VAD model")
        );
        assert_eq!(
            message(h.call(diarize(speech_model))),
            format!("model {speech_model} is a speech model, not a diarization model")
        );
    }
    assert_eq!(message(h.call(vad("d"))), "model d is a diarization model, not a VAD model");
    assert_eq!(message(h.stream(&mut worker, vad_start("d"))), "model d is a diarization model, not a VAD model");
    assert_eq!(message(h.call(diarize("v"))), "model v is a VAD model, not a diarization model");

    // A model that is not loaded.
    assert_eq!(message(h.call(vad("nope"))), "model nope is not loaded");
    assert_eq!(message(h.stream(&mut worker, vad_start("nope"))), "model nope is not loaded");
    assert_eq!(message(h.call(diarize("nope"))), "model nope is not loaded");
    let unloaded = h.call(json!({"v": 1, "type": "unload", "id": "u", "model": "v"}));
    assert_eq!(last(&unloaded)["type"], "ok");
    assert_eq!(message(h.call(vad("v"))), "model v is not loaded");

    // No session was opened by the errors.
    assert_eq!(h.stream(&mut worker, json!({"v": 1, "type": "stream_end", "id": "e", "session": 1}))[0]["type"], "error");
}

#[test]
fn diarize_numbers_the_speakers_and_joins_their_turns() {
    let h = harness("http://unused");
    load_model(&h, "d", &diarization_engine());
    let wav = h.models_dir().join("talk.wav");
    write_wav(&wav, &vec![1000; 16_000 * 4]);

    let reply = h.call(json!({"v": 1, "type": "diarize", "id": "z1", "model": "d", "path": wav}));
    assert_eq!(reply.len(), 1, "{reply:?}");
    assert_eq!(reply[0]["id"], "z1");
    assert_eq!(reply[0]["type"], "ok");
    assert_eq!(reply[0]["speakers"], 2);
    // The model's 9 spoke first and is 0. Its two overlapping turns are one.
    assert_eq!(
        reply[0]["turns"],
        json!([
            {"start_ms": 0, "end_ms": 1500, "speaker": 0},
            {"start_ms": 2000, "end_ms": 3000, "speaker": 1},
            {"start_ms": 2500, "end_ms": 3500, "speaker": 0},
        ])
    );

    let reply = h.call(json!({"v": 1, "type": "diarize", "id": "z2", "model": "d", "path": wav, "speakers": 1}));
    assert_eq!(reply[0]["speakers"], 1);
    assert_eq!(reply[0]["turns"], json!([{"start_ms": 0, "end_ms": 4000, "speaker": 0}]));
    let null = h.call(json!({"v": 1, "type": "diarize", "id": "z3", "model": "d", "path": wav, "speakers": null}));
    assert_eq!(null[0]["type"], "ok");
    assert_eq!(*h.loader.diarized.lock().unwrap(), [None, Some(1), None], "the model gets the number of speakers");
}

#[test]
fn diarize_checks_speakers_and_handles_audio_without_speech() {
    let h = harness("http://unused");
    load_model(&h, "d", &diarization_engine());
    let wav = h.models_dir().join("talk.wav");
    write_wav(&wav, &vec![1000; 16_000]);
    for bad in [json!(0), json!(-1), json!(1.5), json!("2"), json!(true), json!([2])] {
        let reply = h.call(json!({"v": 1, "type": "diarize", "id": "z", "model": "d", "path": wav, "speakers": bad}));
        assert_eq!(reply.len(), 1, "{bad}: {reply:?}");
        assert_eq!(reply[0]["type"], "error", "{bad}");
        assert_eq!(reply[0]["message"], "speakers must be an integer of 1 or more", "{bad}");
    }
    assert!(h.loader.diarized.lock().unwrap().is_empty(), "a bad number never reaches the model");

    // A file without samples has no speech.
    let empty = h.models_dir().join("empty.wav");
    write_wav(&empty, &[]);
    let reply = h.call(json!({"v": 1, "type": "diarize", "id": "z", "model": "d", "path": empty}));
    assert_eq!(reply[0]["speakers"], 0);
    assert_eq!(reply[0]["turns"], json!([]));
    let missing = h.models_dir().join("missing.wav");
    let reply = h.call(json!({"v": 1, "type": "diarize", "id": "z", "model": "d", "path": missing}));
    assert_eq!(reply[0]["type"], "error");
}

#[test]
fn serve_runs_vad_start_on_the_stream_worker() {
    let h = harness("http://unused");
    load_model(&h, "v", &silero());
    let input = [
        r#"{"v":1,"type":"vad_start","id":"a","session":3,"model":"v"}"#,
        r#"{"v":1,"type":"vad_start","id":"b","session":4,"model":"nope"}"#,
        r#"{"v":1,"type":"stream_end","id":"c","session":3}"#,
    ]
    .join("\n");
    h.captured.clear();
    serve(std::io::Cursor::new(input), Arc::clone(&h.engine));
    let deadline = Instant::now() + Duration::from_secs(5);
    while h.captured.messages().len() < 3 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let messages = h.captured.messages();
    let ids: Vec<&str> = messages.iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["a", "b", "c"], "in arrival order: {messages:?}");
    assert_eq!(messages[1]["message"], "model nope is not loaded");
    assert_eq!(messages[2], json!({"v": 1, "type": "ok", "id": "c", "session": 3}));
}

// ---------------------------------------------------------------------------
// Model store
// ---------------------------------------------------------------------------

mod stored {
    use super::*;
    use sayso_core::engine_catalog::descriptors::{Artifact, File, Part, Source as FileSource};
    use sha2::{Digest as _, Sha256};
    use std::collections::BTreeMap;

    const VAD: &str = "silero_vad.onnx";

    fn vad_engine() -> Value {
        json!({ "kind": "sherpa_vad", "recipe": "silero", "file": VAD })
    }

    /// Descriptors with the model `m`: one file on the fake server, with the bytes `claimed`.
    fn fixture(server: &Server, claimed: &[u8]) -> &'static Descriptors {
        let files = vec![File {
            path: VAD.into(),
            size: claimed.len() as u64,
            sha256: Sha256::digest(claimed).iter().map(|b| format!("{b:02x}")).collect(),
        }];
        let artifact = Artifact {
            source: FileSource::File { url: format!("{}/{VAD}", server.base) },
            digest: descriptors::digest(&files).unwrap(),
            files,
        };
        Box::leak(Box::new(Descriptors {
            artifacts: BTreeMap::from([("vad".to_string(), artifact)]),
            descriptors: vec![Descriptor {
                model: "m".into(),
                format: descriptors::SHERPA_ONNX.into(),
                revision: 1,
                parts: vec![Part { role: "main".into(), artifact: "vad".into() }],
            }],
        }))
    }

    fn engine_on(server: &Server, root: &std::path::Path, data: &'static Descriptors) -> Harness {
        harness_with(&server.base, Some(Store::open(root.to_path_buf()).unwrap()), data)
    }

    fn request(kind: &str, id: &str) -> Value {
        json!({ "v": 1, "type": kind, "id": id, "model": "m", "engine": vad_engine() })
    }

    fn listed_state(engine: &Harness) -> String {
        let messages = engine.call(json!({ "v": 1, "type": "list_models", "id": "l", "models": [{ "id": "m", "engine": vad_engine() }] }));
        states(&messages).pop().unwrap()
    }

    #[test]
    fn a_model_goes_through_the_store_and_not_the_models_folder() {
        let server = serve_archives(vec![(VAD.to_string(), b"model bytes".to_vec())]);
        let data = fixture(&server, b"model bytes");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("v1");
        let engine = engine_on(&server, &root, data);

        assert_eq!(listed_state(&engine), "not_downloaded");
        let messages = engine.call(request("load", "l1"));
        assert_eq!(last(&messages)["type"], "error", "{messages:?}");
        assert_eq!(states(&messages), ["not_downloaded"]);

        let messages = engine.call(request("download", "d1"));
        assert_eq!(last(&messages)["type"], "ok", "{messages:?}");
        assert_eq!(states(&messages).last().unwrap(), "downloaded");
        assert_eq!(listed_state(&engine), "downloaded");
        let digest = &data.artifacts["vad"].digest;
        assert_eq!(std::fs::read(root.join("artifacts").join(digest).join(VAD)).unwrap(), b"model bytes");
        assert!(!engine.models_dir().join("m").exists(), "nothing in the models folder");

        // A second download finds it.
        let before = server.requests.load(Ordering::SeqCst);
        assert_eq!(last(&engine.call(request("download", "d2")))["type"], "ok");
        assert_eq!(server.requests.load(Ordering::SeqCst), before);

        assert_eq!(last(&engine.call(request("load", "l2")))["type"], "ok");
        assert_eq!(listed_state(&engine), "ready");
        assert_eq!(states(&engine.call(request("unload", "u"))), ["downloaded"]);
        assert_eq!(last(&engine.call(request("delete", "x")))["type"], "ok");
        assert_eq!(listed_state(&engine), "not_downloaded");
        assert!(!root.join("artifacts").join(digest).exists());
    }

    #[test]
    fn wrong_bytes_are_an_incomplete_download_and_leave_nothing() {
        let server = serve_archives(vec![(VAD.to_string(), b"other bytes".to_vec())]);
        let data = fixture(&server, b"model bytes");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("v1");
        let engine = engine_on(&server, &root, data);
        let messages = engine.call(request("download", "d"));
        assert_eq!(last(&messages)["type"], "error");
        let failed = messages.iter().find(|m| m["state"] == "failed").unwrap();
        assert_eq!(failed["reason"], "incomplete_download");
        assert_eq!(std::fs::read_dir(root.join("artifacts")).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(root.join("staging")).unwrap().count(), 0);
    }

    #[test]
    fn a_loaded_model_stays_until_it_is_unloaded_when_another_engine_deletes_it() {
        let server = serve_archives(vec![(VAD.to_string(), b"model bytes".to_vec())]);
        let data = fixture(&server, b"model bytes");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("v1");
        let one = engine_on(&server, &root, data);
        let other = engine_on(&server, &root, data);
        assert_eq!(last(&one.call(request("download", "d")))["type"], "ok");
        assert_eq!(last(&one.call(request("load", "l")))["type"], "ok");

        assert_eq!(last(&other.call(request("delete", "x")))["type"], "ok");
        assert_eq!(listed_state(&other), "not_downloaded");
        assert_eq!(listed_state(&one), "ready", "it keeps its copy");
        let folder = root.join("artifacts").join(&data.artifacts["vad"].digest);
        assert!(folder.join(VAD).is_file());

        // A VAD session keeps the files after the unload.
        let mut worker = StreamWorker::new(Arc::clone(&one.engine));
        let start = json!({ "v": 1, "type": "vad_start", "id": "s", "model": "m", "session": 1 });
        assert_eq!(last(&one.stream(&mut worker, start))["type"], "ok");
        assert_eq!(states(&one.call(request("unload", "u"))), ["not_downloaded"]);
        std::thread::sleep(Duration::from_millis(100));
        assert!(folder.join(VAD).is_file(), "the session still runs");

        let end = json!({ "v": 1, "type": "stream_end", "id": "e", "session": 1 });
        assert_eq!(last(&one.stream(&mut worker, end))["type"], "ok");
        let deadline = Instant::now() + Duration::from_secs(5);
        while folder.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!folder.exists(), "the artifact goes after the last use");
    }

    #[test]
    fn a_model_without_a_descriptor_stays_in_the_models_folder() {
        let server = serve_archives(vec![("a".to_string(), archive("a", &["tokens.txt", "model.onnx"]))]);
        let data = fixture(&server, b"x");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("v1");
        let engine = engine_on(&server, &root, data);
        let request = json!({ "v": 1, "type": "download", "id": "d", "model": "other", "engine": spec_json("sense_voice", "a", None) });
        let messages = engine.call(request);
        assert_eq!(last(&messages)["type"], "ok", "{messages:?}");
        assert!(engine.models_dir().join("other/a").is_dir());
        assert_eq!(std::fs::read_dir(root.join("artifacts")).unwrap().count(), 0);
    }

    #[test]
    fn urls_of_a_descriptor_follow_the_settings() {
        let engine = harness("http://mirror/releases/asr-models");
        let inner = &engine.engine;
        assert_eq!(
            inner.store_url("https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/a.tar.bz2"),
            "http://mirror/releases/speaker-segmentation-models/a.tar.bz2"
        );
        assert_eq!(inner.store_url("http://elsewhere/x"), "http://elsewhere/x");
        assert_eq!(
            inner.store_url("https://huggingface.co/o/r/resolve/c/f"),
            "http://mirror/releases/asr-models/o/r/resolve/c/f"
        );
    }
}
