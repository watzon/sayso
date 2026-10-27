//! The pipeline of one source of a live session.
//!
//! Two threads:
//!
//! - **intake** takes the audio as it arrives. It writes the recording,
//!   sends the audio to the preview stream and to the VAD, sends `partial`
//!   events, and cuts a part of audio for each speech that ends. It never
//!   waits for a final pass, so the audio does not wait either.
//! - **final** runs the final pass on each part, one at a time, and sends a
//!   `segment` event for each segment, with times from the start of the
//!   session. Before each part it waits while a dictation runs ([`Priority`]).
//!
//! The source stops when its audio channel closes: the intake hears the rest
//! of the audio, closes the preview stream and the VAD session, cuts the last
//! part, and waits for the final thread.

use super::engine::LiveEngine;
use super::wav::WavWriter;
use super::Priority;
use crate::jobs::EventSink;
use crossbeam_channel::{Receiver, Sender, never, select, unbounded};
use sayso_core::basic_vad::BasicVad;
use sayso_core::models::ModelId;
use sayso_core::stt::EngineEvent;
use sayso_protocol::{Event, PartialEvent, Segment, SegmentEvent, Source};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const RATE: u64 = sayso_protocol::pcm::SAMPLE_RATE as u64;
/// Audio before and after the speech of a part, so the first and last words
/// are complete. The audio before a part does not reach into the part before it.
pub(crate) const PAD_MS: u64 = 200;
/// While nobody speaks, the intake keeps this much audio, for a speech start
/// that the engine reports late.
const LOOKBACK_MS: u64 = 10_000;

/// How a source finds speech.
pub(crate) enum SourceVad {
    /// Basic VAD, in the host.
    Basic,
    /// A VAD session of the engine, with this engine session id.
    Engine { session: u64 },
}

/// Everything that a source needs, after the session opened its engine sessions.
pub(crate) struct SourcePlan {
    pub session: u64,
    pub source: Source,
    pub final_model: ModelId,
    pub language: String,
    /// The engine session id of the preview stream, when there is one.
    pub preview: Option<u64>,
    pub vad: SourceVad,
    pub wav: WavWriter,
}

/// What a source gives when it ends.
pub(crate) struct SourceResult {
    pub source: Source,
    pub segments: Vec<Segment>,
    pub wav: PathBuf,
    /// Why the recording is not complete, when it is not.
    pub wav_error: Option<String>,
}

/// A source that runs.
pub(crate) struct SourceRun {
    intake: JoinHandle<SourceResult>,
}

impl SourceRun {
    /// Start the threads. `events` must exist before the engine sessions
    /// opened, so no event is lost. `cancel` stops the final passes that wait.
    pub fn start(plan: SourcePlan, audio: Receiver<Vec<f32>>, events: Receiver<EngineEvent>, engine: Arc<dyn LiveEngine>, sink: Arc<dyn EventSink>, priority: Arc<dyn Priority>, cancel: Arc<AtomicBool>) -> std::io::Result<SourceRun> {
        let (parts, waiting) = unbounded::<PartAudio>();
        let final_pass = {
            let worker = FinalPass { session: plan.session, source: plan.source.clone(), model: plan.final_model.clone(), language: plan.language.clone(), engine: engine.clone(), sink: sink.clone(), priority, cancel };
            std::thread::Builder::new().name("sayso-host-live-final".into()).spawn(move || worker.run(&waiting))?
        };
        let intake = Intake::new(plan, engine, sink, parts);
        let intake = std::thread::Builder::new().name("sayso-host-live-audio".into()).spawn(move || intake.run(&audio, events, final_pass))?;
        Ok(SourceRun { intake })
    }

    /// Wait until the source ended. Its audio channel must be closed first.
    pub fn join(self) -> Option<SourceResult> {
        self.intake.join().ok()
    }
}

/// The audio of one part, for the final pass.
struct PartAudio {
    /// The time of the first sample, from the start of the session.
    offset_ms: u64,
    samples: Vec<f32>,
}

struct Intake {
    session: u64,
    source: Source,
    preview: Option<u64>,
    vad: VadState,
    engine: Arc<dyn LiveEngine>,
    sink: Arc<dyn EventSink>,
    wav: Option<WavWriter>,
    wav_path: PathBuf,
    wav_error: Option<String>,
    /// The audio that a part can still need: `kept` starts at sample `kept_from`.
    kept: Vec<f32>,
    kept_from: u64,
    /// All samples heard.
    heard: u64,
    /// The end of the last part, in ms.
    last_end_ms: u64,
    parts: Sender<PartAudio>,
}

enum VadState {
    Basic(Box<BasicVad>),
    /// The engine session, and the start of the speech that is open.
    Engine { session: u64, open: Option<u64> },
}

impl Intake {
    fn new(plan: SourcePlan, engine: Arc<dyn LiveEngine>, sink: Arc<dyn EventSink>, parts: Sender<PartAudio>) -> Intake {
        let vad = match plan.vad {
            SourceVad::Basic => VadState::Basic(Box::new(BasicVad::new())),
            SourceVad::Engine { session } => VadState::Engine { session, open: None },
        };
        Intake {
            session: plan.session,
            source: plan.source,
            preview: plan.preview,
            vad,
            engine,
            sink,
            wav_path: plan.wav.path().to_path_buf(),
            wav: Some(plan.wav),
            wav_error: None,
            kept: Vec::new(),
            kept_from: 0,
            heard: 0,
            last_end_ms: 0,
            parts,
        }
    }

    fn run(mut self, audio: &Receiver<Vec<f32>>, events: Receiver<EngineEvent>, final_pass: JoinHandle<Vec<Segment>>) -> SourceResult {
        let mut events = events;
        let mut last_trim = Instant::now();
        loop {
            select! {
                recv(audio) -> samples => match samples {
                    Ok(samples) => self.hear(&samples),
                    Err(_) => break,
                },
                recv(events) -> event => match event {
                    Ok(event) => self.event(event),
                    Err(_) => events = never(),
                },
            }
            if last_trim.elapsed() >= Duration::from_secs(1) {
                self.trim_idle();
                last_trim = Instant::now();
            }
        }
        self.finish(&events);
        // The final thread ends when the last part is done.
        let Intake { source, wav_path, wav_error, parts, .. } = self;
        drop(parts);
        let segments = final_pass.join().unwrap_or_default();
        SourceResult { source, segments, wav: wav_path, wav_error }
    }

    fn hear(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        if let Some(wav) = &mut self.wav
            && let Err(e) = wav.write(samples)
        {
            log::warn!("host: live session {}: cannot write {}: {e}", self.session, self.wav_path.display());
            self.wav_error = Some(format!("cannot write {}: {e}", self.wav_path.display()));
            self.wav = None;
        }
        self.kept.extend_from_slice(samples);
        self.heard += samples.len() as u64;
        if let Some(preview) = self.preview
            && let Err(e) = self.engine.push_audio(preview, samples)
        {
            log::debug!("host: live session {}: preview audio: {e}", self.session);
        }
        match &mut self.vad {
            VadState::Basic(vad) => {
                for part in vad.accept(samples) {
                    self.cut(part.start_ms, part.end_ms);
                }
            }
            VadState::Engine { session, .. } => {
                if let Err(e) = self.engine.push_audio(*session, samples) {
                    log::warn!("host: live session {}: VAD audio: {e}", self.session);
                }
            }
        }
    }

    fn event(&mut self, event: EngineEvent) {
        match event {
            EngineEvent::Partial { session, committed, tentative } if Some(session) == self.preview => {
                self.sink.event(Event::Partial(PartialEvent { session: self.session, source: Some(self.source.clone()), committed, tentative }));
            }
            EngineEvent::SpeechStart { session, start_ms } => {
                if let VadState::Engine { session: vad, open } = &mut self.vad
                    && *vad == session
                {
                    *open = Some(start_ms);
                }
            }
            EngineEvent::SpeechEnd { session, start_ms, end_ms } => {
                if let VadState::Engine { session: vad, open } = &mut self.vad
                    && *vad == session
                {
                    *open = None;
                    self.cut(start_ms, end_ms);
                }
            }
            _ => {}
        }
    }

    /// A speech from `start_ms` to `end_ms` ended: its audio goes to the final pass.
    fn cut(&mut self, start_ms: u64, end_ms: u64) {
        let start_ms = start_ms.max(self.last_end_ms);
        if end_ms <= start_ms {
            return;
        }
        let pad_before = PAD_MS.min(start_ms - self.last_end_ms);
        let from = sample_at(start_ms - pad_before).max(self.kept_from);
        let to = sample_at(end_ms + PAD_MS).min(self.heard);
        if from >= to {
            log::debug!("host: live session {}: the audio of the part {start_ms}-{end_ms} ms is gone", self.session);
            return;
        }
        let samples = self.kept[(from - self.kept_from) as usize..(to - self.kept_from) as usize].to_vec();
        self.last_end_ms = end_ms;
        // A later part starts at the end of this one or after it.
        self.drop_before(sample_at(end_ms));
        let _ = self.parts.send(PartAudio { offset_ms: from * 1000 / RATE, samples });
    }

    /// While nobody speaks, forget the audio older than [`LOOKBACK_MS`].
    fn trim_idle(&mut self) {
        let keep_from = match &self.vad {
            VadState::Basic(vad) if vad.speaking() => return,
            VadState::Engine { open: Some(start_ms), .. } => sample_at(start_ms.saturating_sub(PAD_MS)),
            _ => self.heard.saturating_sub(sample_at(LOOKBACK_MS)),
        };
        self.drop_before(keep_from.max(sample_at(self.last_end_ms)));
    }

    fn drop_before(&mut self, sample: u64) {
        let sample = sample.min(self.heard);
        if sample > self.kept_from {
            self.kept.drain(..(sample - self.kept_from) as usize);
            self.kept_from = sample;
        }
    }

    /// The audio ended: close the engine sessions, cut the last part, and close the recording.
    fn finish(&mut self, events: &Receiver<EngineEvent>) {
        if let Some(preview) = self.preview {
            self.engine.end_stream(preview);
        }
        match std::mem::replace(&mut self.vad, VadState::Engine { session: 0, open: None }) {
            VadState::Basic(vad) => {
                // The partials that came already. The host does not wait for
                // the last partial of the engine: the segments have the text.
                for event in events.try_iter() {
                    self.event(event);
                }
                for part in vad.finish() {
                    self.cut(part.start_ms, part.end_ms);
                }
            }
            VadState::Engine { session, open } => {
                self.vad = VadState::Engine { session, open };
                // The engine sends the last `speech_end` before its reply, so
                // it waits in the channel now. A backend that cannot wait for
                // the reply leaves the speech open: it ends at the end of the audio.
                if let Err(e) = self.engine.end_vad(session) {
                    log::warn!("host: live session {}: the end of the VAD session failed: {e}", self.session);
                }
                for event in events.try_iter() {
                    self.event(event);
                }
                if let VadState::Engine { open: Some(start_ms), .. } = self.vad {
                    self.cut(start_ms, self.heard * 1000 / RATE);
                }
            }
        }
        if let Some(wav) = self.wav.take()
            && let Err(e) = wav.finish()
        {
            log::warn!("host: live session {}: cannot close {}: {e}", self.session, self.wav_path.display());
            self.wav_error = Some(format!("cannot write {}: {e}", self.wav_path.display()));
        }
    }
}

fn sample_at(ms: u64) -> u64 {
    ms * RATE / 1000
}

struct FinalPass {
    session: u64,
    source: Source,
    model: ModelId,
    language: String,
    engine: Arc<dyn LiveEngine>,
    sink: Arc<dyn EventSink>,
    priority: Arc<dyn Priority>,
    cancel: Arc<AtomicBool>,
}

impl FinalPass {
    fn run(self, parts: &Receiver<PartAudio>) -> Vec<Segment> {
        let mut all = Vec::new();
        for part in parts.iter() {
            let cancelled = || self.cancel.load(Ordering::SeqCst);
            if cancelled() || !self.priority.wait_idle(&cancelled) {
                continue;
            }
            let call = self.priority.engine_call();
            let transcribed = self.engine.transcribe_segments(&part.samples, &self.model, &self.language);
            drop(call);
            let segments = match transcribed {
                Ok(segments) => segments,
                Err(e) => {
                    log::warn!("host: live session {}: the final pass of a part failed: {e}", self.session);
                    continue;
                }
            };
            for segment in segments {
                let segment = shift(segment, part.offset_ms);
                if segment.text.trim().is_empty() {
                    continue;
                }
                self.sink.event(Event::Segment(SegmentEvent { job: None, session: Some(self.session), source: Some(self.source.clone()), segment: segment.clone() }));
                all.push(segment);
            }
        }
        all
    }
}

/// A segment with times from the start of the session.
fn shift(mut segment: Segment, offset_ms: u64) -> Segment {
    segment.start_ms += offset_ms;
    segment.end_ms += offset_ms;
    for word in segment.words.iter_mut().flatten() {
        word.start_ms += offset_ms;
        word.end_ms += offset_ms;
    }
    segment
}
