//! Audio playback for History: writes the samples to a temp WAV file and
//! plays it with the system's player (`afplay` on macOS), or `PlaySound` on
//! Windows. Pause stops the sound and keeps the position.

use std::io::Write;
use std::path::PathBuf;
#[cfg(not(windows))]
use std::process::{Child, Stdio};
use std::time::Instant;

const RATE: u32 = sayso_core::stt::SAMPLE_RATE;

pub struct Playing {
    pub entry: i64,
    #[cfg(not(windows))]
    child: Child,
    /// Length of the part that plays. `PlaySound` reports no end.
    #[cfg(windows)]
    length_ms: u64,
    /// `PlaySound` plays one sound per process. Only the newest `Playing`
    /// may stop it, so dropping a replaced one keeps the new sound.
    #[cfg(windows)]
    sound: u64,
    started: Instant,
    from_ms: u64,
}

impl Playing {
    /// Start playback of `samples` (16 kHz mono) at `from_ms`.
    pub fn start(entry: i64, samples: &[f32], from_ms: u64) -> Result<Self, String> {
        let skip = ((from_ms * RATE as u64) / 1000) as usize;
        let slice = samples.get(skip.min(samples.len())..).unwrap_or(&[]);
        let path = temp_path(entry);
        write_wav(&path, slice).map_err(|e| format!("Could not prepare the audio: {e}"))?;
        #[cfg(not(windows))]
        let mut player = crate::shell::audio_player().ok_or("No audio player is installed.")?;
        #[cfg(not(windows))]
        let child = player
            .arg(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Could not play the audio: {e}"))?;
        #[cfg(windows)]
        let sound = CURRENT_SOUND.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        #[cfg(windows)]
        if !crate::shell::play_wav(&path) {
            return Err("Could not play the audio.".into());
        }
        Ok(Playing {
            entry,
            #[cfg(not(windows))]
            child,
            #[cfg(windows)]
            length_ms: length_ms(slice),
            #[cfg(windows)]
            sound,
            started: Instant::now(),
            from_ms,
        })
    }

    /// The playback position now.
    pub fn position_ms(&self) -> u64 {
        self.from_ms + self.started.elapsed().as_millis() as u64
    }

    /// True when the sound ended by itself.
    #[cfg(not(windows))]
    pub fn finished(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    #[cfg(windows)]
    pub fn finished(&mut self) -> bool {
        self.started.elapsed().as_millis() as u64 >= self.length_ms
    }

    #[cfg_attr(windows, allow(unused_mut))]
    pub fn stop(mut self) -> u64 {
        let pos = self.position_ms();
        #[cfg(not(windows))]
        {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        pos
    }
}

impl Drop for Playing {
    fn drop(&mut self) {
        #[cfg(not(windows))]
        let _ = self.child.kill();
        #[cfg(windows)]
        if CURRENT_SOUND.load(std::sync::atomic::Ordering::SeqCst) == self.sound {
            crate::shell::stop_wav();
        }
    }
}

#[cfg(windows)]
static CURRENT_SOUND: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn temp_path(entry: i64) -> PathBuf {
    std::env::temp_dir().join(format!("sayso-play-{}-{entry}.wav", std::process::id()))
}

/// 16-bit PCM mono WAV.
fn write_wav(path: &std::path::Path, samples: &[f32]) -> std::io::Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    let mut f = std::fs::File::create(path)?;
    f.write_all(&out)
}

/// The length of the audio in ms.
pub fn length_ms(samples: &[f32]) -> u64 {
    samples.len() as u64 * 1000 / RATE as u64
}
