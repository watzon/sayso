//! The microphone with `cpal`: [`devices`] and [`start`].
//!
//! Pipeline in the audio callback: convert samples to f32, mix to mono,
//! resample to 16 kHz, cut into 20 ms frames, and queue each frame for the
//! worker thread. The callback allocates nothing and takes no lock once its
//! buffers are warm. Each frame gets its time on the capture clock ([`crate::clock`])
//! in the callback, so a slow `on_frame` does not move it.
//!
//! The worker thread computes the display level, copies the frame, and calls
//! `on_frame`. So `on_frame` can take locks, grow the recording, and write to
//! the engine without a stall that costs microphone samples. The queue holds
//! about 20 s of audio. If it is ever full, the callback waits for the worker.
//! It never drops a frame, because the final pass needs all of them.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, SizedSample};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError, bounded};
use sayso_speech::resample::StreamResampler;
use crate::clock;
use std::time::Duration;

/// Output sample rate for the speech engine.
pub const TARGET_RATE: u32 = 16_000;
/// Samples per frame: 20 ms at 16 kHz.
pub const FRAME_SAMPLES: usize = 320;

/// Frames that the queue between the audio callback and the worker holds: 20 s.
pub(crate) const QUEUE_FRAMES: usize = 1_024;
/// The length of a frame.
pub(crate) const FRAME_TIME: Duration = Duration::from_millis(20);
/// How long `stop` waits for the worker to finish the frames in the queue.
const WORKER_STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// Level that maps to 0.0 (dB below full scale).
const LEVEL_FLOOR_DB: f32 = -50.0;
/// dB span from floor to 1.0. Normal speech has an RMS near -26 dBFS,
/// which gives a level of about 0.67. Loud speech (-20 dBFS) gives 0.83.
const LEVEL_RANGE_DB: f32 = 36.0;

/// An input device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// 16 kHz mono samples plus a level for the waveform.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub samples: Vec<f32>,
    /// RMS of this frame, 0.0 to 1.0, scaled for display.
    pub level: f32,
    /// The time of the first sample on the capture clock ([`crate::clock`]).
    /// It comes from the arrival of the audio, so it is right to within one
    /// buffer of the device.
    pub time: Duration,
}

/// Why the capture did not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The system does not let this app use the microphone.
    PermissionDenied,
    /// The text is English.
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::PermissionDenied => f.write_str("the microphone permission is denied"),
            Error::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

/// Root mean square of a frame.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Map an RMS value to the 0.0 to 1.0 range of the waveform display.
pub fn display_level(rms: f32) -> f32 {
    if rms <= 0.0 {
        return 0.0;
    }
    ((20.0 * rms.log10() - LEVEL_FLOOR_DB) / LEVEL_RANGE_DB).clamp(0.0, 1.0)
}

/// Average interleaved channels into mono.
pub fn downmix(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    if channels <= 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    out.extend(interleaved.chunks_exact(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32));
}

/// One 20 ms frame of 16 kHz mono samples.
pub(crate) type RawFrame = [f32; FRAME_SAMPLES];

/// Collects samples and emits whole frames.
struct Framer {
    pending: Vec<f32>,
}

impl Framer {
    fn push(&mut self, samples: &[f32], mut on_frame: impl FnMut(&RawFrame)) {
        self.pending.extend_from_slice(samples);
        let (frames, _rest) = self.pending.as_chunks::<FRAME_SAMPLES>();
        for frame in frames {
            on_frame(frame);
        }
        let whole = frames.len() * FRAME_SAMPLES;
        self.pending.drain(..whole);
    }
}

/// What the callback sends to the worker.
// A box would allocate in the callback, and `Stop` is sent once.
#[allow(clippy::large_enum_variant)]
enum Message {
    Frame(RawFrame, Duration),
    /// Sent after the last frame. The worker ends.
    Stop,
}

/// The callback side of the queue.
struct FrameSink {
    tx: Sender<Message>,
    /// The worker is gone, and it was logged.
    lost: bool,
}

impl FrameSink {
    fn push(&mut self, frame: &RawFrame) {
        // A frame is a plain copy, so the queue allocates nothing.
        match self.tx.try_send(Message::Frame(*frame, clock::now().saturating_sub(FRAME_TIME))) {
            Ok(()) => {}
            Err(TrySendError::Full(message)) => {
                // The worker is far behind. Wait for it: a wait costs a glitch at worst,
                // and a dropped frame would cut a hole in the recording.
                if self.tx.send(message).is_err() {
                    self.worker_lost();
                }
            }
            Err(TrySendError::Disconnected(_)) => self.worker_lost(),
        }
    }

    fn worker_lost(&mut self) {
        if !self.lost {
            self.lost = true;
            log::error!("audio worker ended early, so the recording loses audio from now on");
        }
    }
}

/// The worker thread: it turns queued frames into `on_frame` calls, in order.
struct Worker {
    tx: Sender<Message>,
    done: Receiver<()>,
}

impl Worker {
    fn start(on_frame: Box<dyn FnMut(Frame) + Send>) -> Result<(Self, FrameSink)> {
        Self::with_capacity(QUEUE_FRAMES, on_frame)
    }

    fn with_capacity(capacity: usize, mut on_frame: Box<dyn FnMut(Frame) + Send>) -> Result<(Self, FrameSink)> {
        let (tx, rx) = bounded::<Message>(capacity);
        let (done_tx, done) = bounded::<()>(1);
        std::thread::Builder::new()
            .name("sayso-audio".into())
            .spawn(move || {
                while let Ok(Message::Frame(samples, time)) = rx.recv() {
                    let level = display_level(rms(&samples));
                    on_frame(Frame { samples: samples.to_vec(), level, time });
                }
                let _ = done_tx.send(());
            })
            .map_err(|e| Error::Failed(format!("audio worker: {e}")))?;
        Ok((Self { tx: tx.clone(), done }, FrameSink { tx, lost: false }))
    }

    /// Wait until `on_frame` has seen every queued frame. Call it after the
    /// last frame was pushed. A stuck `on_frame` does not stop the caller for
    /// longer than [`WORKER_STOP_TIMEOUT`].
    fn finish(self) {
        // The frames before this message are all in the queue: the stream is gone.
        let _ = self.tx.send(Message::Stop);
        if self.done.recv_timeout(WORKER_STOP_TIMEOUT) == Err(RecvTimeoutError::Timeout) {
            log::error!("audio worker did not finish in {} s", WORKER_STOP_TIMEOUT.as_secs());
        }
    }
}

/// Everything the audio callback owns.
pub(crate) struct Pipeline<F: FnMut(&RawFrame)> {
    channels: usize,
    resampler: StreamResampler,
    framer: Framer,
    on_frame: F,
    float: Vec<f32>,
    mono: Vec<f32>,
    resampled: Vec<f32>,
}

impl<F: FnMut(&RawFrame)> Pipeline<F> {
    pub(crate) fn new(channels: u16, in_rate: u32, on_frame: F) -> Self {
        Self {
            channels: usize::from(channels.max(1)),
            resampler: StreamResampler::new(in_rate, TARGET_RATE),
            framer: Framer { pending: Vec::with_capacity(FRAME_SAMPLES * 4) },
            on_frame,
            float: Vec::new(),
            mono: Vec::new(),
            resampled: Vec::new(),
        }
    }

    pub(crate) fn push<T: Sample>(&mut self, data: &[T])
    where
        f32: cpal::FromSample<T>,
    {
        self.float.clear();
        self.float.extend(data.iter().map(|s| s.to_sample::<f32>()));
        downmix(&self.float, self.channels, &mut self.mono);
        self.resampled.clear();
        self.resampler.process(&self.mono, &mut self.resampled);
        self.framer.push(&self.resampled, &mut self.on_frame);
    }
}

fn map_error(err: cpal::Error) -> Error {
    if err.kind() == cpal::ErrorKind::PermissionDenied {
        Error::PermissionDenied
    } else {
        Error::Failed(format!("audio input: {err}"))
    }
}

fn build_stream<T, F>(device: &cpal::Device, config: cpal::StreamConfig, mut pipeline: Pipeline<F>) -> Result<cpal::Stream>
where
    F: FnMut(&RawFrame) + Send + 'static,
    T: SizedSample,
    f32: cpal::FromSample<T>,
{
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data, _| pipeline.push(data),
            |err| log::warn!("audio input error: {err}"),
            None,
        )
        .map_err(map_error)
}

/// The device to record from: the chosen device, or the default input when
/// the chosen device is not connected (a USB microphone on a dock, for
/// example).
fn input_device(host: &cpal::Host, device_id: Option<&str>) -> Option<cpal::Device> {
    if let Some(id) = device_id {
        // Accept a device id or, for older configs, a device name.
        let chosen = id.parse().ok().and_then(|parsed| host.device_by_id(&parsed)).or_else(|| {
            host.input_devices().ok()?.find(|d| d.description().is_ok_and(|desc| desc.name() == id))
        });
        if chosen.is_some() {
            return chosen;
        }
        log::warn!("the input device {id} is not connected, so the default input device records");
    }
    host.default_input_device()
}

/// A capture that runs. Dropping it ends the capture.
pub struct Capture {
    stream: Option<cpal::Stream>,
    worker: Option<Worker>,
}

impl Drop for Capture {
    /// Stop the stream, so no callback runs, then wait for the worker to pass
    /// on the frames that are still queued. When this returns, `on_frame` has
    /// seen all the audio.
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            let _ = stream.pause();
        }
        if let Some(worker) = self.worker.take() {
            worker.finish();
        }
    }
}

/// The input devices of the default `cpal` host.
pub fn devices() -> Vec<Device> {
    let host = cpal::default_host();
    let default_id = host.default_input_device().and_then(|d| d.id().ok()).map(|id| id.to_string());
    let Ok(devices) = host.input_devices() else { return Vec::new() };
    devices
        .filter_map(|device| {
            let id = device.id().ok()?.to_string();
            let name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| id.clone());
            Some(Device { is_default: default_id.as_deref() == Some(id.as_str()), id, name })
        })
        .collect()
}

/// Start to capture `device_id`, or the default input device. `on_frame`
/// runs every 20 ms on a worker thread, not the audio callback, so a slow
/// `on_frame` delays frames but does not drop them.
///
/// On some systems a denied microphone gives silence instead of an error.
/// Check the permission of the system before the start.
pub fn start(device_id: Option<&str>, on_frame: Box<dyn FnMut(Frame) + Send>) -> Result<Capture> {
    let host = cpal::default_host();
    let device = input_device(&host, device_id).ok_or_else(|| Error::Failed("no audio input device found".into()))?;

    let supported = device.default_input_config().map_err(map_error)?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let (worker, mut sink) = Worker::start(on_frame)?;
    let pipeline = Pipeline::new(config.channels, config.sample_rate, move |frame: &RawFrame| sink.push(frame));
    // On an error below, the worker ends on its own: the pipeline and the
    // dropped `worker` held the last senders of its queue.
    let stream = match format {
        SampleFormat::F32 => build_stream::<f32, _>(&device, config, pipeline)?,
        SampleFormat::I16 => build_stream::<i16, _>(&device, config, pipeline)?,
        SampleFormat::I32 => build_stream::<i32, _>(&device, config, pipeline)?,
        SampleFormat::U16 => build_stream::<u16, _>(&device, config, pipeline)?,
        SampleFormat::I8 => build_stream::<i8, _>(&device, config, pipeline)?,
        other => return Err(Error::Failed(format!("unsupported sample format {other}"))),
    };
    stream.play().map_err(map_error)?;
    Ok(Capture { stream: Some(stream), worker: Some(worker) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_scaling_puts_speech_between_point_six_and_point_nine() {
        // RMS of normal and loud speech, in linear full scale.
        let normal = display_level(0.05); // -26 dBFS
        let loud = display_level(0.1); // -20 dBFS
        assert!((0.6..=0.9).contains(&normal), "normal speech {normal}");
        assert!((0.6..=0.9).contains(&loud), "loud speech {loud}");
        assert!(loud > normal);
    }

    #[test]
    fn level_is_zero_for_silence_and_clamped_at_one() {
        assert_eq!(display_level(0.0), 0.0);
        assert_eq!(display_level(1e-6), 0.0); // -120 dBFS
        assert_eq!(display_level(0.0031), 0.0); // below the floor: -50 dBFS
        assert_eq!(display_level(1.0), 1.0);
        assert_eq!(display_level(0.5), 1.0);
    }

    #[test]
    fn level_grows_with_rms() {
        let mut last = -1.0;
        for rms in [0.004, 0.01, 0.02, 0.05, 0.1, 0.15] {
            let level = display_level(rms);
            assert!(level > last);
            last = level;
        }
    }

    #[test]
    fn rms_of_a_square_wave_is_its_amplitude() {
        let x = [0.5, -0.5, 0.5, -0.5];
        assert!((rms(&x) - 0.5).abs() < 1e-6);
        assert_eq!(rms(&[]), 0.0);
    }

    #[test]
    fn downmix_averages_channels() {
        let mut out = Vec::new();
        downmix(&[1.0, 0.0, 0.5, 0.5, -1.0, 1.0], 2, &mut out);
        assert_eq!(out, [0.5, 0.5, 0.0]);
        downmix(&[0.1, 0.2], 1, &mut out);
        assert_eq!(out, [0.1, 0.2]);
    }

    #[test]
    fn framer_emits_20_ms_frames_and_keeps_the_rest() {
        let mut framer = Framer { pending: Vec::new() };
        let mut frames: Vec<RawFrame> = Vec::new();
        framer.push(&vec![0.1; 500], |f| frames.push(*f));
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].len(), FRAME_SAMPLES);
        framer.push(&vec![0.1; 160], |f| frames.push(*f));
        assert_eq!(frames.len(), 2);
        assert_eq!(framer.pending.len(), 20);
    }

    #[test]
    fn pipeline_turns_48k_stereo_into_16k_mono_frames() {
        let frames = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<Frame>::new()));
        let collected = frames.clone();
        let (worker, mut sink) = Worker::start(Box::new(move |f| collected.lock().push(f))).unwrap();
        let mut p = Pipeline::new(2, 48_000, move |f: &RawFrame| sink.push(f));
        // 1 second of a 440 Hz tone, stereo, in 10 ms callbacks.
        let tone: Vec<f32> = (0..48_000)
            .flat_map(|n| {
                let s = (2.0 * std::f32::consts::PI * 440.0 * n as f32 / 48_000.0).sin() * 0.07;
                [s, s]
            })
            .collect();
        for chunk in tone.chunks(960) {
            p.push(chunk);
        }
        worker.finish();
        let frames = frames.lock();
        // 1 s is 50 frames; the filter delay holds back a couple.
        assert!((47..=50).contains(&frames.len()), "{} frames", frames.len());
        assert!(frames.iter().all(|f| f.samples.len() == FRAME_SAMPLES));
        let level = frames[frames.len() / 2].level;
        assert!((0.6..=0.9).contains(&level), "level {level}");
    }

    #[test]
    fn pipeline_accepts_integer_samples() {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = count.clone();
        let (worker, mut sink) = Worker::start(Box::new(move |_| {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }))
        .unwrap();
        let mut p = Pipeline::new(1, 16_000, move |f: &RawFrame| sink.push(f));
        p.push(&vec![1000i16; 640]);
        worker.finish();
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    /// Run `samples` through the callback side in chunks of `chunk` samples, with an
    /// `on_frame` that appends to a recording as the app does. Returns the recording.
    fn record(samples: &[f32], chunk: usize, queue: usize, on_frame_delay: Duration) -> Vec<f32> {
        let recording = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<f32>::new()));
        let target = recording.clone();
        let on_frame = Box::new(move |frame: Frame| {
            std::thread::sleep(on_frame_delay);
            target.lock().extend_from_slice(&frame.samples);
        });
        let (worker, mut sink) = Worker::with_capacity(queue, on_frame).unwrap();
        // 16 kHz in and out, so the resampler passes every sample through unchanged.
        let mut callback = Pipeline::new(1, TARGET_RATE, move |f: &RawFrame| sink.push(f));
        for c in samples.chunks(chunk) {
            callback.push(c);
        }
        worker.finish();
        let recording = recording.lock();
        recording.clone()
    }

    /// A ramp, so a lost, doubled, or moved sample shows in the comparison.
    fn ramp(len: usize) -> Vec<f32> {
        (0..len).map(|n| n as f32 / 1_000_000.0).collect()
    }

    #[test]
    fn a_recording_over_thirty_seconds_keeps_every_sample() {
        // 35 s is past the 30 s that the app reserves for a recording, and past the
        // first growth of the buffer. Callbacks of 10 ms, like a 48 kHz device.
        let whole_frames = 35 * 50;
        let input = ramp(whole_frames * FRAME_SAMPLES + 123);
        let recording = record(&input, 160, QUEUE_FRAMES, Duration::ZERO);
        assert_eq!(recording.len(), whole_frames * FRAME_SAMPLES);
        assert!(recording == input[..whole_frames * FRAME_SAMPLES], "the samples differ from the input");
    }

    #[test]
    fn a_full_queue_makes_the_callback_wait_and_loses_nothing() {
        // The queue holds 2 frames and the worker takes 1 ms for each, so the
        // callback finds the queue full again and again.
        let input = ramp(300 * FRAME_SAMPLES);
        let recording = record(&input, 640, 2, Duration::from_millis(1));
        assert!(recording == input, "{} of {} samples, or changed", recording.len(), input.len());
    }

    #[test]
    fn frames_have_rising_times_from_the_callback_side() {
        let times = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<Duration>::new()));
        let collected = times.clone();
        // Later than the length of a frame, so the first time is not cut at zero.
        std::thread::sleep(FRAME_TIME * 2);
        let before = clock::now();
        let (worker, mut sink) = Worker::start(Box::new(move |f| collected.lock().push(f.time))).unwrap();
        sink.push(&[0.0; FRAME_SAMPLES]);
        std::thread::sleep(Duration::from_millis(5));
        sink.push(&[0.0; FRAME_SAMPLES]);
        worker.finish();
        let times = times.lock();
        assert_eq!(times.len(), 2);
        assert!(times[0] + FRAME_TIME >= before && times[1] > times[0], "{times:?}");
    }

    #[test]
    fn finish_returns_after_the_last_frame_reached_on_frame() {
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = seen.clone();
        let (worker, mut sink) = Worker::start(Box::new(move |_| {
            std::thread::sleep(Duration::from_millis(2));
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }))
        .unwrap();
        for _ in 0..100 {
            sink.push(&[0.0; FRAME_SAMPLES]);
        }
        worker.finish();
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 100);
    }
}
