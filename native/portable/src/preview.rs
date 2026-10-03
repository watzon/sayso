//! Live preview for models without a streaming decoder (Parakeet through
//! transcribe-rs): decode the recording again while it grows.
//!
//! A worker thread decodes the uncommitted audio about every 600 ms, when at
//! least 300 ms of new audio arrived, and reports `(committed, tentative)` only
//! when the text changed. To keep the cost bounded in a long dictation, once
//! the uncommitted audio passes 20 s the older part is decoded one last time
//! and its text is committed. The cut is at the quietest point between 12 s and
//! 4 s before the end, so it rarely splits a word. `committed` then only grows,
//! and `tentative` is the text of the last 4 to 12 s, decoded again each time.

use crate::audio::{join_texts, ms, quietest_point};
use crate::protocol::Result;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

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
            commit_after: ms(20_000),
            keep: ms(4_000),
            search: ms(8_000),
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
                self.committed = join_texts([self.committed.as_str(), text]);
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
        join_texts([self.committed.as_str(), self.tentative.as_str()])
    }

    /// The last partial of a stream: all the text, committed. None when it is
    /// the text that went out last.
    pub fn final_partial(&mut self) -> Option<String> {
        let text = self.text();
        let last = self
            .sent
            .as_ref()
            .map(|(c, t)| join_texts([c.as_str(), t.as_str()]))
            .unwrap_or_default();
        if text == last {
            return None;
        }
        self.sent = Some((text.clone(), String::new()));
        Some(text)
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
/// decoder), then the final partial (None when the text the client saw last
/// is the full text) and the full text.
pub fn finish_preview(
    preview: Preview,
    decoder: Option<&mut dyn Decoder>,
) -> (Option<String>, String) {
    let sent = preview.sent.clone();
    let state = Mutex::new(preview);
    if let Some(decoder) = decoder {
        // These partials are not sent; the final partial covers them.
        drive(&state, decoder, true, || false);
    }
    let mut preview = state.into_inner().unwrap_or_else(|e| e.into_inner());
    preview.sent = sent;
    let partial = preview.final_partial();
    (partial, preview.text())
}

/// Run one job. Audio shorter than `min_decode` has no text. A panic in the
/// decoder becomes an error, so the worker keeps going.
fn run_job(decoder: &mut dyn Decoder, job: &Job, min_decode: usize) -> Result<String> {
    if job.samples().len() < min_decode {
        return Ok(String::new());
    }
    catch_unwind(AssertUnwindSafe(|| decoder.decode(job.samples())))
        .unwrap_or_else(|_| Err("the decoder panicked".into()))
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
        let started = Instant::now();
        let result = run_job(decoder, &job, min_decode);
        log::debug!(
            "preview decode of {} ms of audio took {} ms",
            job.samples().len() / ms(1),
            started.elapsed().as_millis()
        );
        let mut state = lock(preview);
        match result {
            Ok(text) => {
                if let Some(partial) = state.apply(&job, &text) {
                    partials.push(partial);
                }
            }
            Err(e) => {
                log::warn!("preview decode failed: {e}");
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
    ) -> Session {
        let interval = config.interval;
        let shared = Arc::new(Shared {
            preview: Mutex::new(Preview::new(config)),
            stop: Mutex::new(false),
            wake: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("sayso-preview".into())
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
                    for (committed, tentative) in
                        drive(&shared.preview, decoder.as_mut(), false, stop)
                    {
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
            .ok();
        if worker.is_none() {
            log::error!("cannot start the preview thread; the preview stays empty");
        }
        Session { shared, worker }
    }

    pub fn push(&self, samples: &[f32]) {
        lock(&self.shared.preview).push(samples);
    }

    /// Stop the worker (it ends after the decode that runs now, if any), then
    /// decode the new audio one last time unless `skip_decode`. Returns the
    /// final partial (None when the text did not change) and the full text.
    pub fn finish(mut self, skip_decode: bool) -> (Option<String>, String) {
        let mut decoder = self.stop_worker().filter(|_| !skip_decode);
        let preview = std::mem::replace(
            &mut *lock(&self.shared.preview),
            Preview::new(Config::default()),
        );
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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
        (0..words)
            .map(|k| format!("w{k}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Hears one word per run of non-zero samples longer than 50 ms. Records
    /// the length of every decode.
    #[derive(Clone, Default)]
    struct FakeDecoder {
        lengths: Arc<Mutex<Vec<usize>>>,
        delay: Duration,
    }

    impl Decoder for FakeDecoder {
        fn decode(&mut self, samples: &[f32]) -> Result<String> {
            self.lengths.lock().unwrap().push(samples.len());
            std::thread::sleep(self.delay);
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
            Ok(words.join(" "))
        }
    }

    /// Feed the audio in 160 ms chunks and decode whenever a job is due, without threads.
    fn run_sync(
        samples: &[f32],
        config: Config,
        decoder: &mut FakeDecoder,
    ) -> (Vec<(String, String)>, Preview) {
        let preview = Mutex::new(Preview::new(config));
        let mut partials = Vec::new();
        for chunk in samples.chunks(ms(160)) {
            lock(&preview).push(chunk);
            partials.extend(drive(&preview, decoder, false, || false));
        }
        partials.extend(drive(&preview, decoder, true, || false));
        (partials, preview.into_inner().unwrap())
    }

    #[test]
    fn short_dictation_previews_without_commits() {
        let samples = speech(8); // 4.4 s
        let mut decoder = FakeDecoder::default();
        let (partials, mut preview) = run_sync(&samples, Config::default(), &mut decoder);
        assert!(partials.len() >= 5, "{partials:?}");
        assert!(
            partials.iter().all(|(c, _)| c.is_empty()),
            "nothing is committed in 4 s"
        );
        assert_eq!(preview.text(), expected(8));
        assert_eq!(
            partials.last().map(|(_, t)| t.as_str()),
            Some(expected(8).as_str())
        );
        assert_eq!(preview.final_partial(), None, "the text went out already");
        // The end of the stream decodes the last word; that text goes out.
        preview.push(&speech(9)[speech(8).len()..]);
        let (partial, text) = finish_preview(preview, Some(&mut decoder));
        assert_eq!(text, expected(9));
        assert_eq!(partial, Some(expected(9)));
        // Partials only go out when the text changed.
        assert!(partials.windows(2).all(|w| w[0] != w[1]));
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
        let longest = decoder
            .lengths
            .lock()
            .unwrap()
            .iter()
            .copied()
            .max()
            .unwrap();
        assert!(
            longest <= config.commit_after + ms(160),
            "decodes stay bounded: {longest}"
        );
        assert!(preview.pending() <= config.commit_after);
        // `committed` only grows, and each partial is a prefix of the final text
        // up to its tentative tail.
        let mut last = String::new();
        for (committed, _) in &partials {
            assert!(
                committed.starts_with(last.as_str()),
                "{last:?} -> {committed:?}"
            );
            assert!(expected(words).starts_with(committed.as_str()));
            last = committed.clone();
        }
        assert!(partials.iter().any(|(c, t)| !c.is_empty() && !t.is_empty()));
    }

    #[test]
    fn the_cut_falls_in_silence() {
        let samples = speech(60);
        let mut preview = Preview::new(Config::default());
        preview.push(&samples[..ms(21_000)]);
        let Some(Job::Commit { cut, .. }) = preview.next_job(false) else {
            panic!("a commit is due")
        };
        assert_eq!(samples[cut], 0.0, "cut at {cut} is inside a word");
        assert!((ms(9_000)..=ms(17_000)).contains(&cut));
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
        assert_eq!(
            run_job(
                &mut decoder,
                &Job::Tail {
                    end: 10,
                    samples: vec![0.1; 10]
                },
                ms(200)
            )
            .unwrap(),
            ""
        );
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

    fn fast_config() -> Config {
        Config {
            interval: Duration::from_millis(20),
            ..Config::default()
        }
    }

    #[test]
    fn session_thread_sends_partials_and_finishes_with_the_full_text() {
        let samples = speech(10);
        let decoder = FakeDecoder {
            delay: Duration::from_millis(2),
            ..FakeDecoder::default()
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let session = Session::start(
            fast_config(),
            Box::new(decoder),
            || false,
            move |c, t| {
                tx.send(join_texts([c, t])).unwrap();
            },
        );
        for chunk in samples.chunks(ms(160)) {
            session.push(chunk);
            std::thread::sleep(Duration::from_millis(15));
        }
        let (partial, text) = session.finish(false);
        assert_eq!(text, expected(10));
        let partials: Vec<String> = rx.try_iter().collect();
        assert!(partials.len() >= 3, "{partials:?}");
        if let Some(partial) = partial {
            assert_eq!(partial, text);
            assert_ne!(partials.last(), Some(&text));
        } else {
            assert_eq!(partials.last(), Some(&text));
        }
    }

    #[test]
    fn a_paused_session_does_not_decode_and_finish_can_skip_the_last_decode() {
        let decoder = FakeDecoder::default();
        let lengths = Arc::clone(&decoder.lengths);
        let paused = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&paused);
        let session = Session::start(
            fast_config(),
            Box::new(decoder),
            move || flag.load(Ordering::SeqCst),
            |_, _| {},
        );
        session.push(&speech(4));
        std::thread::sleep(Duration::from_millis(100));
        assert!(lengths.lock().unwrap().is_empty(), "paused");
        let (partial, text) = session.finish(true);
        assert_eq!((partial, text), (None, String::new()));
        assert!(lengths.lock().unwrap().is_empty());
    }

    #[test]
    fn finish_stops_the_worker_promptly() {
        let decoder = FakeDecoder {
            delay: Duration::from_millis(50),
            ..FakeDecoder::default()
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let session = Session::start(
            fast_config(),
            Box::new(decoder),
            || false,
            move |_, _| {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        );
        session.push(&speech(6));
        std::thread::sleep(Duration::from_millis(40));
        let started = Instant::now();
        let (_, text) = session.finish(true);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
        let _ = text;
    }
}
