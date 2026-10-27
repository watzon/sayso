//! A live preview for a model with no streaming decoder: decode the growing
//! recording again with the final pass (`redecode_preview` in the catalog).
//! Windows uses it for Parakeet TDT v3, which has no streaming archive.
//!
//! A worker thread decodes the uncommitted audio about every 600 ms, when at
//! least 300 ms of new audio arrived. To keep the cost bounded in a long
//! dictation, once the uncommitted audio passes 10 s the older part is decoded
//! one last time and its text is committed. The cut is at the quietest point
//! between 7 s and 2 s before the end, so it rarely splits a word. `committed`
//! then only grows, and `tentative` is the text of the last 2 to 7 s, decoded
//! again each time. The cost of a decode grows faster than its length
//! (NOTES.md), so a short tail keeps the preview fast.
//!
//! The preview and the final pass share one model. While a final pass runs,
//! no preview decode starts, and the end of a stream skips its last decode.

use crate::audio::{self, ms, quietest_point};
use crate::partials::Segment;
use crate::recognizer::{FinalPass, LivePreview, Loaded, PreviewStream};
use anyhow::{Result, anyhow};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Give a loaded final pass a redecode live preview. The final pass that the
/// engine keeps counts its runs, so the preview pauses during them.
pub fn attach(final_pass: Arc<dyn FinalPass>) -> Loaded {
    let running = Arc::new(AtomicUsize::new(0));
    let preview = RedecodePreview {
        model: Arc::clone(&final_pass),
        running: Arc::clone(&running),
        config: Config::default(),
    };
    Loaded {
        final_pass: Some(Arc::new(Counted {
            inner: final_pass,
            running,
        })),
        preview: Some(Arc::new(preview)),
        ..Loaded::default()
    }
}

/// A final pass that counts the runs in progress.
struct Counted {
    inner: Arc<dyn FinalPass>,
    running: Arc<AtomicUsize>,
}

impl FinalPass for Counted {
    fn transcribe(&self, samples: &[f32], language: &str, vocabulary: &[String]) -> Result<String> {
        self.running.fetch_add(1, Ordering::SeqCst);
        let result = self.inner.transcribe(samples, language, vocabulary);
        self.running.fetch_sub(1, Ordering::SeqCst);
        result
    }

    fn transcribe_detailed(&self, samples: &[f32], language: &str, vocabulary: &[String], times: bool) -> Result<crate::timing::Timed> {
        self.running.fetch_add(1, Ordering::SeqCst);
        let result = self.inner.transcribe_detailed(samples, language, vocabulary, times);
        self.running.fetch_sub(1, Ordering::SeqCst);
        result
    }
}

struct RedecodePreview {
    model: Arc<dyn FinalPass>,
    /// Final passes in progress.
    running: Arc<AtomicUsize>,
    config: Config,
}

impl LivePreview for RedecodePreview {
    fn start(&self) -> Result<Box<dyn PreviewStream>> {
        let latest = Arc::new(Mutex::new((String::new(), String::new())));
        let sink = Arc::clone(&latest);
        let running = Arc::clone(&self.running);
        let session = Session::start(
            self.config.clone(),
            Box::new(ModelDecoder(Arc::clone(&self.model))),
            move || running.load(Ordering::SeqCst) > 0,
            move |committed, tentative| {
                *lock(&sink) = (committed.to_string(), tentative.to_string());
            },
        )?;
        Ok(Box::new(RedecodeStream {
            session: Some(session),
            latest,
            reported: String::new(),
            running: Arc::clone(&self.running),
        }))
    }
}

/// The preview decodes with the final pass. Parakeet detects the language
/// itself, and the preview never uses the dictionary words.
struct ModelDecoder(Arc<dyn FinalPass>);

impl Decoder for ModelDecoder {
    fn decode(&mut self, samples: &[f32]) -> Result<String> {
        self.0.transcribe(samples, "auto", &[])
    }
}

/// One stream. The worker thread decodes; `decode` only reads its last text.
struct RedecodeStream {
    session: Option<Session>,
    /// The last `(committed, tentative)` of the worker.
    latest: Arc<Mutex<(String, String)>>,
    /// The committed text that went out as ended segments.
    reported: String,
    running: Arc<AtomicUsize>,
}

impl RedecodeStream {
    /// The part of `committed` that did not go out yet.
    fn unreported<'a>(&self, committed: &'a str) -> &'a str {
        committed.strip_prefix(self.reported.as_str()).unwrap_or(committed).trim()
    }
}

impl PreviewStream for RedecodeStream {
    fn accept(&mut self, samples: &[f32]) {
        if let Some(session) = &self.session {
            session.push(samples);
        }
    }

    /// Newly committed text goes out as an ended segment first. The engine then
    /// asks again at once and gets the tentative text.
    fn decode(&mut self) -> Segment {
        let (committed, tentative) = lock(&self.latest).clone();
        let new = self.unreported(&committed).to_string();
        if !new.is_empty() {
            self.reported = committed;
            return Segment {
                text: new,
                endpoint: true,
            };
        }
        Segment {
            text: tentative,
            endpoint: false,
        }
    }

    fn finish(&mut self) -> Segment {
        let Some(session) = self.session.take() else {
            return Segment {
                text: String::new(),
                endpoint: true,
            };
        };
        // A final pass that runs now has the processor; the preview text it
        // has stays as it is.
        let busy = self.running.load(Ordering::SeqCst) > 0;
        let text = session.finish(busy);
        let rest = self.unreported(&text).to_string();
        self.reported = text;
        Segment {
            text: rest,
            endpoint: true,
        }
    }
}

// ---------------------------------------------------------------------------
// The text state and the worker
// ---------------------------------------------------------------------------

/// Turns samples into text. The engine implements it with a loaded model;
/// tests use a fake.
pub trait Decoder: Send {
    fn decode(&mut self, samples: &[f32]) -> Result<String>;
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Time between two decodes.
    pub interval: Duration,
    /// New audio needed for another decode, in samples.
    pub min_new: usize,
    /// Uncommitted audio that starts a commit, in samples.
    pub commit_after: usize,
    /// Audio that always stays uncommitted after a commit, in samples.
    pub keep: usize,
    /// Length of the region before `keep` where the cut can fall, in samples.
    pub search: usize,
    /// Shorter audio is not decoded (its text is empty), in samples.
    pub min_decode: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            interval: Duration::from_millis(600),
            min_new: ms(300),
            commit_after: ms(10_000),
            keep: ms(2_000),
            search: ms(5_000),
            min_decode: ms(200),
        }
    }
}

/// One decode the preview needs.
#[derive(Debug)]
pub enum Job {
    /// Decode `samples` (the audio before `cut`) and commit the text.
    Commit { cut: usize, samples: Vec<f32> },
    /// Decode all uncommitted audio (`end` samples) as the tentative text.
    Tail { end: usize, samples: Vec<f32> },
}

impl Job {
    pub fn samples(&self) -> &[f32] {
        match self {
            Job::Commit { samples, .. } | Job::Tail { samples, .. } => samples,
        }
    }
}

/// Two texts with a space between them where the script uses spaces.
fn join_texts(left: &str, right: &str) -> String {
    audio::join(&[left.to_string(), right.to_string()])
}

/// The text state of one preview. No threads; [`Session`] drives it.
#[derive(Debug)]
pub struct Preview {
    config: Config,
    /// Audio after the committed part.
    audio: Vec<f32>,
    committed: String,
    tentative: String,
    /// `audio.len()` at the last tail decode.
    decoded: usize,
    /// The tentative text still covers audio that was just committed.
    stale: bool,
    /// The last `(committed, tentative)` that went out.
    sent: Option<(String, String)>,
}

impl Preview {
    pub fn new(config: Config) -> Preview {
        Preview {
            config,
            audio: Vec::new(),
            committed: String::new(),
            tentative: String::new(),
            decoded: 0,
            stale: false,
            sent: None,
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        self.audio.extend_from_slice(samples);
    }

    /// The next decode, if one is due. `finishing` asks for a decode of any
    /// new audio, however little.
    pub fn next_job(&self, finishing: bool) -> Option<Job> {
        let len = self.audio.len();
        let c = &self.config;
        if len > c.commit_after {
            let end = len.saturating_sub(c.keep);
            let cut = quietest_point(&self.audio, end.saturating_sub(c.search)..end).max(1);
            return Some(Job::Commit {
                cut,
                samples: self.audio[..cut].to_vec(),
            });
        }
        let new = len.saturating_sub(self.decoded);
        if self.stale || new >= c.min_new || (finishing && new > 0) {
            return Some(Job::Tail {
                end: len,
                samples: self.audio.clone(),
            });
        }
        None
    }

    /// Take the text of a job. Returns the partial to send, if the text changed.
    pub fn apply(&mut self, job: &Job, text: &str) -> Option<(String, String)> {
        match job {
            Job::Commit { cut, .. } => {
                self.committed = join_texts(&self.committed, text);
                self.audio.drain(..*cut);
                self.decoded = 0;
                self.stale = true;
                None
            }
            Job::Tail { end, .. } => {
                self.tentative = text.trim().to_string();
                self.decoded = *end;
                self.stale = false;
                let pair = (self.committed.clone(), self.tentative.clone());
                if self.sent.as_ref() == Some(&pair) {
                    return None;
                }
                self.sent = Some(pair.clone());
                Some(pair)
            }
        }
    }

    /// A failed decode: keep the old text and wait for new audio.
    pub fn skip(&mut self, job: &Job) {
        if let Job::Tail { end, .. } = job {
            self.decoded = *end;
        }
    }

    /// Committed and tentative text together.
    pub fn text(&self) -> String {
        join_texts(&self.committed, &self.tentative)
    }

    pub fn committed(&self) -> &str {
        &self.committed
    }

    /// Uncommitted audio, in samples.
    pub fn pending(&self) -> usize {
        self.audio.len()
    }
}

/// The end of a stream: decode the new audio one last time (when there is a
/// decoder), then return the full text.
pub fn finish_preview(preview: Preview, decoder: Option<&mut dyn Decoder>) -> String {
    let state = Mutex::new(preview);
    if let Some(decoder) = decoder {
        drive(&state, decoder, true, || false);
    }
    lock(&state).text()
}

/// Run one job. Audio shorter than `min_decode` has no text. A panic in the
/// decoder becomes an error, so the worker keeps going.
fn run_job(decoder: &mut dyn Decoder, job: &Job, min_decode: usize) -> Result<String> {
    if job.samples().len() < min_decode {
        return Ok(String::new());
    }
    catch_unwind(AssertUnwindSafe(|| decoder.decode(job.samples())))
        .unwrap_or_else(|_| Err(anyhow!("the decoder panicked")))
}

/// Decode every due job (a commit is followed by a tail decode at once).
/// Returns the partials to send.
pub fn drive(
    preview: &Mutex<Preview>,
    decoder: &mut dyn Decoder,
    finishing: bool,
    mut stop: impl FnMut() -> bool,
) -> Vec<(String, String)> {
    let min_decode = lock(preview).config.min_decode;
    let mut partials = Vec::new();
    // At most a commit and a tail per round, plus slack for a commit that
    // leaves more than `commit_after` behind.
    for _ in 0..4 {
        if stop() {
            break;
        }
        let Some(job) = lock(preview).next_job(finishing) else {
            break;
        };
        let result = run_job(decoder, &job, min_decode);
        let mut state = lock(preview);
        match result {
            Ok(text) => {
                if let Some(partial) = state.apply(&job, &text) {
                    partials.push(partial);
                }
            }
            Err(e) => {
                eprintln!("sayso-engine: a live preview decode failed: {e:#}");
                state.skip(&job);
                break;
            }
        }
        if matches!(job, Job::Tail { .. }) {
            break;
        }
    }
    partials
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

struct Shared {
    preview: Mutex<Preview>,
    stop: Mutex<bool>,
    wake: Condvar,
}

/// A running preview: the audio buffer and its worker thread.
pub struct Session {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<Box<dyn Decoder>>>,
}

impl Session {
    /// Start the worker. `paused` is asked before each decode; while it is
    /// true no decode starts (a final pass has the processor). `on_partial`
    /// gets `(committed, tentative)` when the text changed.
    pub fn start(
        config: Config,
        decoder: Box<dyn Decoder>,
        paused: impl Fn() -> bool + Send + 'static,
        on_partial: impl Fn(&str, &str) + Send + 'static,
    ) -> Result<Session> {
        let interval = config.interval;
        let shared = Arc::new(Shared {
            preview: Mutex::new(Preview::new(config)),
            stop: Mutex::new(false),
            wake: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("preview-redecode".into())
            .spawn(move || {
                let shared = worker_shared;
                let mut decoder = decoder;
                let mut next = Instant::now() + interval;
                loop {
                    {
                        let mut stopped = lock(&shared.stop);
                        while !*stopped {
                            let now = Instant::now();
                            if now >= next {
                                break;
                            }
                            stopped = shared
                                .wake
                                .wait_timeout(stopped, next - now)
                                .unwrap_or_else(|e| e.into_inner())
                                .0;
                        }
                        if *stopped {
                            break;
                        }
                    }
                    let started = Instant::now();
                    next = started + interval;
                    if paused() {
                        continue;
                    }
                    let stop = || *lock(&shared.stop) || paused();
                    for (committed, tentative) in drive(&shared.preview, decoder.as_mut(), false, stop) {
                        on_partial(&committed, &tentative);
                    }
                    // A decode longer than the interval (a long tail on a slow
                    // processor) is followed by a rest of half its time, so the
                    // preview never takes more than two thirds of the time.
                    let took = started.elapsed();
                    next = next.max(Instant::now() + took / 2);
                }
                decoder
            })
            .map_err(|e| anyhow!("cannot start the live preview thread: {e}"))?;
        Ok(Session {
            shared,
            worker: Some(worker),
        })
    }

    pub fn push(&self, samples: &[f32]) {
        lock(&self.shared.preview).push(samples);
    }

    /// Stop the worker (it ends after the decode that runs now, if any), then
    /// decode the new audio one last time unless `skip_decode`. Returns the
    /// full text.
    pub fn finish(mut self, skip_decode: bool) -> String {
        let mut decoder = self.stop_worker().filter(|_| !skip_decode);
        let preview = std::mem::replace(&mut *lock(&self.shared.preview), Preview::new(Config::default()));
        let decoder: Option<&mut dyn Decoder> = match decoder.as_mut() {
            Some(decoder) => Some(decoder.as_mut()),
            None => None,
        };
        finish_preview(preview, decoder)
    }

    fn stop_worker(&mut self) -> Option<Box<dyn Decoder>> {
        *lock(&self.shared.stop) = true;
        self.shared.wake.notify_all();
        self.worker.take().and_then(|worker| worker.join().ok())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.worker.is_some() {
            self.stop_worker();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partials::Partials;
    use std::sync::atomic::AtomicBool;

    /// Synthetic speech: word `k` is 400 ms of the constant `level(k)`, then
    /// 100 ms of silence (every 5th gap is 300 ms).
    fn level(k: usize) -> f32 {
        0.1 + 0.001 * k as f32
    }

    fn speech(words: usize) -> Vec<f32> {
        let mut samples = Vec::new();
        for k in 0..words {
            samples.extend(std::iter::repeat_n(level(k), ms(400)));
            let gap = if k % 5 == 4 { ms(300) } else { ms(100) };
            samples.extend(std::iter::repeat_n(0.0, gap));
        }
        samples
    }

    fn expected(words: usize) -> String {
        (0..words).map(|k| format!("w{k}")).collect::<Vec<_>>().join(" ")
    }

    /// Hears one word per run of non-zero samples longer than 50 ms.
    fn words_in(samples: &[f32]) -> String {
        let mut words = Vec::new();
        let mut run = 0;
        let mut value = 0.0;
        for &s in samples.iter().chain(std::iter::once(&0.0)) {
            if s != 0.0 {
                run += 1;
                value = s;
            } else {
                if run > ms(50) {
                    words.push(format!("w{}", ((value - 0.1) / 0.001).round() as usize));
                }
                run = 0;
            }
        }
        words.join(" ")
    }

    /// Records the length of every decode.
    #[derive(Clone, Default)]
    struct FakeDecoder {
        lengths: Arc<Mutex<Vec<usize>>>,
        delay: Duration,
    }

    impl Decoder for FakeDecoder {
        fn decode(&mut self, samples: &[f32]) -> Result<String> {
            self.lengths.lock().unwrap().push(samples.len());
            std::thread::sleep(self.delay);
            Ok(words_in(samples))
        }
    }

    /// The fake as a final pass. It counts its calls and can wait in each one.
    #[derive(Default)]
    struct FakeModel {
        calls: AtomicUsize,
        delay: Duration,
    }

    impl FinalPass for FakeModel {
        fn transcribe(&self, samples: &[f32], _language: &str, _vocabulary: &[String]) -> Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            Ok(words_in(samples))
        }
    }

    /// Feed the audio in 160 ms chunks and decode whenever a job is due, without threads.
    fn run_sync(samples: &[f32], config: Config, decoder: &mut FakeDecoder) -> (Vec<(String, String)>, Preview) {
        let preview = Mutex::new(Preview::new(config));
        let mut partials = Vec::new();
        for chunk in samples.chunks(ms(160)) {
            lock(&preview).push(chunk);
            partials.extend(drive(&preview, decoder, false, || false));
        }
        partials.extend(drive(&preview, decoder, true, || false));
        (partials, preview.into_inner().unwrap())
    }

    fn fast_config() -> Config {
        Config {
            interval: Duration::from_millis(20),
            ..Config::default()
        }
    }

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "timed out: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn short_dictation_previews_without_commits() {
        let samples = speech(8); // 4.4 s
        let mut decoder = FakeDecoder::default();
        let (partials, mut preview) = run_sync(&samples, Config::default(), &mut decoder);
        assert!(partials.len() >= 5, "{partials:?}");
        assert!(partials.iter().all(|(c, _)| c.is_empty()), "nothing is committed in 4 s");
        assert_eq!(preview.text(), expected(8));
        assert_eq!(partials.last().map(|(_, t)| t.as_str()), Some(expected(8).as_str()));
        // Partials only go out when the text changed.
        assert!(partials.windows(2).all(|w| w[0] != w[1]));
        // The end of the stream decodes the last word.
        preview.push(&speech(9)[speech(8).len()..]);
        assert_eq!(finish_preview(preview, Some(&mut decoder)), expected(9));
    }

    #[test]
    fn long_dictation_commits_at_quiet_points_and_stays_bounded() {
        let words = 150; // about 80 s
        let samples = speech(words);
        let config = Config::default();
        let mut decoder = FakeDecoder::default();
        let (partials, preview) = run_sync(&samples, config.clone(), &mut decoder);
        assert_eq!(preview.text(), expected(words), "no word lost or repeated");
        assert!(!preview.committed().is_empty());
        let longest = decoder.lengths.lock().unwrap().iter().copied().max().unwrap();
        assert!(longest <= config.commit_after + ms(160), "decodes stay bounded: {longest}");
        assert!(preview.pending() <= config.commit_after);
        // `committed` only grows, and it is always a prefix of the final text.
        let mut last = String::new();
        for (committed, _) in &partials {
            assert!(committed.starts_with(last.as_str()), "{last:?} -> {committed:?}");
            assert!(expected(words).starts_with(committed.as_str()));
            last = committed.clone();
        }
        assert!(partials.iter().any(|(c, t)| !c.is_empty() && !t.is_empty()));
    }

    #[test]
    fn the_cut_falls_in_silence() {
        let samples = speech(60);
        let mut preview = Preview::new(Config::default());
        preview.push(&samples[..ms(11_000)]);
        let Some(Job::Commit { cut, .. }) = preview.next_job(false) else {
            panic!("a commit is due")
        };
        assert_eq!(samples[cut], 0.0, "cut at {cut} is inside a word");
        assert!((ms(4_000)..=ms(9_000)).contains(&cut));
    }

    #[test]
    fn little_new_audio_waits() {
        let mut preview = Preview::new(Config::default());
        preview.push(&vec![0.1; ms(200)]);
        assert!(preview.next_job(false).is_none());
        let job = preview.next_job(true).unwrap();
        assert!(matches!(job, Job::Tail { end, .. } if end == ms(200)));
        // Too short to decode: empty text, no decoder call.
        let mut decoder = FakeDecoder::default();
        let short = Job::Tail {
            end: 10,
            samples: vec![0.1; 10],
        };
        assert_eq!(run_job(&mut decoder, &short, ms(200)).unwrap(), "");
        assert!(decoder.lengths.lock().unwrap().is_empty());
    }

    struct Panicky;
    impl Decoder for Panicky {
        fn decode(&mut self, _: &[f32]) -> Result<String> {
            panic!("boom")
        }
    }

    #[test]
    fn a_panicking_decoder_is_an_error() {
        let job = Job::Tail {
            end: ms(500),
            samples: vec![0.1; ms(500)],
        };
        assert!(run_job(&mut Panicky, &job, 0).is_err());
    }

    #[test]
    fn a_session_sends_partials_and_finishes_with_the_full_text() {
        let samples = speech(10);
        let decoder = FakeDecoder {
            delay: Duration::from_millis(2),
            ..FakeDecoder::default()
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let session = Session::start(fast_config(), Box::new(decoder), || false, move |c, t| {
            tx.send(join_texts(c, t)).unwrap();
        })
        .unwrap();
        for chunk in samples.chunks(ms(160)) {
            session.push(chunk);
            std::thread::sleep(Duration::from_millis(15));
        }
        assert_eq!(session.finish(false), expected(10));
        let partials: Vec<String> = rx.try_iter().collect();
        assert!(partials.len() >= 3, "{partials:?}");
    }

    #[test]
    fn a_paused_session_does_not_decode_and_finish_can_skip_the_last_decode() {
        let decoder = FakeDecoder::default();
        let lengths = Arc::clone(&decoder.lengths);
        let paused = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&paused);
        let session =
            Session::start(fast_config(), Box::new(decoder), move || flag.load(Ordering::SeqCst), |_, _| {}).unwrap();
        session.push(&speech(4));
        std::thread::sleep(Duration::from_millis(100));
        assert!(lengths.lock().unwrap().is_empty(), "paused");
        assert_eq!(session.finish(true), "");
        assert!(lengths.lock().unwrap().is_empty());
    }

    #[test]
    fn finish_stops_the_worker_promptly() {
        let decoder = FakeDecoder {
            delay: Duration::from_millis(50),
            ..FakeDecoder::default()
        };
        let session = Session::start(fast_config(), Box::new(decoder), || false, |_, _| {}).unwrap();
        session.push(&speech(6));
        std::thread::sleep(Duration::from_millis(40));
        let started = Instant::now();
        session.finish(true);
        assert!(started.elapsed() < Duration::from_millis(500), "{:?}", started.elapsed());
    }

    /// Run a stream as the engine's stream worker does: after an ended segment
    /// it asks again at once.
    fn update(stream: &mut dyn PreviewStream, partials: &mut Partials) {
        loop {
            let segment = stream.decode();
            partials.update(&segment);
            if !segment.endpoint {
                break;
            }
        }
    }

    #[test]
    fn the_stream_reports_commits_as_ended_segments_and_keeps_every_word() {
        let model = Arc::new(FakeModel::default());
        let loaded = attach(model.clone());
        let preview = RedecodePreview {
            model: model.clone(),
            running: Arc::new(AtomicUsize::new(0)),
            config: fast_config(),
        };
        assert!(loaded.preview.is_some() && loaded.final_pass.is_some());
        let mut stream = preview.start().unwrap();
        let mut partials = Partials::default();
        let words = 30; // about 16 s: at least one commit
        for chunk in speech(words).chunks(ms(160)) {
            stream.accept(chunk);
            std::thread::sleep(Duration::from_millis(4));
            update(stream.as_mut(), &mut partials);
        }
        let mut committed_seen = false;
        wait_until("a commit", || {
            update(stream.as_mut(), &mut partials);
            committed_seen |= partials.changed().is_some_and(|(c, _)| !c.is_empty());
            committed_seen
        });
        partials.update(&stream.finish());
        assert_eq!(partials.text(), expected(words));
        assert!(model.calls.load(Ordering::SeqCst) > 2);
    }

    #[test]
    fn a_running_final_pass_pauses_the_preview() {
        let model = Arc::new(FakeModel {
            delay: Duration::from_millis(300),
            ..FakeModel::default()
        });
        let loaded = attach(model.clone());
        let final_pass = loaded.final_pass.unwrap();
        let preview = loaded.preview.unwrap();
        let mut stream = preview.start().unwrap();
        stream.accept(&speech(4));
        let final_thread = std::thread::spawn(move || final_pass.transcribe(&speech(2), "en", &[]).unwrap());
        wait_until("the final pass starts", || model.calls.load(Ordering::SeqCst) == 1);
        // The preview interval is 600 ms; while the final pass runs no decode starts,
        // and the end of the stream skips its last decode.
        let segment = stream.finish();
        assert_eq!(segment, Segment { text: String::new(), endpoint: true });
        assert_eq!(final_thread.join().unwrap(), expected(2));
        assert_eq!(model.calls.load(Ordering::SeqCst), 1, "only the final pass decoded");
    }
}
