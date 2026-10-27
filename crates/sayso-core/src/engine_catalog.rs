//! The built-in catalog of each engine (`docs/engine-protocol.md`, "Models").
//!
//! The engines run the models of the Sayso catalog by id, so a client that
//! is not Sayso gets the same models. The Rust engine builds its catalog with
//! [`json`] when it starts. The Swift engine cannot call Rust, so it gets the
//! file `Catalog.generated.swift`, and a test checks that the file is the
//! same as [`json`] (see [`swift_source`]).

use crate::models::{self, ModelInfo, ModelTask};
use crate::models_onnx::{self, System};
use serde_json::{Value, json};

/// The engine that a catalog is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// The Swift engine on macOS.
    Swift,
    /// The Rust engine `sayso-engine` on Linux or Windows.
    Rust(System),
}

/// A model id that was taken out of the catalog. Its id stays public, so a
/// request with it gets a clear error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retired {
    pub id: &'static str,
    /// What to use instead, in English.
    pub message: &'static str,
}

/// The ids that were taken out of the catalogs. Add an id here when a model
/// goes out of a catalog, and never remove one.
pub const RETIRED: &[Retired] = &[];

/// The models of the catalog of `engine`, in display order.
pub fn models(engine: Engine) -> Vec<ModelInfo> {
    match engine {
        Engine::Swift => models::apple_catalog(),
        Engine::Rust(system) => models_onnx::catalog_for(system),
    }
}

/// The catalog of `engine`: `{models, retired}`, as `list_models` without
/// `models` gives it. The entries of the Swift engine also have `min_macos`,
/// which the engine uses to leave out a model that this Mac cannot run.
pub fn json(engine: Engine) -> Value {
    let models: Vec<Value> = models(engine)
        .iter()
        .map(|m| {
            let mut entry = entry(m);
            if engine == Engine::Swift {
                entry["min_macos"] = json!(m.min_macos);
            }
            entry
        })
        .collect();
    let retired: Vec<Value> = RETIRED.iter().map(|r| json!({ "id": r.id, "message": r.message })).collect();
    json!({ "models": models, "retired": retired })
}

/// The roles of a model in the protocol: `final`, `streaming`, `vad`,
/// `diarization`. The Host API gives the same names in `list_models`.
pub fn roles(m: &ModelInfo) -> Vec<&'static str> {
    let mut roles = Vec::new();
    if m.final_pass {
        roles.push("final");
    }
    if m.live_preview {
        roles.push("streaming");
    }
    match m.task() {
        ModelTask::Speech => {}
        ModelTask::Vad => roles.push("vad"),
        ModelTask::Diarization => roles.push("diarization"),
    }
    roles
}

/// One model of the catalog, as the protocol gives it.
fn entry(m: &ModelInfo) -> Value {
    json!({
        "id": m.id,
        "name": m.name,
        "engine": m.engine,
        "size_bytes": m.size_bytes,
        "languages": m.languages,
        "roles": roles(m),
        "timestamps": m.engine.timestamps(),
        "detects_language": m.detects_language(),
        "recommended": m.recommended,
    })
}

/// The Swift source that holds the catalog of the Swift engine.
pub fn swift_source() -> String {
    let json = serde_json::to_string_pretty(&sorted(json(Engine::Swift))).expect("the catalog is JSON");
    format!(
        "// Generated from crates/sayso-core/src/engine_catalog.rs. Do not edit.\n\
         // Make it again with: SAYSO_WRITE_CATALOG=1 cargo test -p sayso-core engine_catalog\n\
         \n\
         /// The built-in catalog of the engine (`docs/engine-protocol.md`, \"Models\").\n\
         enum BuiltInCatalog {{\n    static let json = #\"\"\"\n{json}\n\"\"\"#\n}}\n"
    )
}

/// `value` with the keys of each object in order. A build of the whole workspace
/// turns on the feature `preserve_order` of serde_json, which keeps the keys in
/// the order of insertion. Sorted keys give the same file in each build.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(key, value)| (key, sorted(value))).collect())
        }
        Value::Array(list) => Value::Array(list.into_iter().map(sorted).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_swift_catalog_file_is_up_to_date() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../native/macos/SaysoEngine/Sources/SaysoEngine/Catalog.generated.swift");
        let want = swift_source();
        if std::env::var_os("SAYSO_WRITE_CATALOG").is_some() {
            std::fs::write(&path, &want).unwrap();
        }
        // Git on Windows can check the file out with CRLF line ends.
        let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
        assert!(
            have == want,
            "{} is out of date; run: SAYSO_WRITE_CATALOG=1 cargo test -p sayso-core engine_catalog",
            path.display()
        );
    }

    /// The Swift engine cannot read `Cargo.toml`, so its version is a constant
    /// in `Protocol.swift`. The engine release file has the Sayso version, and
    /// `hello` must say the same (docs/engine-protocol.md, "hello").
    #[test]
    fn the_swift_engine_reports_the_sayso_version() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../native/macos/SaysoEngine/Sources/SaysoEngine/Protocol.swift");
        let source = std::fs::read_to_string(&path).unwrap_or_else(|why| panic!("cannot read {}: {why}", path.display()));
        let have = source
            .lines()
            .find_map(|line| line.strip_prefix("let engineVersion = \"")?.strip_suffix('"'))
            .unwrap_or_else(|| panic!("{} has no line: let engineVersion = \"...\"", path.display()));
        let want = env!("CARGO_PKG_VERSION");
        assert!(
            have == want,
            "engineVersion in {} is {have}, but the Sayso version is {want}; set it to \"{want}\" (docs/releasing.md, \"Make a release\")",
            path.display()
        );
    }

    #[test]
    fn each_catalog_has_unique_ids_and_no_retired_id() {
        for engine in [Engine::Swift, Engine::Rust(System::Linux), Engine::Rust(System::Windows)] {
            let catalog = json(engine);
            let ids: Vec<&str> = catalog["models"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
            let mut unique = ids.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), ids.len(), "{engine:?}");
            assert!(!ids.is_empty(), "{engine:?}");
            for retired in RETIRED {
                assert!(!ids.contains(&retired.id), "{} is retired and in the catalog of {engine:?}", retired.id);
            }
        }
    }

    #[test]
    fn an_entry_has_the_fields_of_the_protocol() {
        let catalog = json(Engine::Rust(System::Windows));
        let v3 = catalog["models"].as_array().unwrap().iter().find(|m| m["id"] == "parakeet-tdt-v3").unwrap();
        assert_eq!(v3["engine"]["kind"], "sherpa");
        assert_eq!(v3["roles"], json!(["final", "streaming"]));
        assert_eq!(v3["timestamps"], "word");
        assert_eq!(v3["recommended"], true);
        assert!(v3["size_bytes"].as_u64().unwrap() > 0);
        assert!(v3.get("min_macos").is_none());
        assert_eq!(catalog["retired"], json!([]));
    }

    /// `detects_language` is true for Whisper and SenseVoice with more than one
    /// language, on each engine (docs/engine-protocol.md, "Detected language").
    #[test]
    fn whisper_and_sense_voice_detect_the_language() {
        for engine in [Engine::Swift, Engine::Rust(System::Linux), Engine::Rust(System::Windows)] {
            let catalog = json(engine);
            let models = catalog["models"].as_array().unwrap();
            let detects = |id: &str| models.iter().find(|m| m["id"] == id).unwrap_or_else(|| panic!("{id} in {engine:?}"))["detects_language"].clone();
            assert_eq!(detects("whisper-small"), true, "{engine:?}");
            assert_eq!(detects("silero-vad"), false, "{engine:?}");
            for m in models {
                let multilingual = m["languages"].as_array().unwrap().len() > 1;
                assert!(m["detects_language"] == false || multilingual, "{} has one language, in {engine:?}", m["id"]);
            }
        }
        let windows = json(Engine::Rust(System::Windows));
        let v3 = windows["models"].as_array().unwrap().iter().find(|m| m["id"] == "parakeet-tdt-v3").unwrap();
        assert_eq!(v3["detects_language"], false, "Parakeet does not report the language");
        // sherpa-onnx gives `yue` for each language with this SenseVoice export (EngineKind::detects_language).
        for engine in [Engine::Rust(System::Linux), Engine::Rust(System::Windows)] {
            let catalog = json(engine);
            let sense_voice = catalog["models"].as_array().unwrap().iter().find(|m| m["id"] == "sensevoice-small").unwrap();
            assert_eq!(sense_voice["detects_language"], false, "{engine:?}");
        }
    }

    /// Each engine gets the VAD model and the diarization model with the engine
    /// object that it runs, and with the role of the model only.
    #[test]
    fn vad_and_diarization_entries_have_their_role_and_engine() {
        let find = |engine: Engine, id: &str| {
            json(engine)["models"].as_array().unwrap().iter().find(|m| m["id"] == id).cloned().unwrap_or_else(|| panic!("{id} in {engine:?}"))
        };
        let swift_vad = find(Engine::Swift, "silero-vad");
        assert_eq!(swift_vad["engine"], json!({"kind": "fluid_vad"}));
        assert_eq!(swift_vad["roles"], json!(["vad"]));
        assert_eq!(swift_vad["size_bytes"], 1_063_427);
        let swift_diarization = find(Engine::Swift, "speaker-diarization");
        assert_eq!(swift_diarization["engine"], json!({"kind": "fluid_diarization"}));
        assert_eq!(swift_diarization["roles"], json!(["diarization"]));
        assert_eq!(swift_diarization["size_bytes"], 21_787_007);
        for system in [System::Linux, System::Windows] {
            let vad = find(Engine::Rust(system), "silero-vad");
            assert_eq!(vad["engine"], json!({"kind": "sherpa_vad", "recipe": "silero", "file": "silero_vad.onnx"}));
            assert_eq!(vad["roles"], json!(["vad"]));
            assert_eq!(vad["size_bytes"], 643_854);
            assert_eq!(vad["timestamps"], "none");
            let diarization = find(Engine::Rust(system), "speaker-diarization");
            assert_eq!(
                diarization["engine"],
                json!({"kind": "sherpa_diarization", "segmentation": "sherpa-onnx-pyannote-segmentation-3-0",
                       "embedding": models_onnx::SPEAKER_EMBEDDING.0})
            );
            assert_eq!(diarization["roles"], json!(["diarization"]));
            assert_eq!(diarization["size_bytes"], 6_958_444 + models_onnx::SPEAKER_EMBEDDING.1);
            assert!(find(Engine::Rust(system), "silero-vad")["languages"].as_array().unwrap().len() > 50, "all languages");
        }
    }
}
