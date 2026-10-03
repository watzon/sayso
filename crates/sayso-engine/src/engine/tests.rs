//! Engine tests with a fake recognizer and a local HTTP server for the archives.

use super::*;
use crate::partials::Segment;
use crate::protocol::tests::Captured;
use crate::recognizer::FinalPass;
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
        })
    }
}

/// Says how many samples it heard and in which language.
struct FakeFinal;

impl FinalPass for FakeFinal {
    fn transcribe(&self, samples: &[f32], language: &str) -> Result<String> {
        Ok(format!("{} samples in {language}", samples.len()))
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

/// Serves `/files/<name>.tar.bz2`. `/redirect/<name>` redirects there, as GitHub does.
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
            let found = archives
                .iter()
                .find(|(name, _)| url == format!("/files/{name}.tar.bz2"))
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
    let dir = tempfile::tempdir().unwrap();
    let captured = Captured::default();
    let out = Arc::new(Output::new(Box::new(captured.clone())));
    let loader = Arc::new(FakeLoader {
        loads: AtomicUsize::new(0),
        final_loads: AtomicUsize::new(0),
    });
    let config = Config {
        models_dir: dir.path().join("models"),
        base_url: base_url.to_string(),
        decode_interval: Duration::ZERO,
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
fn hello_and_unknown_requests() {
    let h = harness("http://unused");
    let reply = h.call(json!({"v": 1, "type": "hello", "id": "r1"}));
    assert_eq!(reply[0]["type"], "hello");
    assert_eq!(reply[0]["id"], "r1");
    assert_eq!(reply[0]["protocol"], 1);

    let reply = h.call(json!({"v": 1, "type": "fly", "id": "r2"}));
    assert_eq!(
        reply[0],
        json!({"v": 1, "type": "error", "id": "r2", "message": "unknown request type fly"})
    );

    let reply = h.call(json!({"v": 1, "type": "load", "id": "r3", "model": "m"}));
    assert_eq!(reply[0]["message"], "missing field engine");
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
    assert_eq!(reply[0]["text"], "16000 samples in en");
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
fn a_failed_download_reports_failed_and_leaves_nothing_complete() {
    let server = serve_archives(vec![]);
    let h = harness(&server.base);
    let engine = spec_json("whisper", "sherpa-onnx-missing", None);
    let reply =
        h.call(json!({"v": 1, "type": "download", "id": "d1", "model": "m", "engine": engine}));
    assert_eq!(states(&reply), ["downloading", "failed"]);
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
