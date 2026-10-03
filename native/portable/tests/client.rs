//! The built `SaysoEngine` through the app's own client, `EngineClient`, with
//! no models on disk.

use sayso_core::models::{self, ModelId};
use sayso_core::stt::{ModelStatus, SessionOptions};
use sayso_engine_client::EngineClient;
use sayso_platform::SttBackend;
use std::path::PathBuf;

fn client(dir: &tempfile::TempDir) -> EngineClient {
    EngineClient::spawn(
        PathBuf::from(env!("CARGO_BIN_EXE_SaysoEngine")),
        dir.path().join("models"),
        dir.path().join("cache"),
    )
    .expect("hello and list_models succeed")
}

#[test]
fn spawn_lists_every_catalog_model_as_not_downloaded() {
    let dir = tempfile::tempdir().unwrap();
    let client = client(&dir);
    let catalog = models::catalog();
    assert!(!catalog.is_empty());
    for info in &catalog {
        assert_eq!(
            client.status(&info.id),
            ModelStatus::NotDownloaded,
            "{}",
            info.id
        );
    }
}

#[test]
fn calls_without_models_fail_with_clear_errors() {
    let dir = tempfile::tempdir().unwrap();
    let client = client(&dir);
    let parakeet = models::default_model();

    let error = client.load(&parakeet).unwrap_err().to_string();
    assert!(error.contains("is not downloaded"), "{error}");

    let options = SessionOptions {
        final_model: parakeet.clone(),
        preview_model: Some(parakeet.clone()),
        language: "en".into(),
        vocabulary: vec!["Sayso".into()],
    };
    let error = client
        .transcribe(&vec![0.0; 16_000], &options)
        .unwrap_err()
        .to_string();
    assert!(error.contains("is not downloaded"), "{error}");
    let error = client.start_stream(1, &options).unwrap_err().to_string();
    assert!(error.contains("is not downloaded"), "{error}");
    // Audio for a session that never started does not break anything.
    client.push_audio(1, &vec![0.0; 2560]).unwrap();
    client.end_stream(1);

    let error = client
        .load(&ModelId::new("no-such-model"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("unknown model"), "{error}");

    client.delete(&parakeet).unwrap();
    assert_eq!(client.status(&parakeet), ModelStatus::NotDownloaded);
    // The engine still answers.
    client.delete(&parakeet).unwrap();
}
