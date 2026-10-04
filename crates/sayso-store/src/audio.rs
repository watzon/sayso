//! Audio files: 16 kHz mono, FLAC, 16-bit.
//!
//! FLAC is lossless and about half the size of WAV for speech. Encoding and
//! decoding are pure Rust (`flacenc`, `claxon`), so there is nothing to link.

use crate::error::{Result, StoreError};
use crate::{AUDIO_SAMPLE_RATE, Store};
use flacenc::component::BitRepr;
use flacenc::error::Verify;
use std::path::PathBuf;

const EXTENSION: &str = "flac";
const MIN_FLAC_SAMPLES: usize = 16;

impl Store {
    /// Encode `samples` (16 kHz mono, -1.0 to 1.0) as FLAC and return the file
    /// name. The file is written to a temporary name first, then renamed, so a
    /// crash never leaves a half-written audio file under a real name.
    pub fn save_audio(&self, samples: &[f32]) -> Result<String> {
        if samples.is_empty() {
            return Err(StoreError::InvalidInput("no audio samples".into()));
        }
        let bytes = encode_flac(samples)?;
        let name = format!("{}.{EXTENSION}", uuid::Uuid::new_v4());
        let final_path = self.audio_dir.join(&name);
        let temp_path = self.audio_dir.join(format!("{name}.part"));
        std::fs::write(&temp_path, bytes)?;
        std::fs::rename(&temp_path, &final_path)?;
        Ok(name)
    }

    /// Decode an audio file to 16 kHz mono samples between -1.0 and 1.0.
    pub fn load_audio(&self, name: &str) -> Result<Vec<f32>> {
        let bytes = std::fs::read(self.audio_path(name))?;
        decode_flac(&bytes)
    }

    /// The full path of an audio file. Only the last part of `name` counts, so
    /// a name from the database can never point outside the audio directory.
    pub fn audio_path(&self, name: &str) -> PathBuf {
        let file = std::path::Path::new(name).file_name().unwrap_or_default();
        self.audio_dir.join(file)
    }

    /// Delete one audio file. A missing file is fine. Other errors are logged,
    /// because a file that stays behind is harmless and must not block a delete.
    pub(crate) fn remove_audio_file(&self, name: &str) -> bool {
        match std::fs::remove_file(self.audio_path(name)) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => {
                log::warn!("could not delete audio file {name}: {e}");
                false
            }
        }
    }

    /// Delete every FLAC file in the audio directory, including orphans.
    pub(crate) fn remove_all_audio_files(&self) {
        let Ok(dir) = std::fs::read_dir(&self.audio_dir) else { return };
        for item in dir.flatten() {
            let path = item.path();
            let ours = path.extension().is_some_and(|e| e == EXTENSION || e == "part");
            if ours && let Err(e) = std::fs::remove_file(&path) {
                log::warn!("could not delete {}: {e}", path.display());
            }
        }
    }

    /// The number of audio files in the audio directory and their total size.
    /// Orphans count, because "Delete audio" removes them too.
    pub(crate) fn audio_use(&self) -> (usize, u64) {
        let Ok(dir) = std::fs::read_dir(&self.audio_dir) else { return (0, 0) };
        dir.flatten()
            .filter(|item| item.path().extension().is_some_and(|e| e == EXTENSION))
            .fold((0, 0), |(files, bytes), item| (files + 1, bytes + item.metadata().map_or(0, |m| m.len())))
    }
}

fn encode_flac(samples: &[f32]) -> Result<Vec<u8>> {
    let mut pcm: Vec<i32> =
        samples.iter().map(|s| (s * 32768.0).round().clamp(-32768.0, 32767.0) as i32).collect();
    // FLAC does not allow blocks shorter than 16 samples. Pad with silence (1 ms).
    if pcm.len() < MIN_FLAC_SAMPLES {
        pcm.resize(MIN_FLAC_SAMPLES, 0);
    }
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|e| StoreError::Audio(format!("encoder config: {e:?}")))?;
    let source = flacenc::source::MemSource::from_samples(&pcm, 1, 16, AUDIO_SAMPLE_RATE as usize);
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| StoreError::Audio(format!("FLAC encode failed: {e:?}")))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|e| StoreError::Audio(format!("FLAC write failed: {e:?}")))?;
    Ok(sink.as_slice().to_vec())
}

fn decode_flac(bytes: &[u8]) -> Result<Vec<f32>> {
    let mut reader = claxon::FlacReader::new(bytes).map_err(|e| StoreError::Audio(format!("FLAC header: {e}")))?;
    let info = reader.streaminfo();
    let channels = info.channels as usize;
    let scale = 1.0 / (1i64 << (info.bits_per_sample - 1)) as f32;
    let interleaved: Vec<i32> = reader
        .samples()
        .collect::<Result<_, _>>()
        .map_err(|e| StoreError::Audio(format!("FLAC decode failed: {e}")))?;
    // Files written by Sayso are mono. Average channels so foreign files still load.
    Ok(interleaved
        .chunks(channels.max(1))
        .map(|frame| frame.iter().map(|&s| s as f32 * scale).sum::<f32>() / frame.len() as f32)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::temp_store;

    fn sine(seconds: f32) -> Vec<f32> {
        let n = (AUDIO_SAMPLE_RATE as f32 * seconds) as usize;
        (0..n).map(|i| 0.6 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / AUDIO_SAMPLE_RATE as f32).sin()).collect()
    }

    #[test]
    fn flac_round_trip_stays_within_16_bit_precision() {
        let (_dir, store) = temp_store();
        let input = sine(1.5);
        let name = store.save_audio(&input).unwrap();
        assert!(name.ends_with(".flac"));
        let output = store.load_audio(&name).unwrap();
        assert_eq!(output.len(), input.len());
        let worst = input.iter().zip(&output).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst <= 0.5 / 32768.0 + 1e-6, "worst error {worst}");
    }

    #[test]
    fn flac_is_smaller_than_raw_pcm() {
        let (_dir, store) = temp_store();
        let input = sine(2.0);
        let name = store.save_audio(&input).unwrap();
        let size = std::fs::metadata(store.audio_path(&name)).unwrap().len() as usize;
        assert!(size < input.len() * 2, "FLAC {size} bytes, PCM {} bytes", input.len() * 2);
    }

    #[test]
    fn out_of_range_samples_clip_instead_of_wrapping() {
        let (_dir, store) = temp_store();
        let name = store.save_audio(&[2.0, -2.0, 0.0, 0.5]).unwrap();
        let out = store.load_audio(&name).unwrap();
        assert!(out[0] > 0.99 && out[1] < -0.99 && out[2].abs() < 1e-6);
    }

    #[test]
    fn very_short_audio_is_padded_to_the_flac_minimum() {
        let (_dir, store) = temp_store();
        let name = store.save_audio(&[0.5, -0.5]).unwrap();
        let out = store.load_audio(&name).unwrap();
        assert_eq!(out.len(), MIN_FLAC_SAMPLES);
        assert!((out[0] - 0.5).abs() < 1e-4 && (out[1] + 0.5).abs() < 1e-4);
    }

    #[test]
    fn empty_audio_and_missing_files_are_errors() {
        let (_dir, store) = temp_store();
        assert!(matches!(store.save_audio(&[]), Err(StoreError::InvalidInput(_))));
        assert!(store.load_audio("missing.flac").is_err());
    }

    #[test]
    fn audio_path_cannot_leave_the_audio_directory() {
        let (_dir, store) = temp_store();
        assert_eq!(store.audio_path("../../etc/passwd"), store.audio_dir().join("passwd"));
        assert_eq!(store.audio_path("a.flac"), store.audio_dir().join("a.flac"));
    }
}
