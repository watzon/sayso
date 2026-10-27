//! WAV files for the engine: 16 kHz, mono, 16-bit PCM, the format that
//! `transcribe`, `vad`, and `diarize` read.

use sayso_protocol::pcm::SAMPLE_RATE;
use std::io::{self, Write};
use std::path::Path;
use tempfile::TempPath;

/// The bytes of a WAV file with these 16-bit samples.
pub fn encode_i16(samples: &[i16]) -> Vec<u8> {
    let data_len = samples.len() as u32 * 2;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes()); // size of the fmt chunk
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // bytes for each second
    bytes.extend_from_slice(&2u16.to_le_bytes()); // bytes for each frame
    bytes.extend_from_slice(&16u16.to_le_bytes()); // bits for each sample
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// The 16-bit samples for samples from -1 to 1. A sample out of that range is clipped.
pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples.iter().map(|&s| (s * 32768.0).round().clamp(-32768.0, 32767.0) as i16).collect()
}

/// Write a WAV file with 16-bit samples at 16 kHz.
pub fn write_i16(path: impl AsRef<Path>, samples: &[i16]) -> io::Result<()> {
    std::fs::write(path, encode_i16(samples))
}

/// Write a WAV file with samples from -1 to 1 at 16 kHz.
pub fn write_f32(path: impl AsRef<Path>, samples: &[f32]) -> io::Result<()> {
    write_i16(path, &f32_to_i16(samples))
}

/// A WAV file in the temporary folder. The file is removed when this value is dropped.
#[derive(Debug)]
pub struct TempWav {
    path: TempPath,
}

impl TempWav {
    pub fn from_i16(samples: &[i16]) -> io::Result<TempWav> {
        let mut file = tempfile::Builder::new().prefix("sayso-engine-sdk-").suffix(".wav").tempfile()?;
        file.write_all(&encode_i16(samples))?;
        file.flush()?;
        // Close the handle, so the engine can read the file on every system.
        Ok(TempWav { path: file.into_temp_path() })
    }

    pub fn from_f32(samples: &[f32]) -> io::Result<TempWav> {
        TempWav::from_i16(&f32_to_i16(samples))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_describes_16_khz_mono_16_bit_pcm() {
        let bytes = encode_i16(&[1, -2, 3]);
        assert_eq!(bytes.len(), 44 + 6);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 36 + 6, "the size after the first 8 bytes");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes(bytes[20..22].try_into().unwrap()), 1, "PCM");
        assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 1, "mono");
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(bytes[28..32].try_into().unwrap()), 32_000);
        assert_eq!(u16::from_le_bytes(bytes[32..34].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(bytes[34..36].try_into().unwrap()), 16);
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 6);
        assert_eq!(&bytes[44..], &[1, 0, 0xfe, 0xff, 3, 0]);
    }

    #[test]
    fn floats_are_scaled_and_clipped() {
        assert_eq!(f32_to_i16(&[0.0, 0.5, -0.5, 2.0, -2.0]), vec![0, 16384, -16384, i16::MAX, i16::MIN]);
    }

    #[test]
    fn a_temporary_file_is_removed_on_drop() {
        let wav = TempWav::from_f32(&[0.0; 160]).unwrap();
        let path = wav.path().to_path_buf();
        assert_eq!(std::fs::read(&path).unwrap().len(), 44 + 320);
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("wav"));
        drop(wav);
        assert!(!path.exists());
    }
}
