//! The recognizer traits on sherpa-onnx, on the CPU. The final pass of a GGUF model goes to `parakeet.rs`,
//! and of a GGML model to `whisper_cpp.rs`.

use crate::layout::{self, OfflineFiles};
use crate::partials::Segment;
use crate::recognizer::{
    FinalPass, LazyPreview, LivePreview, Loaded, Loader, ModelDirs, PreviewStream,
};
use crate::spec::{ModelSpec, Recipe, Source};
use anyhow::{Context, Result, anyhow, bail};
use sherpa_onnx::{
    OfflineModelConfig, OfflineMoonshineModelConfig, OfflineNemoEncDecCtcModelConfig,
    OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig,
    OfflineTransducerModelConfig, OfflineWhisperModelConfig, OnlineModelConfig, OnlineRecognizer,
    OnlineRecognizerConfig, OnlineStream, OnlineTransducerModelConfig,
};
use std::path::Path;
use std::sync::{Arc, Mutex};

const SAMPLE_RATE: i32 = 16_000;

/// Threads for a final pass. More than 8 rarely helps on the CPU.
fn final_pass_threads() -> i32 {
    std::thread::available_parallelism().map_or(1, |n| n.get().min(8)) as i32
}

/// Threads for the live preview. It runs while the user speaks, so it stays small.
fn preview_threads() -> i32 {
    std::thread::available_parallelism().map_or(1, |n| n.get().min(2)) as i32
}

fn path(p: &Path) -> Option<String> {
    Some(p.to_string_lossy().into_owned())
}

pub struct SherpaLoader;

impl SherpaLoader {
    pub fn new() -> Self {
        SherpaLoader
    }
}

impl Default for SherpaLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl Loader for SherpaLoader {
    fn load_final_pass(
        &self,
        spec: &ModelSpec,
        dirs: &ModelDirs,
    ) -> Result<Option<Arc<dyn FinalPass>>> {
        Ok(if let Source::HuggingFace { file, .. } = &spec.source {
            let file = dirs.main.join(file);
            Some(match spec.recipe {
                Recipe::WhisperCpp => crate::whisper_cpp::load(&file)?,
                _ => crate::parakeet::load(&file)?,
            })
        } else if spec.recipe.final_pass() {
            Some(Arc::new(Offline::load(spec.recipe, &dirs.main)?))
        } else {
            None
        })
    }

    fn load(&self, spec: &ModelSpec, dirs: &ModelDirs) -> Result<Loaded> {
        let final_pass = self.load_final_pass(spec, dirs)?;
        let preview: Option<Arc<dyn LivePreview>> = match &dirs.stream {
            // A streaming model with no final pass is the preview only: load it now.
            Some(dir) if final_pass.is_none() => Some(Arc::new(Online::load(dir)?)),
            // A model with both loads its streaming part at the first stream, so a
            // model that is the final model only never holds it. The files are
            // checked now: a missing file shows at the load, not at the first stream.
            Some(dir) => {
                layout::online_files(dir)?;
                let dir = dir.clone();
                Some(Arc::new(LazyPreview::new(move || {
                    Ok(Arc::new(Online::load(&dir)?) as Arc<dyn LivePreview>)
                })))
            }
            None => None,
        };
        Ok(Loaded {
            final_pass,
            preview,
        })
    }
}

// ---------------------------------------------------------------------------
// Final pass
// ---------------------------------------------------------------------------

/// An offline model. Whisper and SenseVoice take the language when the
/// recognizer is made, so they get a new recognizer when the language changes.
struct Offline {
    files: OfflineFiles,
    recognizers: ByLanguage<OfflineRecognizer>,
}

impl Offline {
    fn load(recipe: Recipe, dir: &Path) -> Result<Offline> {
        let files = layout::offline_files(recipe, dir)?;
        let offline = Offline {
            files,
            recognizers: ByLanguage::default(),
        };
        if offline.uses_language() {
            // The language is not known yet. A recognizer made now for "auto" would give
            // way to a second one at the first dictation in a language, and the model
            // would load twice. The first `transcribe` makes it. The files are checked
            // now, so a missing or empty file shows at the load.
            check_readable(&offline.files)?;
        } else {
            // The recognizer does not depend on the language, so make it now. A model
            // that sherpa-onnx cannot read fails at the load.
            offline.recognizer("auto")?;
        }
        Ok(offline)
    }

    fn uses_language(&self) -> bool {
        matches!(
            self.files,
            OfflineFiles::Whisper { .. } | OfflineFiles::SenseVoice { .. }
        )
    }

    fn recognizer(&self, language: &str) -> Result<Arc<OfflineRecognizer>> {
        let key = recognizer_language(&self.files, language);
        self.recognizers
            .get(&key, || create_offline(&self.files, &key))
    }
}

/// The language codes that SenseVoice takes. It detects the language for any other.
const SENSE_VOICE_LANGUAGES: [&str; 5] = ["zh", "en", "ja", "ko", "yue"];

/// A language code with no region, in lower case: "en-US" and "en_us" are "en". An
/// empty value is "auto".
fn base_language(requested: &str) -> String {
    let code = requested
        .trim()
        .split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if code.is_empty() { "auto".into() } else { code }
}

/// The language that the recognizer for a request is made for. Two requests that give
/// the same value share one recognizer. Whisper takes the language code, and detects the
/// language for "auto". SenseVoice takes five codes and detects the language for the
/// others, so those share the "auto" recognizer. The other models ignore the language.
fn recognizer_language(files: &OfflineFiles, requested: &str) -> String {
    let code = base_language(requested);
    match files {
        OfflineFiles::Whisper { .. } => code,
        OfflineFiles::SenseVoice { .. } if SENSE_VOICE_LANGUAGES.contains(&code.as_str()) => code,
        _ => "auto".into(),
    }
}

/// Fail when a model file is missing or empty.
fn check_readable(files: &OfflineFiles) -> Result<()> {
    for file in files.paths() {
        let size = std::fs::metadata(file)
            .with_context(|| format!("cannot read {}", file.display()))?
            .len();
        if size == 0 {
            bail!("{} is empty", file.display());
        }
    }
    Ok(())
}

/// The one value built for the language of the last request. A request for another
/// language builds a new value, and a failed build keeps the old one.
struct ByLanguage<T> {
    current: Mutex<Option<(String, Arc<T>)>>,
}

impl<T> Default for ByLanguage<T> {
    fn default() -> Self {
        ByLanguage {
            current: Mutex::new(None),
        }
    }
}

impl<T> ByLanguage<T> {
    /// The value for `language`. `build` runs when there is none for it. The lock stays
    /// during the build, so a second request for the same language waits and does not
    /// build again.
    fn get(&self, language: &str, build: impl FnOnce() -> Result<T>) -> Result<Arc<T>> {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((built_for, value)) = &*current
            && built_for == language
        {
            return Ok(Arc::clone(value));
        }
        let value = Arc::new(build()?);
        *current = Some((language.to_string(), Arc::clone(&value)));
        Ok(value)
    }
}

impl FinalPass for Offline {
    /// Dictionary words need hotwords, which work only for transducers with beam
    /// search. The catalog models cannot use them, so they are ignored.
    fn transcribe(&self, samples: &[f32], language: &str, _vocabulary: &[String]) -> Result<String> {
        let recognizer = self.recognizer(language)?;
        let stream = recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE, samples);
        recognizer.decode(&stream);
        let result = stream
            .get_result()
            .ok_or_else(|| anyhow!("the model returned no result"))?;
        Ok(result.text.trim().to_string())
    }
}

/// Make a recognizer. `language` is the value of `recognizer_language`.
fn create_offline(files: &OfflineFiles, language: &str) -> Result<OfflineRecognizer> {
    let mut model = OfflineModelConfig {
        num_threads: final_pass_threads(),
        provider: Some("cpu".into()),
        ..Default::default()
    };
    match files {
        OfflineFiles::Transducer {
            encoder,
            decoder,
            joiner,
            tokens,
        } => {
            model.transducer = OfflineTransducerModelConfig {
                encoder: path(encoder),
                decoder: path(decoder),
                joiner: path(joiner),
            };
            model.model_type = Some("nemo_transducer".into());
            model.tokens = path(tokens);
        }
        OfflineFiles::NemoCtc {
            model: file,
            tokens,
        } => {
            model.nemo_ctc = OfflineNemoEncDecCtcModelConfig { model: path(file) };
            model.tokens = path(tokens);
        }
        OfflineFiles::Whisper {
            encoder,
            decoder,
            tokens,
        } => {
            model.whisper = OfflineWhisperModelConfig {
                encoder: path(encoder),
                decoder: path(decoder),
                // An empty language lets Whisper detect it.
                language: Some(if language == "auto" {
                    String::new()
                } else {
                    language.to_string()
                }),
                task: Some("transcribe".into()),
                ..Default::default()
            };
            model.tokens = path(tokens);
        }
        OfflineFiles::SenseVoice {
            model: file,
            tokens,
        } => {
            model.sense_voice = OfflineSenseVoiceModelConfig {
                model: path(file),
                language: Some(language.to_string()),
                // Inverse text normalization: punctuation, capitals, and digits.
                use_itn: true,
            };
            model.tokens = path(tokens);
        }
        OfflineFiles::MoonshineV2 {
            encoder,
            merged_decoder,
            tokens,
        } => {
            model.moonshine = OfflineMoonshineModelConfig {
                encoder: path(encoder),
                merged_decoder: path(merged_decoder),
                ..Default::default()
            };
            model.tokens = path(tokens);
        }
        OfflineFiles::MoonshineV1 {
            preprocessor,
            encoder,
            uncached_decoder,
            cached_decoder,
            tokens,
        } => {
            model.moonshine = OfflineMoonshineModelConfig {
                preprocessor: path(preprocessor),
                encoder: path(encoder),
                uncached_decoder: path(uncached_decoder),
                cached_decoder: path(cached_decoder),
                merged_decoder: None,
            };
            model.tokens = path(tokens);
        }
    }
    let config = OfflineRecognizerConfig {
        model_config: model,
        decoding_method: Some("greedy_search".into()),
        ..Default::default()
    };
    OfflineRecognizer::create(&config)
        .ok_or_else(|| anyhow!("sherpa-onnx cannot load the model (see the engine log)"))
}

// ---------------------------------------------------------------------------
// Live preview
// ---------------------------------------------------------------------------

/// A streaming transducer.
struct Online {
    recognizer: Arc<OnlineRecognizer>,
}

impl Online {
    fn load(dir: &Path) -> Result<Online> {
        let files = layout::online_files(dir)?;
        let config = OnlineRecognizerConfig {
            model_config: OnlineModelConfig {
                transducer: OnlineTransducerModelConfig {
                    encoder: path(&files.encoder),
                    decoder: path(&files.decoder),
                    joiner: path(&files.joiner),
                },
                tokens: path(&files.tokens),
                num_threads: preview_threads(),
                provider: Some("cpu".into()),
                ..Default::default()
            },
            decoding_method: Some("greedy_search".into()),
            // The sherpa-onnx defaults: an endpoint after 2.4 s of silence with no
            // words, 1.2 s after words, or 20 s of speech.
            enable_endpoint: true,
            rule1_min_trailing_silence: 2.4,
            rule2_min_trailing_silence: 1.2,
            rule3_min_utterance_length: 20.0,
            ..Default::default()
        };
        let recognizer = OnlineRecognizer::create(&config).ok_or_else(|| {
            anyhow!("sherpa-onnx cannot load the streaming model (see the engine log)")
        })?;
        Ok(Online {
            recognizer: Arc::new(recognizer),
        })
    }
}

impl LivePreview for Online {
    fn start(&self) -> Result<Box<dyn PreviewStream>> {
        let stream = self.recognizer.create_stream();
        Ok(Box::new(OnlineSession {
            stream,
            recognizer: Arc::clone(&self.recognizer),
        }))
    }
}

/// One stream. `stream` comes first, so it is dropped before the recognizer.
struct OnlineSession {
    stream: OnlineStream,
    recognizer: Arc<OnlineRecognizer>,
}

impl OnlineSession {
    fn decode_ready(&self) {
        while self.recognizer.is_ready(&self.stream) {
            self.recognizer.decode(&self.stream);
        }
    }

    fn text(&self) -> String {
        self.recognizer
            .get_result(&self.stream)
            .map(|r| r.text.trim().to_string())
            .unwrap_or_default()
    }
}

impl PreviewStream for OnlineSession {
    fn accept(&mut self, samples: &[f32]) {
        self.stream.accept_waveform(SAMPLE_RATE, samples);
    }

    fn decode(&mut self) -> Segment {
        self.decode_ready();
        let text = self.text();
        let endpoint = self.recognizer.is_endpoint(&self.stream);
        if endpoint {
            self.recognizer.reset(&self.stream);
        }
        Segment { text, endpoint }
    }

    fn finish(&mut self) -> Segment {
        // Silence at the end lets the model emit the last words. The Kroko
        // model decodes in chunks of about 1.4 s, and 0.8 s loses the last words.
        let tail = vec![0.0f32; SAMPLE_RATE as usize * 2];
        self.stream.accept_waveform(SAMPLE_RATE, &tail);
        self.stream.input_finished();
        self.decode_ready();
        Segment {
            text: self.text(),
            endpoint: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn whisper() -> OfflineFiles {
        OfflineFiles::Whisper {
            encoder: PathBuf::from("encoder.onnx"),
            decoder: PathBuf::from("decoder.onnx"),
            tokens: PathBuf::from("tokens.txt"),
        }
    }

    fn sense_voice() -> OfflineFiles {
        OfflineFiles::SenseVoice {
            model: PathBuf::from("model.onnx"),
            tokens: PathBuf::from("tokens.txt"),
        }
    }

    fn nemo_ctc() -> OfflineFiles {
        OfflineFiles::NemoCtc {
            model: PathBuf::from("model.onnx"),
            tokens: PathBuf::from("tokens.txt"),
        }
    }

    #[test]
    fn equal_languages_get_one_recognizer() {
        for same in ["en", "en-US", "en_GB", "EN", " en "] {
            assert_eq!(recognizer_language(&whisper(), same), "en", "{same}");
        }
        for auto in ["", "auto", "AUTO", "  "] {
            assert_eq!(recognizer_language(&whisper(), auto), "auto", "{auto:?}");
        }
        assert_eq!(recognizer_language(&whisper(), "zh-CN"), "zh");
        assert_ne!(
            recognizer_language(&whisper(), "en"),
            recognizer_language(&whisper(), "de")
        );
    }

    #[test]
    fn sense_voice_shares_the_auto_recognizer_for_languages_it_does_not_know() {
        assert_eq!(recognizer_language(&sense_voice(), "ja-JP"), "ja");
        assert_eq!(recognizer_language(&sense_voice(), "yue"), "yue");
        for other in ["de", "fr-CA", "", "auto"] {
            assert_eq!(
                recognizer_language(&sense_voice(), other),
                "auto",
                "{other}"
            );
        }
    }

    #[test]
    fn a_model_without_a_language_has_one_recognizer() {
        for language in ["en", "de", "", "auto"] {
            assert_eq!(recognizer_language(&nemo_ctc(), language), "auto");
        }
    }

    #[test]
    fn a_recognizer_is_built_when_first_asked_for_and_once_per_language() {
        let cache = ByLanguage::<String>::default();
        let builds = AtomicUsize::new(0);
        let build = |name: &str| {
            builds.fetch_add(1, Ordering::SeqCst);
            Ok(name.to_string())
        };
        assert_eq!(
            builds.load(Ordering::SeqCst),
            0,
            "nothing builds at the load"
        );

        assert_eq!(*cache.get("en", || build("en")).unwrap(), "en");
        assert_eq!(*cache.get("en", || build("again")).unwrap(), "en");
        assert_eq!(
            builds.load(Ordering::SeqCst),
            1,
            "the same language builds once"
        );

        assert_eq!(*cache.get("de", || build("de")).unwrap(), "de");
        assert_eq!(builds.load(Ordering::SeqCst), 2);
        assert_eq!(
            *cache.get("en", || build("en again")).unwrap(),
            "en again",
            "only the last language is kept"
        );
    }

    #[test]
    fn a_failed_build_keeps_the_recognizer_that_works() {
        let cache = ByLanguage::<String>::default();
        cache.get("en", || Ok("en".to_string())).unwrap();
        assert!(cache.get("de", || Err(anyhow!("broken"))).is_err());
        let builds = AtomicUsize::new(0);
        let kept = cache
            .get("en", || {
                builds.fetch_add(1, Ordering::SeqCst);
                Ok("rebuilt".to_string())
            })
            .unwrap();
        assert_eq!(*kept, "en");
        assert_eq!(builds.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_missing_or_empty_file_fails_at_the_load() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, body: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, body).unwrap();
            path
        };
        let files = OfflineFiles::SenseVoice {
            model: write("model.onnx", b"x"),
            tokens: write("tokens.txt", b"x"),
        };
        assert!(check_readable(&files).is_ok());

        let empty = OfflineFiles::SenseVoice {
            model: write("model.onnx", b""),
            tokens: write("tokens.txt", b"x"),
        };
        let error = check_readable(&empty).unwrap_err().to_string();
        assert!(error.ends_with("is empty"), "{error}");

        let missing = OfflineFiles::SenseVoice {
            model: dir.path().join("gone.onnx"),
            tokens: write("tokens.txt", b"x"),
        };
        assert!(check_readable(&missing).is_err());
    }
}
