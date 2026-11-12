//! An audio source in JavaScript as a `Microphone` or a `SystemAudio` of `sayso-speech`
//! (`docs/sdk-typescript.md`, "API": `capture`).
//!
//! When a live session opens the source, Rust calls `open` in JavaScript with the number
//! of a writer, and JavaScript gives the audio to the writer: `writerPush`, `writerGap`,
//! and `writerEnd`. When the session no longer needs the audio, JavaScript gets `close`
//! for that writer.

use crate::bridge::Bridge;
use crate::id_of;
use crossbeam_channel::{Sender, unbounded};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use parking_lot::Mutex;
use sayso_protocol::ErrorCode;
use sayso_speech::live::{AudioFeed, CapturedAudio, FeedItem, Microphone, OpenError, SourceEnd, SystemAudio};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

static WRITERS: LazyLock<Mutex<HashMap<u32, Sender<FeedItem>>>> = LazyLock::new(Mutex::default);
static NEXT_WRITER: AtomicU32 = AtomicU32::new(1);

/// A source of the app, in JavaScript.
pub(crate) struct JsSource {
    bridge: Arc<Bridge>,
}

impl JsSource {
    pub(crate) fn new(bridge: Bridge) -> JsSource {
        JsSource { bridge: Arc::new(bridge) }
    }

    fn open_feed(&self, app_pid: Option<u32>) -> std::result::Result<AudioFeed, OpenError> {
        let (tx, items) = unbounded();
        let writer = NEXT_WRITER.fetch_add(1, Ordering::Relaxed).max(1);
        WRITERS.lock().insert(writer, tx);
        let lease = Lease { writer, bridge: self.bridge.clone() };
        match self.bridge.call("open", json!({ "writer": writer, "appPid": app_pid }).to_string(), None) {
            Ok(answer) => {
                let excludes = serde_json::from_str::<serde_json::Value>(&answer)
                    .ok()
                    .and_then(|value| value.get("excludes").cloned())
                    .and_then(|excludes| serde_json::from_value::<Vec<String>>(excludes).ok())
                    .unwrap_or_default();
                Ok(AudioFeed { items, lease: Box::new(lease), excludes })
            }
            Err(e) => {
                // The lease tells JavaScript to close a capture that it may have started.
                drop(lease);
                Err(match e.code {
                    Some(ErrorCode::Permission) => OpenError::Permission(e.message),
                    Some(ErrorCode::Unsupported) => OpenError::Unsupported(e.message),
                    _ => OpenError::Failed(e.message),
                })
            }
        }
    }
}

impl Microphone for JsSource {
    fn open(&self) -> std::result::Result<AudioFeed, OpenError> {
        self.open_feed(None)
    }
}

impl SystemAudio for JsSource {
    fn open(&self, app_pid: Option<u32>) -> std::result::Result<AudioFeed, OpenError> {
        self.open_feed(app_pid)
    }
}

/// The session holds it while it needs the audio. Its drop ends the writer.
struct Lease {
    writer: u32,
    bridge: Arc<Bridge>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if WRITERS.lock().remove(&self.writer).is_some() {
            self.bridge.notify("close", json!({ "writer": self.writer }).to_string());
        }
    }
}

fn send(writer: u32, item: FeedItem) -> bool {
    let writers = WRITERS.lock();
    writers.get(&writer).is_some_and(|tx| tx.send(item).is_ok())
}

/// 16 kHz mono samples from -1 to 1 for a writer. `time_ms` is the time of the first
/// sample on a clock that the sources of the session share. False: the writer is closed.
#[napi]
pub fn writer_push(writer: u32, samples: Float32Array, time_ms: Option<f64>) -> Result<bool> {
    let time = match time_ms {
        Some(ms) if ms.is_finite() && ms >= 0.0 => Some(Duration::from_secs_f64(ms / 1000.0)),
        Some(ms) => return Err(Error::new(Status::InvalidArg, format!("the time must be a number of milliseconds from 0, not {ms}"))),
        None => None,
    };
    Ok(send(writer, FeedItem::Audio(CapturedAudio { samples: samples.to_vec(), time })))
}

/// The source lost this much audio. False: the writer is closed.
#[napi]
pub fn writer_gap(writer: u32, ms: f64) -> Result<bool> {
    Ok(send(writer, FeedItem::Gap { ms: id_of(ms, "the gap")? }))
}

/// The source ended before the session: `permission`, or `failed` with a message.
#[napi]
pub fn writer_end(writer: u32, reason: String, message: Option<String>) {
    let end = match reason.as_str() {
        "permission" => SourceEnd::Permission,
        _ => SourceEnd::Failed(message.unwrap_or_else(|| "the audio source of the app ended".into())),
    };
    if let Some(tx) = WRITERS.lock().remove(&writer) {
        let _ = tx.send(FeedItem::Ended(end));
    }
}
