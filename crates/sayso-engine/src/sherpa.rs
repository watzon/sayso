//! The recognizer traits on sherpa-onnx, on the CPU. The final pass of a GGUF model goes to `parakeet.rs`,
//! and of a GGML model to `whisper_cpp.rs`.

use crate::diarization::{CLUSTER_THRESHOLD, RawTurn};
use crate::layout::{self, OfflineFiles};
use crate::partials::Segment;
use crate::recognizer::{
    Diarizer, FinalPass, LazyPreview, LivePreview, Loaded, Loader, ModelDirs, PreviewStream, VadModel,
};
use crate::sherpa_c::{self, Decoded};
use crate::spec::{ModelSpec, Recipe, Source, Task};
use crate::timing::{self, TextSegment, Timed, Timing};
use crate::vad::{self, Detector};
use anyhow::{Context, Result, anyhow, bail};
use sherpa_onnx::{
    FastClusteringConfig, OfflineModelConfig, OfflineMoonshineModelConfig,
    OfflineNemoEncDecCtcModelConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig,
    OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig,
    OfflineTransducerModelConfig, OnlineModelConfig, OnlineRecognizer,
    OnlineRecognizerConfig, OnlineStream, OnlineTransducerModelConfig, SileroVadModelConfig,
    SpeakerEmbeddingExtractorConfig, TenVadModelConfig, VadModelConfig, VoiceActivityDetector,
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
        match spec.recipe.task() {
            Task::Vad => return load_vad(spec, dirs),
            Task::Diarization => return load_diarization(spec, dirs),
            Task::Speech => {}
        }
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
                let store_use = dirs.store_use.clone();
                Some(Arc::new(LazyPreview::new(move || {
                    let _store_use = &store_use;
                    Ok(Arc::new(Online::load(&dir)?) as Arc<dyn LivePreview>)
                })))
            }
            None => None,
        };
        Ok(Loaded {
            final_pass,
            preview,
            ..Loaded::default()
        })
    }
}

// ---------------------------------------------------------------------------
// Final pass
// ---------------------------------------------------------------------------

/// An offline model. Whisper and SenseVoice take the language when the
/// recognizer is made, so they get a new recognizer when the language changes.
/// Whisper also gets a new one when a request with segments follows one
/// without, or the opposite (see [`Key`]).
struct Offline {
    files: OfflineFiles,
    recognizers: ByLanguage<Key, Recognizer>,
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
            offline.recognizer("auto", false)?;
        }
        Ok(offline)
    }

    fn uses_language(&self) -> bool {
        matches!(
            self.files,
            OfflineFiles::Whisper { .. } | OfflineFiles::SenseVoice { .. }
        )
    }

    fn recognizer(&self, language: &str, times: bool) -> Result<Arc<Recognizer>> {
        let key = recognizer_key(&self.files, language, times);
        self.recognizers
            .get(&key, || create_offline(&self.files, &key))
    }
}

/// What a recognizer is made for. Two requests with the same key share one recognizer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    /// The value of [`recognizer_language`].
    language: String,
    /// Whisper with segment times. With them, the text of Whisper can be
    /// different: in tests it lost punctuation, and once the last sentence of a
    /// 26 s clip. So only a `transcribe` with `segments: true` uses them, and
    /// dictation keeps the recognizer without them.
    segment_times: bool,
}

/// The key of the recognizer for a request. `times` is true for a request with segments.
fn recognizer_key(files: &OfflineFiles, requested: &str, times: bool) -> Key {
    Key {
        language: recognizer_language(files, requested),
        segment_times: times && matches!(files, OfflineFiles::Whisper { .. }),
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

/// The one value built for the key (the language) of the last request. A request
/// with another key builds a new value, and a failed build keeps the old one.
struct ByLanguage<K, T> {
    current: Mutex<Option<(K, Arc<T>)>>,
}

impl<K, T> Default for ByLanguage<K, T> {
    fn default() -> Self {
        ByLanguage {
            current: Mutex::new(None),
        }
    }
}

impl<K: PartialEq + Clone, T> ByLanguage<K, T> {
    /// The value for `key`. `build` runs when there is none for it. The lock stays
    /// during the build, so a second request for the same key waits and does not
    /// build again.
    fn get(&self, key: &K, build: impl FnOnce() -> Result<T>) -> Result<Arc<T>> {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((built_for, value)) = &*current
            && built_for == key
        {
            return Ok(Arc::clone(value));
        }
        let value = Arc::new(build()?);
        *current = Some((key.clone(), Arc::clone(&value)));
        Ok(value)
    }
}

impl FinalPass for Offline {
    /// Dictionary words need hotwords, which work only for transducers with beam
    /// search. The catalog models cannot use them, so they are ignored.
    fn transcribe(&self, samples: &[f32], language: &str, vocabulary: &[String]) -> Result<String> {
        self.transcribe_detailed(samples, language, vocabulary, false).map(|timed| timed.text)
    }

    /// Token times give word times when the model has them (Parakeet TDT,
    /// SenseVoice). Whisper gives segment times with `times` only (see [`Key`]).
    /// Moonshine gives none. Whisper and SenseVoice give the language.
    fn transcribe_detailed(&self, samples: &[f32], language: &str, _vocabulary: &[String], times: bool) -> Result<Timed> {
        let recognizer = self.recognizer(language, times)?;
        let result = recognizer.decode(samples)?;
        let audio_ms = (samples.len() as u64 * 1000) / SAMPLE_RATE as u64;
        Ok(Timed {
            text: result.text.trim().to_string(),
            timing: timing_of(&result, audio_ms),
            language: result.lang.as_deref().and_then(crate::language::normalize),
        })
    }
}

/// The times of a result: the segments of Whisper, else word times from the token times.
fn timing_of(result: &Decoded, audio_ms: u64) -> Timing {
    if !result.segments.is_empty() {
        let ms = |seconds: f32| ((seconds.max(0.0) * 1000.0).round() as u64).min(audio_ms);
        return Timing::Segments(
            result
                .segments
                .iter()
                .map(|s| TextSegment {
                    start_ms: ms(s.start),
                    end_ms: ms(s.start + s.duration.max(0.0)),
                    text: s.text.trim().to_string(),
                    words: None,
                })
                .collect(),
        );
    }
    match &result.timestamps {
        Some(starts) if !starts.is_empty() && starts.len() == result.tokens.len() => {
            let words = timing::words_from_tokens(&result.tokens, starts, result.durations.as_deref(), audio_ms);
            if words.is_empty() { Timing::None } else { Timing::Words(words) }
        }
        _ => Timing::None,
    }
}

/// An offline recognizer. Whisper and SenseVoice are made on the C API, which
/// gives the language and the segment times (`sherpa_c`). The other models use
/// the `sherpa-onnx` crate.
enum Recognizer {
    Crate(OfflineRecognizer),
    C(sherpa_c::Recognizer),
}

impl Recognizer {
    fn decode(&self, samples: &[f32]) -> Result<Decoded> {
        match self {
            Recognizer::Crate(recognizer) => {
                let stream = recognizer.create_stream();
                stream.accept_waveform(SAMPLE_RATE, samples);
                recognizer.decode(&stream);
                let result = stream
                    .get_result()
                    .ok_or_else(|| anyhow!("the model returned no result"))?;
                Ok(Decoded {
                    text: result.text,
                    tokens: result.tokens,
                    timestamps: result.timestamps,
                    durations: result.durations,
                    ..Decoded::default()
                })
            }
            Recognizer::C(recognizer) => recognizer.decode(SAMPLE_RATE, samples),
        }
    }
}

/// Make a recognizer for `key` (see [`recognizer_key`]).
fn create_offline(files: &OfflineFiles, key: &Key) -> Result<Recognizer> {
    let language = key.language.as_str();
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
            let common = sherpa_c::Common { tokens, threads: final_pass_threads() };
            // An empty language lets Whisper detect it.
            let language = if language == "auto" { "" } else { language };
            return sherpa_c::Recognizer::whisper(encoder, decoder, language, key.segment_times, &common).map(Recognizer::C);
        }
        OfflineFiles::SenseVoice {
            model: file,
            tokens,
        } => {
            let common = sherpa_c::Common { tokens, threads: final_pass_threads() };
            return sherpa_c::Recognizer::sense_voice(file, language, &common).map(Recognizer::C);
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
        .map(Recognizer::Crate)
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

// ---------------------------------------------------------------------------
// VAD
// ---------------------------------------------------------------------------

/// The speech probability above which a window is speech. This is the default of sherpa-onnx.
const VAD_THRESHOLD: f32 = 0.5;

/// The `max_speech_duration` of the detector, in seconds. It is long on purpose. The detector
/// does not cut speech at that length. It only splits at a short pause, and it does not split
/// speech without one. A split at a short pause leaves a gap under the pause that ends speech
/// (500 ms) between two parts, which the protocol does not allow. So `vad::Tracker` cuts the
/// speech at 30 s, and the detector keeps its other rules.
const VAD_MAX_SPEECH_SECONDS: f32 = 3600.0;

/// The seconds of audio that the buffer of a detector holds at first. It grows when needed (and
/// sherpa-onnx writes a line to the log each time).
const VAD_BUFFER_SECONDS: f32 = 60.0;

/// A VAD model: the configuration of sherpa-onnx. Each audio gets its own detector, because
/// a detector keeps the state of the audio it hears.
struct SherpaVad {
    config: VadModelConfig,
    window: usize,
}

fn vad_config(recipe: Recipe, model: &Path) -> VadModelConfig {
    let seconds = |ms: usize| ms as f32 / 1000.0;
    let mut config = VadModelConfig {
        sample_rate: SAMPLE_RATE,
        num_threads: 1,
        provider: Some("cpu".into()),
        ..Default::default()
    };
    match recipe {
        Recipe::TenVad => {
            config.ten_vad = TenVadModelConfig {
                model: path(model),
                threshold: VAD_THRESHOLD,
                min_silence_duration: seconds(vad::MIN_SILENCE_MS),
                min_speech_duration: seconds(vad::MIN_SPEECH_MS),
                window_size: 256,
                max_speech_duration: VAD_MAX_SPEECH_SECONDS,
            };
        }
        _ => {
            config.silero_vad = SileroVadModelConfig {
                model: path(model),
                threshold: VAD_THRESHOLD,
                min_silence_duration: seconds(vad::MIN_SILENCE_MS),
                min_speech_duration: seconds(vad::MIN_SPEECH_MS),
                window_size: 512,
                max_speech_duration: VAD_MAX_SPEECH_SECONDS,
            };
        }
    }
    config
}

fn load_vad(spec: &ModelSpec, dirs: &ModelDirs) -> Result<Loaded> {
    let Source::SherpaFile { file } = &spec.source else {
        bail!("a VAD model is one file");
    };
    let config = vad_config(spec.recipe, &layout::vad_file(&dirs.main, file)?);
    let window = match spec.recipe {
        Recipe::TenVad => config.ten_vad.window_size,
        _ => config.silero_vad.window_size,
    } as usize;
    let model = SherpaVad { config, window };
    // A model that sherpa-onnx cannot read fails at the load.
    model.detector()?;
    Ok(Loaded {
        vad: Some(Arc::new(model)),
        ..Loaded::default()
    })
}

impl VadModel for SherpaVad {
    fn detector(&self) -> Result<Box<dyn Detector>> {
        let vad = VoiceActivityDetector::create(&self.config, VAD_BUFFER_SECONDS)
            .ok_or_else(|| anyhow!("sherpa-onnx cannot load the VAD model (see the engine log)"))?;
        Ok(Box::new(SherpaDetector { vad, window: self.window }))
    }
}

struct SherpaDetector {
    vad: VoiceActivityDetector,
    window: usize,
}

impl Detector for SherpaDetector {
    fn window(&self) -> usize {
        self.window
    }

    fn accept(&mut self, window: &[f32]) {
        self.vad.accept_waveform(window);
    }

    fn detected(&self) -> bool {
        self.vad.detected()
    }

    fn next_segment(&mut self) -> Option<(u64, u64)> {
        let segment = self.vad.front()?;
        let start = u64::try_from(segment.start()).unwrap_or(0);
        let end = start + u64::try_from(segment.n()).unwrap_or(0);
        drop(segment);
        self.vad.pop();
        Some((start, end))
    }

    fn flush(&mut self) {
        self.vad.flush();
    }
}

// ---------------------------------------------------------------------------
// Diarization
// ---------------------------------------------------------------------------

/// Pyannote segmentation, a speaker embedding model, and fast clustering.
struct SherpaDiarizer {
    /// The clustering changes with each request, so a request sets it and runs under the lock.
    diarizer: Mutex<OfflineSpeakerDiarization>,
    config: OfflineSpeakerDiarizationConfig,
}

fn load_diarization(spec: &ModelSpec, dirs: &ModelDirs) -> Result<Loaded> {
    let (Source::Diarization { embedding }, Some(embedding_dir)) = (&spec.source, &dirs.embedding) else {
        bail!("a diarization model has a segmentation model and an embedding model");
    };
    let files = layout::diarization_files(&dirs.main, embedding_dir, embedding)?;
    let threads = final_pass_threads();
    let config = OfflineSpeakerDiarizationConfig {
        segmentation: OfflineSpeakerSegmentationModelConfig {
            pyannote: OfflineSpeakerSegmentationPyannoteModelConfig {
                model: path(&files.segmentation),
                ..Default::default()
            },
            num_threads: threads,
            ..Default::default()
        },
        embedding: SpeakerEmbeddingExtractorConfig {
            model: path(&files.embedding),
            num_threads: threads,
            ..Default::default()
        },
        clustering: clustering(None),
        ..Default::default()
    };
    let diarizer = OfflineSpeakerDiarization::create(&config)
        .ok_or_else(|| anyhow!("sherpa-onnx cannot load the diarization model (see the engine log)"))?;
    if diarizer.sample_rate() != SAMPLE_RATE {
        bail!("the diarization model needs {} Hz audio, not {SAMPLE_RATE} Hz", diarizer.sample_rate());
    }
    Ok(Loaded {
        diarizer: Some(Arc::new(SherpaDiarizer { diarizer: Mutex::new(diarizer), config })),
        ..Loaded::default()
    })
}

/// With a number of speakers, the clustering makes that many clusters. Without one, it
/// merges the embeddings that are closer than the threshold.
fn clustering(speakers: Option<u32>) -> FastClusteringConfig {
    FastClusteringConfig {
        num_clusters: speakers.map_or(-1, |n| i32::try_from(n).unwrap_or(i32::MAX)),
        threshold: CLUSTER_THRESHOLD,
        compute_confidence: false,
    }
}

impl Diarizer for SherpaDiarizer {
    fn diarize(&self, samples: &[f32], speakers: Option<u32>) -> Result<Vec<RawTurn>> {
        let mut config = self.config.clone();
        config.clustering = clustering(speakers);
        let diarizer = self.diarizer.lock().unwrap_or_else(|e| e.into_inner());
        diarizer.set_config(&config);
        let result = diarizer
            .process(samples)
            .ok_or_else(|| anyhow!("sherpa-onnx cannot diarize the audio (see the engine log)"))?;
        Ok(result
            .sort_by_start_time()
            .into_iter()
            .map(|turn| RawTurn { start: turn.start, end: turn.end, speaker: turn.speaker })
            .collect())
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
    fn whisper_with_segments_has_its_own_recognizer() {
        let dictation = recognizer_key(&whisper(), "auto", false);
        let job = recognizer_key(&whisper(), "", true);
        assert_eq!(dictation, Key { language: "auto".into(), segment_times: false });
        assert_eq!(job, Key { language: "auto".into(), segment_times: true });
        assert_ne!(dictation, job, "dictation never gets the recognizer with segment times");
        assert_eq!(recognizer_key(&whisper(), "en-US", true), Key { language: "en".into(), segment_times: true });
    }

    #[test]
    fn other_models_share_the_recognizer_with_and_without_segments() {
        for files in [sense_voice(), nemo_ctc()] {
            assert_eq!(recognizer_key(&files, "auto", true), recognizer_key(&files, "auto", false));
        }
        let cache = ByLanguage::<Key, String>::default();
        cache.get(&recognizer_key(&sense_voice(), "de", false), || Ok("auto".into())).unwrap();
        let shared = cache.get(&recognizer_key(&sense_voice(), "fr", true), || Ok("rebuilt".into())).unwrap();
        assert_eq!(*shared, "auto");
    }

    #[test]
    fn whisper_segments_become_segment_times() {
        let result = Decoded {
            text: " One. Two. Three.".into(),
            segments: vec![
                sherpa_c::RawSegment { start: 0.0, duration: 5.16, text: " One.".into() },
                sherpa_c::RawSegment { start: 5.16, duration: 4.68, text: " Two.".into() },
                sherpa_c::RawSegment { start: 9.84, duration: 3.12, text: " Three.".into() },
            ],
            ..Decoded::default()
        };
        let Timing::Segments(segments) = timing_of(&result, 12_000) else {
            panic!("segments expected");
        };
        let times: Vec<_> = segments.iter().map(|s| (s.start_ms, s.end_ms, s.text.as_str())).collect();
        assert_eq!(times, [(0, 5_160, "One."), (5_160, 9_840, "Two."), (9_840, 12_000, "Three.")], "the last end stays in the audio");
    }

    #[test]
    fn token_times_become_word_times_and_no_times_is_none() {
        let words = Decoded {
            text: "Guten Tag.".into(),
            tokens: vec!["▁Guten".into(), "▁Tag.".into()],
            timestamps: Some(vec![0.3, 0.66]),
            ..Decoded::default()
        };
        assert!(matches!(timing_of(&words, 2_000), Timing::Words(w) if w.len() == 2));
        let none = Decoded { text: "Hello.".into(), tokens: vec![" Hello".into()], timestamps: Some(vec![]), ..Decoded::default() };
        assert_eq!(timing_of(&none, 2_000), Timing::None);
    }

    #[test]
    fn a_recognizer_is_built_when_first_asked_for_and_once_per_language() {
        let cache = ByLanguage::<&str, String>::default();
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

        assert_eq!(*cache.get(&"en", || build("en")).unwrap(), "en");
        assert_eq!(*cache.get(&"en", || build("again")).unwrap(), "en");
        assert_eq!(
            builds.load(Ordering::SeqCst),
            1,
            "the same language builds once"
        );

        assert_eq!(*cache.get(&"de", || build("de")).unwrap(), "de");
        assert_eq!(builds.load(Ordering::SeqCst), 2);
        assert_eq!(
            *cache.get(&"en", || build("en again")).unwrap(),
            "en again",
            "only the last language is kept"
        );
    }

    #[test]
    fn a_failed_build_keeps_the_recognizer_that_works() {
        let cache = ByLanguage::<&str, String>::default();
        cache.get(&"en", || Ok("en".to_string())).unwrap();
        assert!(cache.get(&"de", || Err(anyhow!("broken"))).is_err());
        let builds = AtomicUsize::new(0);
        let kept = cache
            .get(&"en", || {
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
