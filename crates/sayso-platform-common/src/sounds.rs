//! [`SoundPlayer`] with `rodio`.
//!
//! Opening an output device can take a moment. So a worker thread owns the
//! output stream, opens it on the first sound, and caches the WAV bytes.
//! `play` only sends a message and never blocks.

use crossbeam_channel::Sender;
use rodio::{DeviceSinkBuilder, MixerDeviceSink};
use sayso_platform::{SoundKind, SoundPlayer};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// How long the worker keeps the output device open after the last sound.
const IDLE_CLOSE: Duration = Duration::from_secs(30);

pub struct RodioSounds {
    tx: Sender<(SoundKind, f32)>,
}

/// File name of each sound inside the sounds directory.
pub fn file_name(sound: SoundKind) -> &'static str {
    match sound {
        SoundKind::Start => "start.wav",
        SoundKind::Stop => "stop.wav",
        SoundKind::Cancel => "cancel.wav",
        SoundKind::Insert => "insert.wav",
    }
}

impl RodioSounds {
    pub fn new(sounds_dir: PathBuf) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded::<(SoundKind, f32)>();
        let spawned = std::thread::Builder::new().name("sayso-sounds".into()).spawn(move || {
            let mut worker = Worker { dir: sounds_dir, cache: HashMap::new(), sink: None };
            // Wait for work. Close the device after a quiet period.
            loop {
                match rx.recv_timeout(IDLE_CLOSE) {
                    Ok((sound, volume)) => worker.play(sound, volume),
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => worker.close_if_idle(),
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
                // Drain bursts without waiting.
                while let Ok((sound, volume)) = rx.try_recv() {
                    worker.play(sound, volume);
                }
            }
        });
        if let Err(err) = spawned {
            log::error!("cannot start the sound thread: {err}");
        }
        Self { tx }
    }
}

impl SoundPlayer for RodioSounds {
    fn play(&self, sound: SoundKind, volume: f32) {
        let _ = self.tx.send((sound, volume.clamp(0.0, 1.0)));
    }
}

struct Worker {
    dir: PathBuf,
    cache: HashMap<&'static str, Arc<[u8]>>,
    sink: Option<MixerDeviceSink>,
}

impl Worker {
    fn bytes(&mut self, sound: SoundKind) -> Option<Arc<[u8]>> {
        let name = file_name(sound);
        if let Some(bytes) = self.cache.get(name) {
            return Some(bytes.clone());
        }
        match read(&self.dir, name) {
            Ok(bytes) => {
                self.cache.insert(name, bytes.clone());
                Some(bytes)
            }
            Err(err) => {
                log::warn!("cannot load sound {name}: {err}");
                None
            }
        }
    }

    fn play(&mut self, sound: SoundKind, volume: f32) {
        let Some(bytes) = self.bytes(sound) else { return };
        if self.sink.is_none() {
            match DeviceSinkBuilder::open_default_sink() {
                Ok(mut sink) => {
                    sink.log_on_drop(false);
                    self.sink = Some(sink);
                }
                Err(err) => return log::warn!("cannot open the audio output: {err}"),
            }
        }
        let Some(sink) = &self.sink else { return };
        match rodio::play(sink.mixer(), Cursor::new(bytes)) {
            Ok(player) => {
                player.set_volume(volume);
                // Let it finish on its own.
                player.detach();
            }
            Err(err) => log::warn!("cannot play {}: {err}", file_name(sound)),
        }
    }

    fn close_if_idle(&mut self) {
        self.sink = None;
    }
}

fn read(dir: &Path, name: &str) -> std::io::Result<Arc<[u8]>> {
    Ok(std::fs::read(dir.join(name))?.into())
}
