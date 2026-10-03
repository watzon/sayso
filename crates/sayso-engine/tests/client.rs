//! The real client (`sayso-engine-client`) against the built engine binary.
//! No real models: the archives come from a local HTTP server.

#![cfg(not(target_os = "macos"))]

use sayso_core::models::{self, ModelId};
use sayso_core::stt::{EngineEvent, ModelStatus};
use sayso_engine_client::EngineClient;
use sayso_platform::SttBackend;
use std::path::PathBuf;
use std::time::Duration;

/// A `.tar.bz2` with one top folder and files that look like a streaming model.
fn fake_archive(name: &str) -> Vec<u8> {
    let encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    for file in ["encoder.onnx", "decoder.onnx", "joiner.onnx", "tokens.txt"] {
        let body = b"not a real model";
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("{name}/{file}"), &body[..])
            .unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

#[test]
fn the_client_downloads_and_reports_errors_through_the_engine() {
    let fallback = models::find(&models::preview_fallback()).unwrap();
    let sayso_core::models::EngineKind::Sherpa { archive, .. } = &fallback.engine else {
        panic!("the preview fallback is not a sherpa model");
    };
    let bytes = fake_archive(archive);
    let path = format!("/{archive}.tar.bz2");
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let response = if request.url() == path {
                tiny_http::Response::from_data(bytes.clone())
            } else {
                tiny_http::Response::from_data(Vec::new()).with_status_code(404)
            };
            let _ = request.respond(response);
        }
    });
    // SAFETY: this file has one test, so no other thread reads the environment now.
    // The engine child process inherits the variable.
    unsafe { std::env::set_var("SAYSO_MODELS_URL", format!("http://127.0.0.1:{port}")) };

    let dir = tempfile::tempdir().unwrap();
    let engine = PathBuf::from(env!("CARGO_BIN_EXE_sayso-engine"));
    let client =
        EngineClient::spawn(engine, dir.path().join("models"), dir.path().join("cache")).unwrap();
    let events = client.subscribe();

    // `spawn` said hello and listed the catalog: nothing is on disk.
    for info in client.catalog() {
        assert_eq!(
            client.status(&info.id),
            ModelStatus::NotDownloaded,
            "{}",
            info.id
        );
    }

    // Load before download fails with the engine's message.
    let error = client.load(&fallback.id).unwrap_err().to_string();
    assert!(error.contains("is not downloaded"), "{error}");

    // Download from the local server.
    client.download(&fallback.id).unwrap();
    let mut progress = 0;
    loop {
        match events
            .recv_timeout(Duration::from_secs(30))
            .expect("download finished")
        {
            EngineEvent::ModelStatus { model, status } if model == fallback.id => match status {
                ModelStatus::Downloading { .. } => progress += 1,
                ModelStatus::Downloaded => break,
                other => panic!("unexpected status {other:?}"),
            },
            _ => {}
        }
    }
    assert!(progress > 0);
    assert!(
        dir.path()
            .join("models")
            .join(fallback.id.as_str())
            .join(archive)
            .join("joiner.onnx")
            .is_file()
    );

    // The files are not real models. onnxruntime aborts the engine on such a
    // file, so the load fails and the client starts the engine again.
    let error = client.load(&fallback.id).unwrap_err().to_string();
    assert!(
        error.contains("engine exited") || error.contains("cannot load"),
        "{error}"
    );
    loop {
        if let EngineEvent::Restarted = events
            .recv_timeout(Duration::from_secs(30))
            .expect("engine restarted")
        {
            break;
        }
    }
    assert_eq!(client.status(&fallback.id), ModelStatus::Downloaded);

    // A download that fails shows as Failed with the HTTP error.
    let other = ModelId::new("whisper-tiny");
    client.download(&other).unwrap();
    loop {
        if let EngineEvent::ModelStatus {
            model,
            status: ModelStatus::Failed { message },
        } = events
            .recv_timeout(Duration::from_secs(30))
            .expect("download failed")
            && model == other
        {
            assert!(message.contains("404"), "{message}");
            break;
        }
    }

    // Delete puts the model back to not downloaded.
    client.delete(&fallback.id).unwrap();
    assert_eq!(client.status(&fallback.id), ModelStatus::NotDownloaded);
}
