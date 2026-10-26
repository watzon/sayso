//! The final pass of `whisper_cpp` models: one Whisper GGML file on whisper.cpp,
//! through transcribe-rs, on the CPU. Windows uses it: whisper.cpp is faster
//! there than the Whisper exports of sherpa-onnx, and it reads the files that
//! the Windows engine before 0.12 downloaded. On Linux it builds only with the
//! `whisper-cpp` feature, for tests.

// Without whisper.cpp only the tests use the text helpers.
#![cfg_attr(not(any(windows, feature = "whisper-cpp")), allow(dead_code))]

use crate::audio::rms;
use crate::recognizer::FinalPass;
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

#[cfg(not(any(windows, feature = "whisper-cpp")))]
pub fn load(file: &Path) -> Result<Arc<dyn FinalPass>> {
    anyhow::bail!(
        "{} needs whisper.cpp, and this engine was built without the whisper-cpp feature",
        file.display()
    )
}

#[cfg(any(windows, feature = "whisper-cpp"))]
pub use imp::load;

#[cfg(any(windows, feature = "whisper-cpp"))]
mod imp {
    use super::*;
    use crate::audio::SAMPLE_RATE;
    use crate::timing::{TextSegment, Timed, Timing};
    use anyhow::{anyhow, bail};
    use std::sync::Mutex;
    use transcribe_rs::whisper_cpp::{WhisperEngine, WhisperInferenceParams, WhisperLoadParams};

    /// whisper.cpp ignores input shorter than one second.
    const MIN_SAMPLES: usize = SAMPLE_RATE + SAMPLE_RATE / 4;

    struct Whisper {
        engine: Mutex<WhisperEngine>,
        threads: i32,
    }

    /// Load a GGML file.
    pub fn load(file: &Path) -> Result<Arc<dyn FinalPass>> {
        if file.to_str().is_none() {
            // transcribe-rs passes the path to C as UTF-8.
            bail!("the model path {} is not valid Unicode", file.display());
        }
        let params = WhisperLoadParams {
            use_gpu: transcribe_rs::get_whisper_accelerator().use_gpu(),
            ..WhisperLoadParams::default()
        };
        let engine = WhisperEngine::load_with_params(file, params)
            .map_err(|e| anyhow!("cannot load Whisper from {}: {e}", file.display()))?;
        // whisper.cpp uses at most 4 threads by default. Up to 8 is faster on a
        // desktop processor.
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get().clamp(1, 8)) as i32;
        Ok(Arc::new(Whisper {
            engine: Mutex::new(engine),
            threads,
        }))
    }

    impl FinalPass for Whisper {
        fn transcribe(&self, samples: &[f32], language: &str, vocabulary: &[String]) -> Result<String> {
            self.transcribe_timed(samples, language, vocabulary).map(|timed| timed.text)
        }

        /// whisper.cpp gives segments with times (timestamp level `segment`).
        fn transcribe_timed(&self, samples: &[f32], language: &str, vocabulary: &[String]) -> Result<Timed> {
            if is_silence(samples) {
                return Ok(Timed::untimed(String::new()));
            }
            let mut input = samples.to_vec();
            if input.len() < MIN_SAMPLES {
                input.resize(MIN_SAMPLES, 0.0);
            }
            let params = WhisperInferenceParams {
                language: whisper_language(language),
                initial_prompt: whisper_prompt(vocabulary),
                n_threads: self.threads,
                ..WhisperInferenceParams::default()
            };
            let mut engine = self.engine.lock().unwrap_or_else(|e| e.into_inner());
            let result = engine
                .transcribe_with(&input, &params)
                .map_err(|e| anyhow!("Whisper failed: {e}"))?;
            let text = clean_whisper_text(&result.text);
            let timing = match result.segments {
                Some(segments) if !segments.is_empty() => Timing::Segments(
                    segments
                        .iter()
                        .map(|s| TextSegment {
                            start_ms: (s.start.max(0.0) * 1000.0) as u64,
                            end_ms: (s.end.max(0.0) * 1000.0) as u64,
                            text: clean_whisper_text(&s.text),
                            words: None,
                        })
                        .collect(),
                ),
                _ => Timing::None,
            };
            Ok(Timed { text, timing })
        }
    }
}

/// Whisper makes up words ("You") for silence. Near digital silence, far
/// below the noise floor of a microphone (about 0.003 to 0.01), has no speech.
fn is_silence(samples: &[f32]) -> bool {
    rms(samples) < 0.0005
}

/// `auto` (or nothing) lets Whisper detect the language. A region goes away:
/// whisper.cpp takes "en", not "en-US".
fn whisper_language(language: &str) -> Option<String> {
    let code = language.trim().split(['-', '_']).next().unwrap_or_default().to_ascii_lowercase();
    (!code.is_empty() && code != "auto").then_some(code)
}

/// Dictionary words go in as the initial prompt, like the WhisperKit prompt
/// tokens on macOS. Whisper has no hard boosting.
fn whisper_prompt(vocabulary: &[String]) -> Option<String> {
    let words: Vec<&str> = vocabulary.iter().map(|w| w.trim()).filter(|w| !w.is_empty()).collect();
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
                let known = tag.chars().all(|c| c.is_ascii_uppercase() || c == '_' || c == ' ');
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
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whisper_options() {
        assert_eq!(whisper_language("auto"), None);
        assert_eq!(whisper_language(""), None);
        assert_eq!(whisper_language("de"), Some("de".into()));
        assert_eq!(whisper_language("en-US"), Some("en".into()));
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
    fn silence_is_near_zero() {
        assert!(is_silence(&[0.0; 100]));
        assert!(is_silence(&[]));
        assert!(!is_silence(&[0.01; 100]));
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(&dir.path().join("ggml-tiny.bin")).is_err());
    }
}
