//! [`AudioCapture`] with `cpal`.
//!
//! Pipeline in the audio callback: convert samples to f32, mix to mono,
//! resample to 16 kHz, cut into 20 ms frames, compute a display level, and
//! call `on_frame`. The callback allocates nothing once its buffers are warm.

use crate::permissions::microphone_status;
use crate::resample::StreamResampler;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, SizedSample};
use sayso_platform::{AudioCapture, AudioDevice, AudioFrame, CaptureHandle, Permission, PermissionState, PlatformError, Result};

/// Output sample rate for the speech engine.
pub const TARGET_RATE: u32 = 16_000;
/// Samples per frame: 20 ms at 16 kHz.
pub const FRAME_SAMPLES: usize = 320;

/// Level that maps to 0.0 (dB below full scale).
const LEVEL_FLOOR_DB: f32 = -50.0;
/// dB span from floor to 1.0. Normal speech has an RMS near -26 dBFS,
/// which gives a level of about 0.67. Loud speech (-20 dBFS) gives 0.83.
const LEVEL_RANGE_DB: f32 = 36.0;

pub struct MacAudio;

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

/// Collects samples and emits whole frames.
struct Framer {
    pending: Vec<f32>,
}

impl Framer {
    fn push(&mut self, samples: &[f32], on_frame: &mut dyn FnMut(AudioFrame)) {
        self.pending.extend_from_slice(samples);
        let (frames, _rest) = self.pending.as_chunks::<FRAME_SAMPLES>();
        for frame in frames {
            on_frame(AudioFrame { samples: frame.to_vec(), level: display_level(rms(frame)) });
        }
        let whole = frames.len() * FRAME_SAMPLES;
        self.pending.drain(..whole);
    }
}

/// Everything the audio callback owns.
struct Pipeline {
    channels: usize,
    resampler: StreamResampler,
    framer: Framer,
    on_frame: Box<dyn FnMut(AudioFrame) + Send>,
    float: Vec<f32>,
    mono: Vec<f32>,
    resampled: Vec<f32>,
}

impl Pipeline {
    fn new(channels: u16, in_rate: u32, on_frame: Box<dyn FnMut(AudioFrame) + Send>) -> Self {
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

    fn push<T: Sample>(&mut self, data: &[T])
    where
        f32: cpal::FromSample<T>,
    {
        self.float.clear();
        self.float.extend(data.iter().map(|s| s.to_sample::<f32>()));
        downmix(&self.float, self.channels, &mut self.mono);
        self.resampled.clear();
        self.resampler.process(&self.mono, &mut self.resampled);
        self.framer.push(&self.resampled, &mut *self.on_frame);
    }
}

fn map_error(err: cpal::Error) -> PlatformError {
    if err.kind() == cpal::ErrorKind::PermissionDenied {
        PlatformError::PermissionDenied(Permission::Microphone)
    } else {
        PlatformError::Failed(format!("audio input: {err}"))
    }
}

fn build_stream<T>(device: &cpal::Device, config: cpal::StreamConfig, mut pipeline: Pipeline) -> Result<cpal::Stream>
where
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

struct Handle {
    stream: cpal::Stream,
}

impl CaptureHandle for Handle {
    fn stop(self: Box<Self>) {
        // Dropping the stream stops it.
        let _ = self.stream.pause();
    }
}

impl AudioCapture for MacAudio {
    fn devices(&self) -> Vec<AudioDevice> {
        let host = cpal::default_host();
        let default_id = host.default_input_device().and_then(|d| d.id().ok()).map(|id| id.to_string());
        let Ok(devices) = host.input_devices() else { return Vec::new() };
        devices
            .filter_map(|device| {
                let id = device.id().ok()?.to_string();
                let name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| id.clone());
                Some(AudioDevice { is_default: default_id.as_deref() == Some(id.as_str()), id, name })
            })
            .collect()
    }

    fn start(
        &self,
        device_id: Option<&str>,
        on_frame: Box<dyn FnMut(AudioFrame) + Send>,
    ) -> Result<Box<dyn CaptureHandle>> {
        // A denied microphone gives silence, not an error, so check first.
        if microphone_status() == PermissionState::Denied {
            return Err(PlatformError::PermissionDenied(Permission::Microphone));
        }
        let host = cpal::default_host();
        let device = match device_id {
            // Accept a device id or, for older configs, a device name.
            Some(id) => id.parse().ok().and_then(|parsed| host.device_by_id(&parsed)).or_else(|| {
                host.input_devices().ok()?.find(|d| d.description().is_ok_and(|desc| desc.name() == id))
            }),
            None => host.default_input_device(),
        }
        .ok_or_else(|| PlatformError::Failed("no audio input device found".into()))?;

        let supported = device.default_input_config().map_err(map_error)?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let pipeline = Pipeline::new(config.channels, config.sample_rate, on_frame);
        let stream = match format {
            SampleFormat::F32 => build_stream::<f32>(&device, config, pipeline)?,
            SampleFormat::I16 => build_stream::<i16>(&device, config, pipeline)?,
            SampleFormat::I32 => build_stream::<i32>(&device, config, pipeline)?,
            SampleFormat::U16 => build_stream::<u16>(&device, config, pipeline)?,
            SampleFormat::I8 => build_stream::<i8>(&device, config, pipeline)?,
            other => return Err(PlatformError::Failed(format!("unsupported sample format {other}"))),
        };
        stream.play().map_err(map_error)?;
        Ok(Box::new(Handle { stream }))
    }
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
        let mut frames: Vec<AudioFrame> = Vec::new();
        framer.push(&vec![0.1; 500], &mut |f| frames.push(f));
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].samples.len(), FRAME_SAMPLES);
        framer.push(&vec![0.1; 160], &mut |f| frames.push(f));
        assert_eq!(frames.len(), 2);
        assert_eq!(framer.pending.len(), 20);
    }

    #[test]
    fn pipeline_turns_48k_stereo_into_16k_mono_frames() {
        let frames = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<AudioFrame>::new()));
        let sink = frames.clone();
        let mut p = Pipeline::new(2, 48_000, Box::new(move |f| sink.lock().push(f)));
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
        let mut p = Pipeline::new(1, 16_000, Box::new(move |_| {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        p.push(&vec![1000i16; 640]);
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}
