//! The `engine` field of a request: how to download and load one model.
//!
//! The field is `EngineKind` from `sayso-core`, serialized. This engine runs
//! only the `sherpa` kind.

use anyhow::{Result, anyhow, bail};
use sayso_core::models::EngineKind;
use serde_json::Value;

/// How the engine loads the files of an archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recipe {
    /// NeMo Parakeet TDT: encoder, decoder, and joiner (transducer).
    ParakeetTdt,
    /// NeMo Parakeet TDT-CTC: the CTC head, one model file.
    ParakeetTdtCtc,
    /// A streaming zipformer transducer. Live preview only.
    ZipformerStreaming,
    Whisper,
    SenseVoice,
    Moonshine,
}

impl Recipe {
    pub fn parse(name: &str) -> Result<Recipe> {
        Ok(match name {
            "parakeet_tdt" => Recipe::ParakeetTdt,
            "parakeet_tdt_ctc" => Recipe::ParakeetTdtCtc,
            "zipformer_streaming" => Recipe::ZipformerStreaming,
            "whisper" => Recipe::Whisper,
            "sense_voice" => Recipe::SenseVoice,
            "moonshine" => Recipe::Moonshine,
            other => bail!("unknown sherpa recipe {other}"),
        })
    }

    /// True when the main archive can do the final pass.
    pub fn final_pass(self) -> bool {
        self != Recipe::ZipformerStreaming
    }

    /// The longest piece of audio that one decode gets, in seconds. Longer
    /// recordings are cut at quiet points. Whisper reads 30 second windows.
    /// Moonshine (sherpa-onnx 1.13.8) returns no text for 10 seconds or more.
    pub fn max_chunk_seconds(self) -> usize {
        match self {
            Recipe::Whisper => 28,
            Recipe::SenseVoice => 25,
            Recipe::Moonshine => 7,
            Recipe::ParakeetTdt | Recipe::ParakeetTdtCtc => 60,
            Recipe::ZipformerStreaming => usize::MAX,
        }
    }
}

/// One model as the engine sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    pub recipe: Recipe,
    /// Archive name in the sherpa-onnx `asr-models` release, without `.tar.bz2`.
    pub archive: String,
    /// The streaming archive that gives the live preview, if the main one cannot.
    pub preview_archive: Option<String>,
}

impl ModelSpec {
    /// Parse the `engine` object of a request.
    pub fn from_json(value: &Value) -> Result<ModelSpec> {
        let kind: EngineKind = serde_json::from_value(value.clone())
            .map_err(|e| anyhow!("invalid engine field: {e}"))?;
        let EngineKind::Sherpa {
            recipe,
            archive,
            preview_archive,
        } = kind
        else {
            let name = value.get("kind").and_then(Value::as_str).unwrap_or("?");
            bail!("engine kind {name} does not run in this engine");
        };
        check_archive_name(&archive)?;
        if let Some(preview) = &preview_archive {
            check_archive_name(preview)?;
        }
        Ok(ModelSpec {
            recipe: Recipe::parse(&recipe)?,
            archive,
            preview_archive,
        })
    }

    /// Every archive to download, main archive first.
    pub fn archives(&self) -> Vec<&str> {
        let mut archives = vec![self.archive.as_str()];
        if let Some(preview) = self
            .preview_archive
            .as_deref()
            .filter(|p| *p != self.archive)
        {
            archives.push(preview);
        }
        archives
    }

    /// The archive that streams the live preview, if the model has one.
    pub fn stream_archive(&self) -> Option<&str> {
        if self.recipe == Recipe::ZipformerStreaming {
            Some(&self.archive)
        } else {
            self.preview_archive.as_deref()
        }
    }
}

/// An archive name becomes a URL and a folder name, so it must be one plain path part.
fn check_archive_name(name: &str) -> Result<()> {
    let plain = !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if plain {
        Ok(())
    } else {
        bail!("invalid archive name {name:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_a_sherpa_engine() {
        let spec = ModelSpec::from_json(&json!({
            "kind": "sherpa", "recipe": "parakeet_tdt", "archive": "a-int8", "preview_archive": "p"
        }))
        .unwrap();
        assert_eq!(spec.recipe, Recipe::ParakeetTdt);
        assert_eq!(spec.archives(), vec!["a-int8", "p"]);
        assert_eq!(spec.stream_archive(), Some("p"));
        assert!(spec.recipe.final_pass());

        let stream = ModelSpec::from_json(&json!({
            "kind": "sherpa", "recipe": "zipformer_streaming", "archive": "z", "preview_archive": null
        }))
        .unwrap();
        assert_eq!(stream.archives(), vec!["z"]);
        assert_eq!(stream.stream_archive(), Some("z"));
        assert!(!stream.recipe.final_pass());
    }

    #[test]
    fn the_catalog_serializes_to_specs_this_engine_reads() {
        for info in sayso_core::models_onnx::catalog() {
            let value = serde_json::to_value(&info.engine).unwrap();
            let spec = ModelSpec::from_json(&value).unwrap_or_else(|e| panic!("{}: {e}", info.id));
            assert_eq!(
                spec.stream_archive().is_some(),
                info.live_preview,
                "{}",
                info.id
            );
            assert_eq!(spec.recipe.final_pass(), info.final_pass, "{}", info.id);
        }
    }

    #[test]
    fn rejects_other_kinds_and_unsafe_names() {
        let error =
            ModelSpec::from_json(&json!({"kind": "whisper", "variant": "tiny"})).unwrap_err();
        assert_eq!(
            error.to_string(),
            "engine kind whisper does not run in this engine"
        );
        assert!(ModelSpec::from_json(&json!({"kind": "nope"})).is_err());
        for bad in ["../x", "a/b", ".hidden", ""] {
            let value = json!({"kind": "sherpa", "recipe": "whisper", "archive": bad, "preview_archive": null});
            assert!(ModelSpec::from_json(&value).is_err(), "{bad}");
        }
        let value =
            json!({"kind": "sherpa", "recipe": "nope", "archive": "a", "preview_archive": null});
        assert_eq!(
            ModelSpec::from_json(&value).unwrap_err().to_string(),
            "unknown sherpa recipe nope"
        );
    }
}
