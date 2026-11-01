//! The whole path with the real runtime and the real model files: download
//! folder, runtime, `RuntimeCleaner`, parts, and checks. Ignored by default:
//!
//! ```text
//! cd native/cleanup && cargo build --release && cd -
//! SAYSO_CLEANUP_MODELS=<folder with s1-mini-q4_k_m.gguf and cleanup-1-q8_0.gguf> \
//!   cargo test -p sayso-cleanup-client --test real_models -- --ignored --nocapture
//! ```
//!
//! The test puts the files in a models folder of its own, as a finished
//! download would, with links to the files in `SAYSO_CLEANUP_MODELS`.

use sayso_cleanup_client::cleaner::RuntimeCleaner;
use sayso_cleanup_client::download;
use sayso_cleanup_client::runtime::Runtime;
use sayso_cleanup_models::{Cleaner, CleanupRequest, Format, Settings, Tone};
use std::path::Path;
use std::time::Duration;

/// Make `models_dir` look like a finished download of `id`, with a link to the real file.
fn install(models_dir: &Path, source: &Path, id: &str) {
    let info = sayso_cleanup_models::find(id).unwrap();
    let dir = download::model_dir(models_dir, info);
    std::fs::create_dir_all(&dir).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(source.join(info.file), dir.join(info.file)).unwrap();
    #[cfg(not(unix))]
    std::fs::copy(source.join(info.file), dir.join(info.file)).unwrap();
    let ready =
        serde_json::json!({ "revision": info.revision, "sha256": info.sha256, "file": info.file });
    std::fs::write(dir.join("ready.json"), ready.to_string()).unwrap();
    assert!(
        download::is_ready(models_dir, info),
        "the test folder must look ready to the downloader"
    );
}

fn request(model: &str, text: &str, settings: Settings) -> CleanupRequest {
    CleanupRequest {
        model: model.into(),
        text: text.into(),
        settings,
        app: Some("Slack".into()),
        site: None,
        timeout: Duration::from_secs(10),
    }
}

#[test]
#[ignore = "needs the release runtime and the model files (SAYSO_CLEANUP_MODELS)"]
fn both_models_clean_up_through_the_runtime() {
    let source = std::env::var_os("SAYSO_CLEANUP_MODELS").expect("SAYSO_CLEANUP_MODELS");
    let source = Path::new(&source);
    let models = tempfile::tempdir().unwrap();
    install(models.path(), source, "s1-mini");
    install(models.path(), source, "spokenly-cleanup-1");
    let cleaner = RuntimeCleaner::new(
        Runtime::new(Runtime::default_path()),
        models.path().to_path_buf(),
    );

    let text = "um so the meeting is on tuesday no wait wednesday at three thirty";
    let s1 = cleaner
        .clean(&request("s1-mini", text, Settings::default()))
        .unwrap();
    println!(
        "s1-mini: {:?} ({} ms, load {} ms)",
        s1.text, s1.elapsed_ms, s1.load_ms
    );
    assert_eq!(s1.text, "So the meeting is on Wednesday at 3:30.");

    let email = cleaner
        .clean(&request(
            "s1-mini",
            "hi dana can you send me the q three report by friday thanks",
            Settings {
                format: Format::Email,
                ..Default::default()
            },
        ))
        .unwrap();
    println!("s1-mini email: {:?}", email.text);
    assert!(
        email.text.contains("Dana") && email.text.contains('\n'),
        "{:?}",
        email.text
    );

    let spokenly = cleaner
        .clean(&request(
            "spokenly-cleanup-1",
            text,
            Settings {
                tone: Tone::Casual,
                ..Default::default()
            },
        ))
        .unwrap();
    println!(
        "cleanup-1: {:?} ({} ms, load {} ms)",
        spokenly.text, spokenly.elapsed_ms, spokenly.load_ms
    );
    assert_eq!(spokenly.text, "The meeting is on Wednesday at 3:30");

    // Only filler words: an empty answer is right.
    let filler = cleaner
        .clean(&request("s1-mini", "um uh you know", Settings::default()))
        .unwrap();
    assert_eq!(filler.text, "");

    // A long text goes in parts and comes back whole. Every sentence is
    // different: a small model joins sentences that repeat.
    let colors = [
        "red", "blue", "green", "yellow", "black", "white", "orange", "purple",
    ];
    let long: String = (1..=120)
        .map(|n| {
            format!(
                "um so box number {n} is the {} one and it goes on shelf {} uh next to the lamp. ",
                colors[n % colors.len()],
                n * 7
            )
        })
        .collect();
    let out = cleaner
        .clean(&request("s1-mini", &long, Settings::default()))
        .unwrap();
    println!(
        "long: {} parts, {} chars in, {} chars out, {} ms",
        out.parts,
        long.len(),
        out.text.len(),
        out.elapsed_ms
    );
    assert!(out.parts > 1, "the text should split");
    let found = (1..=120)
        .filter(|n| out.text.contains(&format!("shelf {}", n * 7)))
        .count();
    assert!(found >= 110, "only {found} of 120 sentences came back");

    // The same sentence many times makes S1-mini repeat until its limit. The
    // check stops that answer, and the caller keeps the text.
    let looped =
        "so um the plan for next week is that we finish the import screen on monday. ".repeat(30);
    let error = cleaner.clean(&request("s1-mini", &looped, Settings::default()));
    println!("looped: {error:?}");
    assert!(
        error.is_err()
            || error
                .as_ref()
                .is_ok_and(|r| r.text.len() < looped.len() * 2),
        "a loop must not get through"
    );
}
