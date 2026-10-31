//! WASAPI loopback of the default output device, with `cpal`: an output device
//! as an input stream is a loopback stream (`cpal::host::wasapi`).
//!
//! Loopback needs no permission of the system. It captures the mix of the
//! device, so it cannot leave out a process. Application loopback
//! (`AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS`, Windows 10 build 20348) can leave out
//! one process tree, and this code does not use it (`docs/sdk.md`, "System audio").

use super::{Backend, Error, FrameSink, Stream};
use crate::mic::{Pipeline, RawFrame};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, SizedSample};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) struct Loopback;

struct LoopbackStream {
    _stream: cpal::Stream,
    host: cpal::Host,
    device: Option<cpal::DeviceId>,
    broken: Arc<AtomicBool>,
}

impl Stream for LoopbackStream {
    fn still_good(&mut self) -> bool {
        !self.broken.load(Ordering::SeqCst) && default_id(&self.host) == self.device
    }
}

fn default_id(host: &cpal::Host) -> Option<cpal::DeviceId> {
    host.default_output_device().and_then(|device| device.id().ok())
}

fn map_error(err: cpal::Error) -> Error {
    if err.kind() == cpal::ErrorKind::PermissionDenied { Error::PermissionDenied } else { Error::Failed(format!("system audio: {err}")) }
}

fn build<T, F>(device: &cpal::Device, config: cpal::StreamConfig, mut pipeline: Pipeline<F>, broken: Arc<AtomicBool>) -> Result<cpal::Stream, Error>
where
    F: FnMut(&RawFrame) + Send + 'static,
    T: SizedSample,
    f32: cpal::FromSample<T>,
    T: Sample,
{
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data, _| pipeline.push(data),
            move |err| {
                if err.kind() != cpal::ErrorKind::Xrun {
                    log::warn!("system audio: {err}");
                    broken.store(true, Ordering::SeqCst);
                }
            },
            None,
        )
        .map_err(map_error)
}

impl Backend for Loopback {
    fn prepare(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn permission_held(&self) -> bool {
        true
    }

    fn open(&mut self, mut sink: FrameSink) -> Result<Box<dyn Stream>, Error> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| Error::Failed("no audio output device found".into()))?;
        let supported = device.default_output_config().map_err(map_error)?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let broken = Arc::new(AtomicBool::new(false));
        let pipeline = Pipeline::new(config.channels, config.sample_rate, move |frame: &RawFrame| sink.push(frame));
        let flag = broken.clone();
        let stream = match format {
            SampleFormat::F32 => build::<f32, _>(&device, config, pipeline, flag)?,
            SampleFormat::I16 => build::<i16, _>(&device, config, pipeline, flag)?,
            SampleFormat::I32 => build::<i32, _>(&device, config, pipeline, flag)?,
            SampleFormat::U16 => build::<u16, _>(&device, config, pipeline, flag)?,
            other => return Err(Error::Failed(format!("unsupported sample format {other}"))),
        };
        stream.play().map_err(map_error)?;
        let device = device.id().ok();
        Ok(Box::new(LoopbackStream { _stream: stream, host, device, broken }))
    }

    fn excludes(&self) -> Vec<String> {
        Vec::new()
    }
}
