//! The recognizer traits on sherpa-onnx. CPU only.

use crate::layout::{self, OfflineFiles};
use crate::partials::Segment;
use crate::recognizer::{FinalPass, LivePreview, Loaded, Loader, ModelDirs, PreviewStream};
use crate::spec::{ModelSpec, Recipe};
use anyhow::{Result, anyhow};
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
        Ok(if spec.recipe.final_pass() {
            Some(Arc::new(Offline::load(spec.recipe, &dirs.main)?))
        } else {
            None
        })
    }

    fn load(&self, spec: &ModelSpec, dirs: &ModelDirs) -> Result<Loaded> {
        let final_pass = self.load_final_pass(spec, dirs)?;
        let preview: Option<Arc<dyn LivePreview>> = match &dirs.stream {
            Some(dir) => Some(Arc::new(Online::load(dir)?)),
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
    current: Mutex<(String, Arc<OfflineRecognizer>)>,
}

impl Offline {
    fn load(recipe: Recipe, dir: &Path) -> Result<Offline> {
        let files = layout::offline_files(recipe, dir)?;
        let language = "auto".to_string();
        let recognizer = Arc::new(create_offline(&files, &language)?);
        Ok(Offline {
            files,
            current: Mutex::new((language, recognizer)),
        })
    }

    fn uses_language(&self) -> bool {
        matches!(
            self.files,
            OfflineFiles::Whisper { .. } | OfflineFiles::SenseVoice { .. }
        )
    }

    fn recognizer(&self, language: &str) -> Result<Arc<OfflineRecognizer>> {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if self.uses_language() && current.0 != language {
            let recognizer = Arc::new(create_offline(&self.files, language)?);
            *current = (language.to_string(), recognizer);
        }
        Ok(Arc::clone(&current.1))
    }
}

impl FinalPass for Offline {
    fn transcribe(&self, samples: &[f32], language: &str) -> Result<String> {
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
            let known = ["zh", "en", "ja", "ko", "yue"].contains(&language);
            model.sense_voice = OfflineSenseVoiceModelConfig {
                model: path(file),
                language: Some(if known { language } else { "auto" }.to_string()),
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
