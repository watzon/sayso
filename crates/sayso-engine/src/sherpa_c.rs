//! Whisper and SenseVoice recognizers on the C API of sherpa-onnx.
//!
//! The C API gives the result of a decode as JSON. The `sherpa-onnx` crate
//! reads only `text`, `tokens`, `timestamps`, and `durations` from it, and keeps
//! the pointers of its recognizer and stream private. The engine also needs
//! `lang` (the language that the model found) and the segment times of Whisper
//! (`segment_timestamps`, `segment_durations`, `segment_texts`). So these two
//! models are made here, on `sherpa-onnx-sys`, the same C library that the
//! crate uses.

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use sherpa_onnx_sys as sys;
use std::ffi::{CStr, CString, c_char};
use std::path::Path;

/// One segment of a Whisper result, in seconds from the start of the audio.
#[derive(Debug, Clone, PartialEq)]
pub struct RawSegment {
    pub start: f32,
    pub duration: f32,
    pub text: String,
}

/// The result of one decode.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decoded {
    pub text: String,
    pub tokens: Vec<String>,
    /// The start of each token, in seconds, when the model gives it.
    pub timestamps: Option<Vec<f32>>,
    /// The length of each token, in seconds, when the model gives it.
    pub durations: Option<Vec<f32>>,
    /// The language as the model gives it: `en` (Whisper) or `<|en|>` (SenseVoice).
    pub lang: Option<String>,
    /// Whisper with `enable_segment_timestamps` only.
    pub segments: Vec<RawSegment>,
}

impl Decoded {
    /// Read the JSON of `SherpaOnnxGetOfflineStreamResultAsJson`.
    pub fn parse(json: &str) -> Result<Decoded> {
        let value: Value = serde_json::from_str(json).context("the model returned a result that is not JSON")?;
        let strings = |key: &str| -> Vec<String> {
            value[key]
                .as_array()
                .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default()
        };
        let floats = |key: &str| -> Option<Vec<f32>> {
            value[key].as_array().map(|items| items.iter().filter_map(Value::as_f64).map(|f| f as f32).collect())
        };
        let segments = match (floats("segment_timestamps"), floats("segment_durations")) {
            (Some(starts), Some(durations)) => starts
                .into_iter()
                .zip(durations)
                .zip(strings("segment_texts"))
                .map(|((start, duration), text)| RawSegment { start, duration, text })
                .collect(),
            _ => Vec::new(),
        };
        Ok(Decoded {
            text: value["text"].as_str().unwrap_or_default().to_string(),
            tokens: strings("tokens"),
            timestamps: floats("timestamps"),
            durations: floats("durations"),
            lang: value["lang"].as_str().filter(|l| !l.is_empty()).map(str::to_string),
            segments,
        })
    }
}

/// The settings of a recognizer that both models share.
pub struct Common<'a> {
    pub tokens: &'a Path,
    pub threads: i32,
}

/// An offline recognizer of the C API.
pub struct Recognizer {
    ptr: *const sys::OfflineRecognizer,
}

// SAFETY: the C library allows one recognizer to decode streams from more than
// one thread; the `sherpa-onnx` crate marks its recognizer the same way.
unsafe impl Send for Recognizer {}
unsafe impl Sync for Recognizer {}

/// The strings of a configuration. The C API reads them only while it makes the
/// recognizer, so they live until then.
#[derive(Default)]
struct Strings(Vec<CString>);

impl Strings {
    fn add(&mut self, value: &str) -> Result<*const c_char> {
        let c = CString::new(value).map_err(|_| anyhow!("{value:?} has a NUL character"))?;
        let ptr = c.as_ptr();
        self.0.push(c);
        Ok(ptr)
    }

    fn path(&mut self, path: &Path) -> Result<*const c_char> {
        self.add(&path.to_string_lossy())
    }
}

/// A configuration with every field zero, as the C examples make it with
/// `memset`: the C API uses its default for a zero field. The fields that the
/// `sherpa-onnx` crate sets by default are set to the same values.
fn base_config(strings: &mut Strings, common: &Common) -> Result<sys::OfflineRecognizerConfig> {
    // SAFETY: the configuration is `repr(C)` and holds only raw pointers,
    // integers, floats, and structs of them. All zero bytes are a valid value
    // for each: null pointers and zeros.
    let mut config: sys::OfflineRecognizerConfig = unsafe { std::mem::zeroed() };
    config.feat_config.sample_rate = 16_000;
    config.feat_config.feature_dim = 80;
    config.lm_config.scale = 1.0;
    config.max_active_paths = 4;
    config.decoding_method = strings.add("greedy_search")?;
    config.model_config.tokens = strings.path(common.tokens)?;
    config.model_config.num_threads = common.threads;
    config.model_config.provider = strings.add("cpu")?;
    Ok(config)
}

impl Recognizer {
    /// Whisper. An empty `language` lets Whisper find it. `segment_times`
    /// turns on the segment times; the text can then be different.
    pub fn whisper(
        encoder: &Path,
        decoder: &Path,
        language: &str,
        segment_times: bool,
        common: &Common,
    ) -> Result<Recognizer> {
        let mut strings = Strings::default();
        let mut config = base_config(&mut strings, common)?;
        let whisper = &mut config.model_config.whisper;
        whisper.encoder = strings.path(encoder)?;
        whisper.decoder = strings.path(decoder)?;
        whisper.language = strings.add(language)?;
        whisper.task = strings.add("transcribe")?;
        // Token times need cross-attention outputs, which the exports do not have.
        whisper.enable_token_timestamps = 0;
        whisper.enable_segment_timestamps = i32::from(segment_times);
        Self::create(&config)
    }

    /// SenseVoice with inverse text normalization (punctuation, capitals, and digits).
    pub fn sense_voice(model: &Path, language: &str, common: &Common) -> Result<Recognizer> {
        let mut strings = Strings::default();
        let mut config = base_config(&mut strings, common)?;
        let sense_voice = &mut config.model_config.sense_voice;
        sense_voice.model = strings.path(model)?;
        sense_voice.language = strings.add(language)?;
        sense_voice.use_itn = 1;
        Self::create(&config)
    }

    fn create(config: &sys::OfflineRecognizerConfig) -> Result<Recognizer> {
        // SAFETY: `config` and the strings that it points to are valid during the call.
        let ptr = unsafe { sys::SherpaOnnxCreateOfflineRecognizer(config) };
        if ptr.is_null() {
            bail!("sherpa-onnx cannot load the model (see the engine log)");
        }
        Ok(Recognizer { ptr })
    }

    /// Decode 16 kHz mono audio.
    pub fn decode(&self, sample_rate: i32, samples: &[f32]) -> Result<Decoded> {
        let count = i32::try_from(samples.len()).map_err(|_| anyhow!("the audio is too long for one decode"))?;
        // SAFETY: `self.ptr` is a live recognizer.
        let stream = Stream(unsafe { sys::SherpaOnnxCreateOfflineStream(self.ptr) });
        if stream.0.is_null() {
            bail!("sherpa-onnx cannot make a stream");
        }
        // SAFETY: the stream is live, and `samples` holds `count` floats.
        unsafe {
            sys::SherpaOnnxAcceptWaveformOffline(stream.0, sample_rate, samples.as_ptr(), count);
            sys::SherpaOnnxDecodeOfflineStream(self.ptr, stream.0);
        }
        // SAFETY: the stream is live. The JSON is a C string that only
        // `SherpaOnnxDestroyOfflineStreamResultJson` frees, after the copy.
        let json = unsafe {
            let ptr = sys::SherpaOnnxGetOfflineStreamResultAsJson(stream.0);
            if ptr.is_null() {
                bail!("the model returned no result");
            }
            let json = CStr::from_ptr(ptr).to_string_lossy().into_owned();
            sys::SherpaOnnxDestroyOfflineStreamResultJson(ptr);
            json
        };
        Decoded::parse(&json)
    }
}

impl Drop for Recognizer {
    fn drop(&mut self) {
        // SAFETY: the pointer came from `SherpaOnnxCreateOfflineRecognizer`, and
        // each stream is dropped at the end of its `decode`.
        unsafe { sys::SherpaOnnxDestroyOfflineRecognizer(self.ptr) }
    }
}

/// A stream of one decode. It is destroyed when `decode` returns.
struct Stream(*const sys::OfflineStream);

impl Drop for Stream {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from `SherpaOnnxCreateOfflineStream`.
            unsafe { sys::SherpaOnnxDestroyOfflineStream(self.0) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `lang` and the segment keys of a real result: Whisper tiny (int8) with
    /// segment times, sherpa-onnx 1.13.8, on an English clip. `text` and
    /// `tokens` are added in the form that the C API gives them.
    const WHISPER_SEGMENTS: &str = r#"{"lang":"en","segment_timestamps":[0.00,5.16,9.84],"segment_durations":[5.16,4.68,3.12],"segment_texts":[" Hello, this is a test of the speech recognizer."," The quick brown fox jumps over the lazy dog."," Now we are speaking the third sentence after another pause."],"text":" Hello, this is a test of the speech recognizer. The quick brown fox jumps over the lazy dog. Now we are speaking the third sentence after another pause.","tokens":[" Hello",","," this"],"timestamps":[]}"#;

    #[test]
    fn whisper_segments_and_language_come_from_the_json() {
        let decoded = Decoded::parse(WHISPER_SEGMENTS).unwrap();
        assert_eq!(decoded.lang.as_deref(), Some("en"));
        assert!(decoded.text.starts_with(" Hello, this is a test"));
        assert_eq!(decoded.tokens, [" Hello", ",", " this"]);
        assert_eq!(decoded.timestamps.as_deref(), Some(&[][..]));
        assert_eq!(decoded.segments.len(), 3);
        assert_eq!(
            decoded.segments[1],
            RawSegment { start: 5.16, duration: 4.68, text: " The quick brown fox jumps over the lazy dog.".into() }
        );
        assert_eq!(decoded.segments[2].start, 9.84);
        assert_eq!(decoded.segments[2].duration, 3.12);
    }

    #[test]
    fn sense_voice_gives_the_language_in_tags_and_token_times() {
        let json = r#"{"lang":"<|de|>","emotion":"<|NEUTRAL|>","event":"<|Speech|>","text":"Guten Tag.","timestamps":[0.30,0.66],"durations":[],"tokens":["▁Guten","▁Tag."],"words":[]}"#;
        let decoded = Decoded::parse(json).unwrap();
        assert_eq!(decoded.lang.as_deref(), Some("<|de|>"));
        assert_eq!(decoded.timestamps.as_deref(), Some(&[0.30, 0.66][..]));
        assert_eq!(decoded.durations.as_deref(), Some(&[][..]));
        assert!(decoded.segments.is_empty());
    }

    #[test]
    fn missing_keys_are_empty() {
        let decoded = Decoded::parse(r#"{"text":"","lang":""}"#).unwrap();
        assert_eq!(decoded, Decoded::default());
        assert!(Decoded::parse("not json").is_err());
    }

    #[test]
    fn segments_need_starts_and_durations() {
        let json = r#"{"text":"a","segment_timestamps":[0.0],"segment_texts":["a"]}"#;
        assert!(Decoded::parse(json).unwrap().segments.is_empty());
    }
}
