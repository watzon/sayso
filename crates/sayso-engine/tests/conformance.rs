//! The conformance suite (`sayso-engine-conformance`): the core level, and the
//! model level with the Parakeet TDT v3 entry of the catalog. The ready level
//! needs the model on disk; `crates/sayso-engine/NOTES.md` says how to run it.

#![cfg(not(target_os = "macos"))]

use sayso_engine_conformance::{Model, Suite, run};

#[test]
fn the_engine_passes_the_core_and_model_levels() {
    let entry = sayso_core::models_onnx::catalog()
        .into_iter()
        .find(|m| m.id.as_str() == "parakeet-tdt-v3")
        .expect("parakeet-tdt-v3 is in the catalog");
    let mut suite = Suite::new(env!("CARGO_BIN_EXE_sayso-engine"));
    suite.model = Some(Model {
        id: entry.id.as_str().to_string(),
        engine: serde_json::to_value(&entry.engine).unwrap(),
    });
    let report = run(&suite);
    assert!(report.passed(), "{report}");
}
