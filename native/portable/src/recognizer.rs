//! The speech models, through transcribe-rs: Parakeet (ONNX Runtime) and
//! Whisper (whisper.cpp). Everything else in the engine sees them as a
//! [`Recognizer`], so tests can swap in a fake one.

use crate::audio::{self, SAMPLE_RATE, join_texts};
use crate::protocol::Result;
use crate::store::Family;
use sayso_core::models::EngineKind;
use std::path::Path;

/// A loaded model. One call at a time (`&mut self`); the engine keeps each
/// model behind a mutex.
pub trait Recognizer: Send {
    /// Text of 16 kHz mono samples. `language` is a code such as `en`, or
    /// `auto`. `vocabulary` holds the dictionary words.
    fn transcribe(
        &mut self,
        samples: &[f32],
        language: &str,
        vocabulary: &[String],
    ) -> Result<String>;
}

/// Load the model files in `folder`.
pub fn load(folder: &Path, engine: &EngineKind) -> Result<Box<dyn Recognizer>> {
    match (Family::of(engine)?, engine) {
        (Family::Parakeet, _) => Parakeet::load(folder).map(|m| Box::new(m) as Box<dyn Recognizer>),
        (Family::Whisper, EngineKind::WhisperCpp { file }) => {
            Whisper::load(&folder.join(file)).map(|m| Box::new(m) as Box<dyn Recognizer>)
        }
        (Family::Whisper, _) => unreachable!("Family::of checked the kind"),
    }
}

/// NVIDIA Parakeet TDT 0.6B, int8 ONNX export.
pub struct Parakeet {
    model: transcribe_rs::onnx::parakeet::ParakeetModel,
}

impl Parakeet {
    /// The encoder attends to the whole input, so memory grows with the square
    /// of its length. A longer recording goes in pieces, cut at quiet points.
    const MAX_PIECE: usize = 90 * SAMPLE_RATE;
    const CUT_SEARCH: usize = 10 * SAMPLE_RATE;

    pub fn load(folder: &Path) -> Result<Parakeet> {
        use transcribe_rs::onnx::Quantization;
        use transcribe_rs::onnx::parakeet::ParakeetModel;
        ParakeetModel::load(folder, &Quantization::Int8)
            .map(|model| Parakeet { model })
            .map_err(|e| format!("cannot load Parakeet from {}: {e}", folder.display()))
    }
}

impl Recognizer for Parakeet {
    /// Parakeet v3 detects the language itself, and v2 is English only, so the
    /// language is not used. The catalog marks Parakeet without vocabulary.
    fn transcribe(
        &mut self,
        samples: &[f32],
        _language: &str,
        _vocabulary: &[String],
    ) -> Result<String> {
        use transcribe_rs::onnx::parakeet::ParakeetParams;
        let mut texts = Vec::new();
        for piece in audio::split_at_quiet_points(samples, Self::MAX_PIECE, Self::CUT_SEARCH) {
            let result = self
                .model
                .transcribe_with(&samples[piece], &ParakeetParams::default())
                .map_err(|e| format!("Parakeet failed: {e}"))?;
            texts.push(result.text);
        }
        Ok(join_texts(texts.iter().map(String::as_str)))
    }
}

/// OpenAI Whisper, one GGML file through whisper.cpp.
pub struct Whisper {
    engine: transcribe_rs::whisper_cpp::WhisperEngine,
    threads: i32,
}

impl Whisper {
    /// whisper.cpp ignores input shorter than one second.
    const MIN_SAMPLES: usize = SAMPLE_RATE + SAMPLE_RATE / 4;
    /// Far below the noise floor of a microphone (about 0.003 to 0.01).
    const SILENCE_RMS: f32 = 0.0005;

    pub fn load(file: &Path) -> Result<Whisper> {
        use transcribe_rs::whisper_cpp::{WhisperEngine, WhisperLoadParams};
        if file.to_str().is_none() {
            // transcribe-rs passes the path to C as UTF-8.
            return Err(format!(
                "the model path {} is not valid Unicode",
                file.display()
            ));
        }
        let params = WhisperLoadParams {
            use_gpu: transcribe_rs::get_whisper_accelerator().use_gpu(),
            ..WhisperLoadParams::default()
        };
        let engine = WhisperEngine::load_with_params(file, params)
            .map_err(|e| format!("cannot load Whisper from {}: {e}", file.display()))?;
        // whisper.cpp uses at most 4 threads by default. Up to 8 is faster on a
        // desktop processor.
        let threads =
            std::thread::available_parallelism().map_or(4, |n| n.get().clamp(1, 8)) as i32;
        Ok(Whisper { engine, threads })
    }
}

impl Recognizer for Whisper {
    fn transcribe(
        &mut self,
        samples: &[f32],
        language: &str,
        vocabulary: &[String],
    ) -> Result<String> {
        use transcribe_rs::whisper_cpp::WhisperInferenceParams;
        // Whisper makes up words ("You") for silence. Near digital silence has
        // no speech, so it gets no text.
        if audio::rms(samples) < Self::SILENCE_RMS {
            return Ok(String::new());
        }
        let padded: Vec<f32>;
        let input = if samples.len() < Self::MIN_SAMPLES {
            let mut longer = samples.to_vec();
            longer.resize(Self::MIN_SAMPLES, 0.0);
            padded = longer;
            &padded
        } else {
            samples
        };
        let params = WhisperInferenceParams {
            language: whisper_language(language),
            initial_prompt: whisper_prompt(vocabulary),
            n_threads: self.threads,
            ..WhisperInferenceParams::default()
        };
        let result = self
            .engine
            .transcribe_with(input, &params)
            .map_err(|e| format!("Whisper failed: {e}"))?;
        Ok(clean_whisper_text(&result.text))
    }
}

/// `auto` (or nothing) lets Whisper detect the language.
fn whisper_language(language: &str) -> Option<String> {
    let language = language.trim();
    (!language.is_empty() && language != "auto").then(|| language.to_string())
}

/// Dictionary words go in as the initial prompt, like the WhisperKit prompt
/// tokens on macOS. Whisper has no hard boosting.
fn whisper_prompt(vocabulary: &[String]) -> Option<String> {
    let words: Vec<&str> = vocabulary
        .iter()
        .map(|w| w.trim())
        .filter(|w| !w.is_empty())
        .collect();
    (!words.is_empty()).then(|| words.join(", "))
}

/// whisper.cpp writes `[BLANK_AUDIO]` and similar tags for silence.
fn clean_whisper_text(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    let mut tag = String::new();
    for c in text.chars() {
        match c {
            '[' => {
                depth += 1;
                tag.clear();
            }
            ']' if depth > 0 => {
                depth -= 1;
                let known = tag
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == '_' || c == ' ');
                if !known {
                    out.push('[');
                    out.push_str(&tag);
                    out.push(']');
                }
            }
            _ if depth > 0 => tag.push(c),
            _ => out.push(c),
        }
    }
    if depth > 0 {
        out.push('[');
        out.push_str(&tag);
    }
    join_texts(out.split_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whisper_options() {
        assert_eq!(whisper_language("auto"), None);
        assert_eq!(whisper_language(""), None);
        assert_eq!(whisper_language("de"), Some("de".into()));
        assert_eq!(whisper_prompt(&[]), None);
        assert_eq!(
            whisper_prompt(&["Sayso".into(), " ".into(), "Kubernetes".into()]),
            Some("Sayso, Kubernetes".into())
        );
    }

    #[test]
    fn whisper_tags_go_away() {
        assert_eq!(clean_whisper_text(" [BLANK_AUDIO]"), "");
        assert_eq!(clean_whisper_text("Hello [MUSIC] world."), "Hello world.");
        assert_eq!(clean_whisper_text("See [note 1] here"), "See [note 1] here");
        assert_eq!(clean_whisper_text("open [bracket"), "open [bracket");
    }

    #[test]
    fn unsupported_engines_do_not_load() {
        let error = load(Path::new("."), &EngineKind::AppleSpeech)
            .err()
            .unwrap();
        assert!(error.contains("apple_speech"), "{error}");
    }

    #[test]
    fn missing_files_are_an_error_not_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let engine = EngineKind::WhisperCpp {
            file: "ggml-tiny.bin".into(),
        };
        assert!(load(dir.path(), &engine).is_err());
    }
}
