//! The conformance suite (`sayso-engine-conformance`) against `sayso-fake-engine`, so
//! the tests of the client run against an engine that keeps the protocol.

use sayso_engine_conformance::{Model, Ready, Suite, run, write_wav};
use serde_json::json;

#[test]
fn the_fake_engine_passes_the_core_and_ready_levels() {
    let models = tempfile::tempdir().unwrap();
    // The fake engine reports a model as downloaded when this file is there.
    std::fs::write(models.path().join("downloaded-conformance-model"), "").unwrap();
    let audio = models.path().join("speech.wav");
    write_wav(&audio, &vec![0; 16_000]).unwrap();
    let model = Model {
        id: "conformance-model".into(),
        engine: json!({"kind": "whisper", "variant": "base"}),
    };
    let mut suite = Suite::new(env!("CARGO_BIN_EXE_sayso-fake-engine"));
    suite.ready = Some(Ready {
        models_dir: models.path().to_path_buf(),
        model,
        audio,
        stream: true,
        vad: None,
        diarization: None,
    });
    let report = run(&suite);
    assert!(report.passed(), "{report}");
    assert!(report.outcomes.len() > 15, "the ready level ran: {report}");
}
