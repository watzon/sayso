//! The final pass of `whisper_cpp` models: one Whisper GGML file on whisper.cpp,
//! through whisper-rs, with the GPU choice of transcribe-rs. Windows uses it: whisper.cpp is faster
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
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

    /// whisper.cpp ignores input shorter than one second.
    const MIN_SAMPLES: usize = SAMPLE_RATE + SAMPLE_RATE / 4;

    struct Whisper {
        /// The state holds the context (the model) too.
        state: Mutex<WhisperState>,
        threads: i32,
    }

    /// Load a GGML file. whisper-rs is used directly, because transcribe-rs
    /// keeps the state private and its result has no language. The settings are
    /// those of `WhisperEngine::load` in transcribe-rs 0.3.12: a GPU when the
    /// build has one, the best GPU, and flash attention.
    pub fn load(file: &Path) -> Result<Arc<dyn FinalPass>> {
        let Some(path) = file.to_str() else {
            // whisper-rs passes the path to C as UTF-8.
            bail!("the model path {} is not valid Unicode", file.display());
        };
        if !file.exists() {
            bail!("cannot load Whisper from {}: the file is missing", file.display());
        }
        let use_gpu = transcribe_rs::get_whisper_accelerator().use_gpu();
        let params = WhisperContextParameters {
            use_gpu,
            flash_attn: true,
            gpu_device: if use_gpu { transcribe_rs::whisper_cpp::gpu::auto_select_gpu_device() } else { 0 },
            ..WhisperContextParameters::default()
        };
        let context = WhisperContext::new_with_params(path, params)
            .map_err(|e| anyhow!("cannot load Whisper from {}: {e}", file.display()))?;
        let state = context
            .create_state()
            .map_err(|e| anyhow!("cannot load Whisper from {}: {e}", file.display()))?;
        // whisper.cpp uses at most 4 threads by default. Up to 8 is faster on a
        // desktop processor.
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get().clamp(1, 8)) as i32;
        Ok(Arc::new(Whisper {
            state: Mutex::new(state),
            threads,
        }))
    }

    impl FinalPass for Whisper {
        fn transcribe(&self, samples: &[f32], language: &str, vocabulary: &[String]) -> Result<String> {
            self.transcribe_detailed(samples, language, vocabulary, false).map(|timed| timed.text)
        }

        /// whisper.cpp gives segments with times (timestamp level `segment`)
        /// and the language that it found, at no extra cost.
        fn transcribe_detailed(&self, samples: &[f32], language: &str, vocabulary: &[String], _times: bool) -> Result<Timed> {
            if is_silence(samples) {
                return Ok(Timed::untimed(String::new()));
            }
            let mut input = samples.to_vec();
            if input.len() < MIN_SAMPLES {
                input.resize(MIN_SAMPLES, 0.0);
            }
            let language = whisper_language(language);
            let prompt = whisper_prompt(vocabulary);
            // The parameters of transcribe-rs 0.3.12 (`WhisperEngine::transcribe_with`).
            let mut params = FullParams::new(SamplingStrategy::BeamSearch { beam_size: 3, patience: -1.0 });
            params.set_language(language.as_deref());
            params.set_translate(false);
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            params.set_suppress_blank(true);
            params.set_suppress_nst(true);
            params.set_no_speech_thold(0.2);
            params.set_no_context(true);
            params.set_n_threads(self.threads);
            if let Some(prompt) = &prompt {
                params.set_initial_prompt(prompt);
            }
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.full(params, &input).map_err(|e| anyhow!("Whisper failed: {e}"))?;
            let mut raw_text = String::new();
            let mut segments = Vec::new();
            for i in 0..state.full_n_segments() {
                let segment = state.get_segment(i).ok_or_else(|| anyhow!("Whisper failed: segment {i} is missing"))?;
                let text = segment.to_str().map_err(|e| anyhow!("Whisper failed: {e}"))?;
                raw_text.push_str(text);
                // whisper.cpp gives times in 10 ms steps.
                segments.push(TextSegment {
                    start_ms: segment.start_timestamp().max(0) as u64 * 10,
                    end_ms: segment.end_timestamp().max(0) as u64 * 10,
                    text: clean_whisper_text(text),
                    words: None,
                });
            }
            let found = whisper_rs::get_lang_str(state.full_lang_id_from_state()).and_then(crate::language::normalize);
            let timing = if segments.is_empty() { Timing::None } else { Timing::Segments(segments) };
            Ok(Timed { text: clean_whisper_text(&raw_text), timing, language: found })
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
