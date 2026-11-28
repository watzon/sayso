//! The cleanup models without a real model: the catalog and the prompt (C01), the download from a
//! local HTTP server with its hash check and `ready.json` (C02), and `RuntimeCleaner` with a text
//! that splits into parts, over `sayso-fake-cleanup` (C04).
#![cfg(feature = "cleanup")]

mod common;

use sayso_sdk::cleanup::client::cleaner::RuntimeCleaner;
use sayso_sdk::cleanup::client::download::{self, DownloadError};
use sayso_sdk::cleanup::client::runtime::Runtime;
use sayso_sdk::cleanup::models::protocol::Stop;
use sayso_sdk::cleanup::models::{
    self, Cleaner, CleanupModelInfo, CleanupRequest, Format, Layout, Settings, Tone,
};
use sha2::{Digest, Sha256};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn the_catalog_has_two_pinned_models_and_renders_a_prompt() {
    let ids: Vec<_> = models::catalog().iter().map(|m| m.id).collect();
    assert_eq!(ids, ["s1-mini", "spokenly-cleanup-1"]);
    for model in models::catalog() {
        assert_eq!(model.sha256.len(), 64, "{}", model.id);
        assert!(
            model.revision.len() >= 7 && model.file.ends_with(".gguf"),
            "{}",
            model.id
        );
        assert!(
            model.file_url(model.file).contains(model.revision),
            "the URL is pinned to the revision"
        );
    }
    assert_eq!(models::find("s1-mini").unwrap().name, "S1-mini");
    let info = models::find("s1-mini").unwrap();
    let prompt = models::render(
        info,
        "um so hello there",
        Settings {
            tone: Tone::Formal,
            layout: Layout::Paragraphs,
            format: Format::Plain,
        },
        None,
        None,
        5,
    );
    assert!(prompt.text.contains("um so hello there"), "{}", prompt.text);
    assert!(prompt.max_tokens >= 5 && !prompt.end.is_empty());
    assert!(models::is_filler_only("um"));
    assert!(!models::is_filler_only("um hello"));
    assert_eq!(
        models::restore_spelling(
            "push it to GitHub",
            "push it to github",
            &["GitHub".to_string()]
        ),
        "push it to GitHub"
    );
    assert!(
        models::check("hello there", Stop::Limit, " Hello there.").is_err(),
        "an answer that stopped at the limit is not whole"
    );
    passed();
}

fn passed() {
    common::passed("C01");
}

/// A model with the file `bytes` and the hash `sha`, with the repo and the revision of a server.
fn model(bytes: &[u8], sha: &str) -> CleanupModelInfo {
    let mut info = models::find("s1-mini").unwrap().clone();
    info.repo = "test/repo";
    info.revision = "0123456789abcdef";
    info.file = "model.gguf";
    info.size_bytes = bytes.len() as u64;
    info.sha256 = Box::leak(sha.to_string().into_boxed_str());
    info.license_files = &["LICENSE"];
    info
}

fn serve(bytes: &'static [u8]) -> (String, std::sync::Arc<tiny_http::Server>) {
    let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let url = format!("http://{}", server.server_addr().to_ip().unwrap());
    let running = server.clone();
    std::thread::spawn(move || {
        for request in running.incoming_requests() {
            let body: &[u8] = if request.url().ends_with("/model.gguf") {
                bytes
            } else {
                b"license"
            };
            let _ = request.respond(tiny_http::Response::from_data(body));
        }
    });
    (url, server)
}

#[test]
fn a_download_checks_the_hash_and_writes_ready_json_last() {
    const BYTES: &[u8] = b"fake gguf bytes";
    let (endpoint, server) = serve(BYTES);
    let folder = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);

    let good = model(BYTES, &hex(BYTES));
    let mut last = 0;
    let path = download::download(folder.path(), &good, &endpoint, &cancel, &mut |p| {
        last = p.bytes_done
    })
    .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), BYTES);
    assert_eq!(last, BYTES.len() as u64);
    assert!(download::is_ready(folder.path(), &good));
    assert_eq!(download::model_path(folder.path(), &good), Some(path));
    let ready: serde_json::Value = serde_json::from_slice(
        &std::fs::read(download::model_dir(folder.path(), &good).join("ready.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        (ready["revision"].as_str(), ready["sha256"].as_str()),
        (Some("0123456789abcdef"), Some(good.sha256))
    );
    download::delete(folder.path(), &good).unwrap();
    assert!(!download::is_ready(folder.path(), &good));

    // The same size with other bytes: the hash is wrong, and nothing is ready.
    let bad = model(BYTES, &hex(b"other bytes!!!!"));
    let error =
        download::download(folder.path(), &bad, &endpoint, &cancel, &mut |_| {}).unwrap_err();
    assert!(matches!(error, DownloadError::WrongHash { .. }), "{error}");
    assert!(!download::is_ready(folder.path(), &bad));
    assert!(
        !download::model_dir(folder.path(), &bad)
            .join("ready.json")
            .exists()
    );
    server.unblock();
    common::passed("C02");
}

/// Put a model of the catalog on disk the way the download leaves it, without a download.
fn put_ready(models_dir: &std::path::Path, info: &CleanupModelInfo) {
    let dir = download::model_dir(models_dir, info);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(info.file), b"gguf").unwrap();
    std::fs::write(
        dir.join("ready.json"),
        serde_json::json!({"revision": info.revision, "sha256": info.sha256}).to_string(),
    )
    .unwrap();
}

#[test]
fn a_long_text_goes_to_the_runtime_in_parts_and_comes_back_as_one_text() {
    let Some(program) = common::fake_cleanup() else {
        return;
    };
    let folder = tempfile::tempdir().unwrap();
    // The fake runtime answers a prompt of key-value lines (`text: ...`), the format of Cleanup 1.
    let info = models::find("spokenly-cleanup-1").unwrap();
    put_ready(folder.path(), info);
    let cleaner = RuntimeCleaner::new(Runtime::new(program), folder.path().to_path_buf());

    let sentence = "um so we met on monday and we talked about the plan for the release. ";
    let text = sentence.repeat(160);
    let request = CleanupRequest {
        model: info.id.to_string(),
        text: text.clone(),
        settings: Settings::default(),
        app: None,
        site: None,
        timeout: Duration::from_secs(60),
    };
    let response = cleaner.clean(&request).unwrap();
    assert!(
        response.parts >= 2,
        "{} words did not split: {} part",
        text.split_whitespace().count(),
        response.parts
    );
    assert_eq!(response.model, "spokenly-cleanup-1");
    assert!(
        !response.text.contains("um so") && response.text.contains("we met on monday"),
        "{}",
        &response.text[..200.min(response.text.len())]
    );
    assert!(response.tokens_in > 0 && response.tokens_out > 0);

    let short = cleaner
        .clean(&CleanupRequest {
            text: "um hello there".into(),
            ..request.clone()
        })
        .unwrap();
    assert_eq!(
        (short.parts, short.load_ms),
        (1, 0),
        "the model stays loaded"
    );
    assert_eq!(short.text.trim(), "Hello there.");

    let missing = RuntimeCleaner::new(
        Runtime::new("/no/such/runtime".into()),
        tempfile::tempdir().unwrap().path().to_path_buf(),
    );
    assert!(
        matches!(
            missing.clean(&request),
            Err(models::CleanError::NotConfigured(_))
        ),
        "a model that is not downloaded is not configured"
    );
    common::passed("C04");
}
