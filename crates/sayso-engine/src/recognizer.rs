//! The seam between the protocol and the speech library.
//!
//! The engine talks to models only through these traits. [`crate::sherpa`]
//! implements them with sherpa-onnx, [`crate::parakeet`] and
//! [`crate::whisper_cpp`] add final passes on ggml, and [`crate::redecode`]
//! makes a live preview from a final pass. VAD and diarization models are here
//! too. The tests use a fake.

use crate::diarization::RawTurn;
use crate::partials::Segment;
use crate::spec::ModelSpec;
use crate::timing::Timed;
use crate::vad::Detector;
use anyhow::{Result, anyhow};
use sayso_core::model_store::{Lease, Store};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

/// The folders of a model's extracted archives.
#[derive(Debug, Clone)]
pub struct ModelDirs {
    /// The main archive.
    pub main: PathBuf,
    /// The archive that streams the live preview, if the model has one.
    pub stream: Option<PathBuf>,
    /// The speaker embedding model of a diarization model. Its segmentation model is `main`.
    pub embedding: Option<PathBuf>,
}

/// The files of a loaded model in the model store: the `use` locks stay while
/// something holds this. When the last holder lets go, a cleanup removes the
/// artifacts of a model that another engine has deleted in the meantime.
pub struct StoreUse {
    pub lease: Option<Lease>,
    pub store: Store,
}

impl Drop for StoreUse {
    fn drop(&mut self) {
        self.lease = None;
        let store = self.store.clone();
        let _ = std::thread::Builder::new().name("store-cleanup".into()).spawn(move || {
            let _ = store.cleanup();
        });
    }
}

/// Loads models. Loading is slow, so the engine calls it on a worker thread.
pub trait Loader: Send + Sync {
    fn load(&self, spec: &ModelSpec, dirs: &ModelDirs) -> Result<Loaded>;
    /// Load only the final pass, for a model whose live preview is in memory
    /// already. None when the recipe has no final pass.
    fn load_final_pass(
        &self,
        spec: &ModelSpec,
        dirs: &ModelDirs,
    ) -> Result<Option<Arc<dyn FinalPass>>>;
}

/// A model in memory. A speech model has a final pass, a live preview, or both.
/// After an `unload` with `keep_preview`, the final pass of a model with both
/// is out and the live preview stays. A VAD model has `vad`, and a diarization
/// model has `diarizer`. They have no other part.
#[derive(Clone, Default)]
pub struct Loaded {
    pub final_pass: Option<Arc<dyn FinalPass>>,
    pub preview: Option<Arc<dyn LivePreview>>,
    pub vad: Option<Arc<dyn VadModel>>,
    pub diarizer: Option<Arc<dyn Diarizer>>,
    /// For a model in the model store. A job or a stream that uses the model holds it too.
    pub store_use: Option<Arc<StoreUse>>,
}

/// A VAD model. It makes one detector for each audio.
pub trait VadModel: Send + Sync {
    fn detector(&self) -> Result<Box<dyn Detector>>;
}

/// A diarization model.
pub trait Diarizer: Send + Sync {
    /// Who speaks when in 16 kHz mono audio, with the speaker numbers of the model.
    /// `speakers` is the number of speakers when the client knows it, else the model finds it.
    fn diarize(&self, samples: &[f32], speakers: Option<u32>) -> Result<Vec<RawTurn>>;
}

/// The final pass: whole recordings to text.
pub trait FinalPass: Send + Sync {
    /// Transcribe one piece of 16 kHz mono audio. `language` is a language
    /// code or "auto". `vocabulary` holds the dictionary words; a model that
    /// cannot use them ignores them. The engine cuts long recordings into
    /// pieces first, unless the recipe cuts them itself.
    fn transcribe(&self, samples: &[f32], language: &str, vocabulary: &[String]) -> Result<String>;

    /// The same as [`FinalPass::transcribe`], with the language that the model
    /// found (`docs/engine-protocol.md`, "Detected language") and, when `times`
    /// is true, the times that the model gives ("Segments"). Times are from the
    /// start of `samples`. A model can give times also when `times` is false;
    /// `times` lets a model use a slower decode that only a request with
    /// segments needs. A model without times and without a language keeps this
    /// default.
    fn transcribe_detailed(&self, samples: &[f32], language: &str, vocabulary: &[String], times: bool) -> Result<Timed> {
        let _ = times;
        self.transcribe(samples, language, vocabulary).map(Timed::untimed)
    }
}

/// The live preview: a streaming model.
pub trait LivePreview: Send + Sync {
    /// Start a new stream.
    fn start(&self) -> Result<Box<dyn PreviewStream>>;
}

/// One stream of a live preview.
pub trait PreviewStream: Send {
    /// Add 16 kHz mono audio. This must be fast: it does not decode.
    fn accept(&mut self, samples: &[f32]);
    /// Decode the audio that is ready. Returns the text of the current segment.
    /// After a segment with `endpoint`, the engine calls `decode` again at once.
    fn decode(&mut self) -> Segment;
    /// No more audio comes. Decode the rest and return the last segment,
    /// with `endpoint` set.
    fn finish(&mut self) -> Segment;
}

// ---------------------------------------------------------------------------
// Lazy live preview
// ---------------------------------------------------------------------------

/// The most audio a stream holds while its model loads: a minute at 16 kHz.
const MAX_HELD_SAMPLES: usize = 16_000 * 60;

type PreviewLoad = dyn Fn() -> Result<Arc<dyn LivePreview>> + Send + Sync;

enum Slot {
    /// Nothing is in memory and no load runs.
    Idle,
    Loading,
    Ready(Arc<dyn LivePreview>),
    /// The last load failed. The next `start` reports it and then tries again.
    Failed(String),
}

fn lock(slot: &Mutex<Slot>) -> MutexGuard<'_, Slot> {
    slot.lock().unwrap_or_else(|e| e.into_inner())
}

/// A live preview that loads its model when the first stream starts, on its own
/// thread. A model that is the final model only never loads its streaming part.
///
/// `start` does not wait for the load: the app opens a stream while the user starts
/// to speak, and one worker serves every stream. The stream holds the audio and decodes
/// it when the model is in memory, so the first preview of a process comes late and
/// catches up. The model stays in memory until the loaded model is dropped.
pub struct LazyPreview {
    load: Arc<PreviewLoad>,
    slot: Arc<Mutex<Slot>>,
}

impl LazyPreview {
    pub fn new(load: impl Fn() -> Result<Arc<dyn LivePreview>> + Send + Sync + 'static) -> Self {
        LazyPreview {
            load: Arc::new(load),
            slot: Arc::new(Mutex::new(Slot::Idle)),
        }
    }

    /// True when the streaming model is in memory.
    pub fn is_loaded(&self) -> bool {
        matches!(*lock(&self.slot), Slot::Ready(_))
    }

    fn spawn_load(&self) -> std::io::Result<()> {
        let load = Arc::clone(&self.load);
        let slot = Arc::clone(&self.slot);
        std::thread::Builder::new()
            .name("preview-load".into())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| load()));
                *lock(&slot) = match result {
                    Ok(Ok(preview)) => Slot::Ready(preview),
                    Ok(Err(e)) => Slot::Failed(format!("{e:#}")),
                    Err(_) => Slot::Failed("the streaming model load stopped unexpectedly".into()),
                };
            })
            .map(drop)
    }
}

impl LivePreview for LazyPreview {
    fn start(&self) -> Result<Box<dyn PreviewStream>> {
        {
            let mut state = lock(&self.slot);
            match &*state {
                Slot::Ready(_) | Slot::Loading => {}
                Slot::Failed(message) => {
                    let error = anyhow!("{message}");
                    *state = Slot::Idle;
                    return Err(error);
                }
                Slot::Idle => {
                    *state = Slot::Loading;
                    if let Err(e) = self.spawn_load() {
                        *state = Slot::Idle;
                        return Err(anyhow!("cannot start the streaming model load: {e}"));
                    }
                }
            }
        }
        Ok(Box::new(LazyStream {
            slot: Arc::clone(&self.slot),
            inner: None,
            held: Vec::new(),
            lost: false,
        }))
    }
}

/// A stream that waits for its model. It decodes nothing and gives empty text
/// until the model is in memory.
struct LazyStream {
    slot: Arc<Mutex<Slot>>,
    inner: Option<Box<dyn PreviewStream>>,
    /// Audio that came before the model.
    held: Vec<f32>,
    /// The model gave no stream. This stream stays empty.
    lost: bool,
}

impl LazyStream {
    /// Move to the model when it is in memory. True when the stream runs on it.
    fn attach(&mut self) -> bool {
        if self.inner.is_some() {
            return true;
        }
        if self.lost {
            return false;
        }
        let preview = match &*lock(&self.slot) {
            Slot::Ready(preview) => Arc::clone(preview),
            _ => return false,
        };
        match preview.start() {
            Ok(mut stream) => {
                stream.accept(&self.held);
                self.held = Vec::new();
                self.inner = Some(stream);
                true
            }
            Err(_) => {
                self.lost = true;
                self.held = Vec::new();
                false
            }
        }
    }
}

impl PreviewStream for LazyStream {
    fn accept(&mut self, samples: &[f32]) {
        if self.attach() {
            if let Some(inner) = &mut self.inner {
                inner.accept(samples);
            }
        } else if !self.lost && self.held.len() + samples.len() <= MAX_HELD_SAMPLES {
            self.held.extend_from_slice(samples);
        }
    }

    fn decode(&mut self) -> Segment {
        self.attach();
        match &mut self.inner {
            Some(inner) => inner.decode(),
            None => Segment::default(),
        }
    }

    fn finish(&mut self) -> Segment {
        // A stream that ends before its model loads gives no text. The final pass
        // does not use the preview, and the stream worker must not wait for the load.
        self.attach();
        match &mut self.inner {
            Some(inner) => inner.finish(),
            None => Segment {
                text: String::new(),
                endpoint: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// Counts the samples it heard. `decode` gives the count as text.
    struct Counting {
        heard: Arc<AtomicUsize>,
    }

    impl PreviewStream for Counting {
        fn accept(&mut self, samples: &[f32]) {
            self.heard.fetch_add(samples.len(), Ordering::SeqCst);
        }
        fn decode(&mut self) -> Segment {
            Segment {
                text: self.heard.load(Ordering::SeqCst).to_string(),
                endpoint: false,
            }
        }
        fn finish(&mut self) -> Segment {
            Segment {
                text: format!("done {}", self.heard.load(Ordering::SeqCst)),
                endpoint: true,
            }
        }
    }

    struct CountingPreview {
        heard: Arc<AtomicUsize>,
    }

    impl LivePreview for CountingPreview {
        fn start(&self) -> Result<Box<dyn PreviewStream>> {
            Ok(Box::new(Counting {
                heard: Arc::clone(&self.heard),
            }))
        }
    }

    fn counters() -> (Arc<AtomicUsize>, Arc<AtomicUsize>) {
        (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)))
    }

    /// A lazy preview whose load waits for the sender. It counts its loads.
    fn gated(
        loads: &Arc<AtomicUsize>,
        heard: &Arc<AtomicUsize>,
    ) -> (LazyPreview, mpsc::Sender<()>) {
        let (release, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let (loads, heard) = (Arc::clone(loads), Arc::clone(heard));
        let preview = LazyPreview::new(move || {
            loads.fetch_add(1, Ordering::SeqCst);
            gate.lock().unwrap().recv().ok();
            Ok(Arc::new(CountingPreview {
                heard: Arc::clone(&heard),
            }) as Arc<dyn LivePreview>)
        });
        (preview, release)
    }

    fn wait_until(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "timed out: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn nothing_loads_until_the_first_stream() {
        let (loads, heard) = counters();
        let (preview, _release) = gated(&loads, &heard);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(loads.load(Ordering::SeqCst), 0);
        assert!(!preview.is_loaded());
    }

    #[test]
    fn start_returns_while_the_model_loads_and_the_stream_catches_up() {
        let (loads, heard) = counters();
        let (preview, release) = gated(&loads, &heard);
        // The load waits for the gate, so a `start` that waited for it would hang.
        let mut stream = preview.start().unwrap();
        stream.accept(&[0.1; 100]);
        assert_eq!(stream.decode(), Segment::default());
        assert_eq!(
            heard.load(Ordering::SeqCst),
            0,
            "the audio waits for the model"
        );

        release.send(()).unwrap();
        wait_until("the model loads", || preview.is_loaded());
        stream.accept(&[0.1; 50]);
        assert_eq!(
            heard.load(Ordering::SeqCst),
            150,
            "the held audio first, then the new audio"
        );
        assert_eq!(stream.decode().text, "150");
        assert_eq!(stream.finish().text, "done 150");
    }

    #[test]
    fn streams_share_one_load() {
        let (loads, heard) = counters();
        let (preview, release) = gated(&loads, &heard);
        let mut first = preview.start().unwrap();
        let mut second = preview.start().unwrap();
        release.send(()).unwrap();
        wait_until("the model loads", || preview.is_loaded());
        first.accept(&[0.0; 10]);
        second.accept(&[0.0; 10]);
        let mut third = preview.start().unwrap();
        third.accept(&[0.0; 10]);
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(heard.load(Ordering::SeqCst), 30);
    }

    #[test]
    fn a_stream_that_ends_before_the_load_gives_no_text() {
        let (loads, heard) = counters();
        let (preview, _release) = gated(&loads, &heard);
        let mut stream = preview.start().unwrap();
        stream.accept(&[0.1; 100]);
        let segment = stream.finish();
        assert_eq!(segment.text, "");
        assert!(segment.endpoint);
    }

    #[test]
    fn a_stream_holds_at_most_a_minute_of_audio() {
        let (loads, heard) = counters();
        let (preview, release) = gated(&loads, &heard);
        let mut stream = preview.start().unwrap();
        stream.accept(&vec![0.1; MAX_HELD_SAMPLES]);
        stream.accept(&[0.1; 1]);
        release.send(()).unwrap();
        wait_until("the model loads", || preview.is_loaded());
        stream.accept(&[]);
        assert_eq!(heard.load(Ordering::SeqCst), MAX_HELD_SAMPLES);
    }

    #[test]
    fn a_failed_load_shows_at_the_next_start_and_then_tries_again() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let preview = LazyPreview::new(move || {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(anyhow!("broken model"))
            } else {
                Ok(Arc::new(CountingPreview {
                    heard: Arc::new(AtomicUsize::new(0)),
                }) as Arc<dyn LivePreview>)
            }
        });
        let mut stream = preview.start().unwrap();
        wait_until("the first load fails", || {
            matches!(*lock(&preview.slot), Slot::Failed(_))
        });
        // The stream of the failed load stays empty.
        stream.accept(&[0.1; 10]);
        assert_eq!(stream.decode(), Segment::default());

        let error = preview.start().err().expect("the failure shows once");
        assert_eq!(error.to_string(), "broken model");
        let _retry = preview.start().unwrap();
        wait_until("the second load", || preview.is_loaded());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_panic_in_the_load_is_a_failure() {
        let preview = LazyPreview::new(|| panic!("boom"));
        let _stream = preview.start().unwrap();
        wait_until("the load ends", || {
            matches!(*lock(&preview.slot), Slot::Failed(_))
        });
        assert!(preview.start().is_err());
    }
}
