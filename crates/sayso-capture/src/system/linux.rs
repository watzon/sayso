//! The monitor of the default sink, through the native protocol of PulseAudio,
//! which PipeWire also serves (`pipewire-pulse`).
//!
//! The monitor is the mix of the sink, so the capture cannot leave out a
//! process (`docs/sdk.md`, "System audio"). The server resamples to 16 kHz mono.
//! There is no permission on the desktop. A Flatpak needs access to the socket
//! of the server, and a refusal of the server gives [`Error::PermissionDenied`].

use super::{Backend, Error, FrameSink, Stream};
use crate::mic::{FRAME_SAMPLES, Pipeline, RawFrame, TARGET_RATE};
use futures::executor::block_on;
use pulseaudio::{Client, ClientError, protocol};
use std::ffi::CString;
use std::sync::mpsc;
use std::time::Duration;

/// A server that hangs does not stop `session_start` for longer than this.
const SERVER_TIMEOUT: Duration = Duration::from_secs(5);
/// One frame in the format of the stream.
const FRAME_BYTES: u32 = (FRAME_SAMPLES * size_of::<f32>()) as u32;

pub(super) struct Monitor;

struct MonitorStream {
    client: Client,
    sink_name: Option<CString>,
    _stream: pulseaudio::RecordStream,
}

impl Stream for MonitorStream {
    fn still_good(&mut self) -> bool {
        match with_timeout(block_server_info(self.client.clone())) {
            Ok(info) => info.default_sink_name == self.sink_name,
            Err(error) => {
                log::warn!("system audio: {error}");
                false
            }
        }
    }
}

fn block_server_info(client: Client) -> impl FnOnce() -> Result<protocol::ServerInfo, Error> + Send + 'static {
    move || block_on(client.server_info()).map_err(map_error)
}

/// Run a call of the server on its own thread, with a limit for the wait.
fn with_timeout<T: Send + 'static>(call: impl FnOnce() -> Result<T, Error> + Send + 'static) -> Result<T, Error> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("sayso-system-audio-server".into())
        .spawn(move || {
            let _ = tx.send(call());
        })
        .map_err(|e| Error::Failed(format!("the thread of the sound server: {e}")))?;
    rx.recv_timeout(SERVER_TIMEOUT).map_err(|_| Error::Failed("the sound server gave no reply".into()))?
}

fn map_error(err: ClientError) -> Error {
    match err {
        ClientError::ServerError(protocol::PulseError::AccessDenied | protocol::PulseError::AuthKey) => Error::PermissionDenied,
        // The reply to the first message of a connection comes as a protocol error.
        ClientError::Protocol(protocol::ProtocolError::ServerError(protocol::PulseError::AccessDenied | protocol::PulseError::AuthKey)) => Error::PermissionDenied,
        ClientError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Error::PermissionDenied,
        other => Error::Failed(format!("the sound server: {other}")),
    }
}

/// The samples of the stream: little-endian `f32`.
fn samples(bytes: &[u8], out: &mut Vec<f32>) {
    out.clear();
    out.extend(bytes.chunks_exact(4).map(|sample| f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]])));
}

fn open(mut sink: FrameSink) -> Result<MonitorStream, Error> {
    let client = Client::from_env(c"Sayso").map_err(map_error)?;
    let info = block_on(client.server_info()).map_err(map_error)?;
    let sink_name = info.default_sink_name.clone();
    let name = sink_name.clone().ok_or_else(|| Error::Failed("the sound server has no default sink".into()))?;
    let sink_info = block_on(client.sink_info_by_name(name)).map_err(map_error)?;
    let params = protocol::RecordStreamParams {
        sample_spec: protocol::SampleSpec { format: protocol::SampleFormat::Float32Le, channels: 1, sample_rate: TARGET_RATE },
        channel_map: protocol::ChannelMap::mono(),
        source_index: sink_info.monitor_source_index,
        // The default is pieces of up to 2 s. Each frame gets the time of its arrival, so ask for pieces of one frame.
        buffer_attr: protocol::stream::BufferAttr { fragment_size: FRAME_BYTES, ..Default::default() },
        flags: protocol::stream::StreamFlags { adjust_latency: true, ..Default::default() },
        ..Default::default()
    };
    let mut pipeline = Pipeline::new(1, TARGET_RATE, move |frame: &RawFrame| sink.push(frame));
    let mut decoded = Vec::new();
    let callback = move |bytes: &[u8]| {
        samples(bytes, &mut decoded);
        pipeline.push(&decoded);
    };
    let stream = block_on(client.create_record_stream(params, callback)).map_err(map_error)?;
    Ok(MonitorStream { client, sink_name, _stream: stream })
}

impl Backend for Monitor {
    fn prepare(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn permission_held(&self) -> bool {
        true
    }

    fn open(&mut self, sink: FrameSink) -> Result<Box<dyn Stream>, Error> {
        Ok(Box::new(with_timeout(move || open(sink))?))
    }

    fn excludes(&self) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_of_the_server_is_the_permission_and_other_errors_are_not() {
        let refused = [
            ClientError::ServerError(protocol::PulseError::AccessDenied),
            ClientError::Protocol(protocol::ProtocolError::ServerError(protocol::PulseError::AccessDenied)),
            ClientError::Io(std::io::ErrorKind::PermissionDenied.into()),
        ];
        for error in refused {
            assert!(matches!(map_error(error), Error::PermissionDenied));
        }
        assert!(matches!(map_error(ClientError::ServerUnavailable), Error::Failed(_)));
    }

    #[test]
    fn the_bytes_of_the_stream_become_samples() {
        let mut out = Vec::new();
        let bytes: Vec<u8> = [0.5f32, -0.25].iter().flat_map(|s| s.to_le_bytes()).chain([1, 2]).collect();
        samples(&bytes, &mut out);
        assert_eq!(out, [0.5, -0.25], "a sample that is not complete is left out");
    }
}
