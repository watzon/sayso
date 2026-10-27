//! Audio in messages: base64 of 16 kHz mono little-endian 16-bit PCM.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// The sample rate of all audio in the protocol.
pub const SAMPLE_RATE: u32 = 16_000;

/// The text for `pcm` from 16-bit samples.
pub fn encode_i16(samples: &[i16]) -> String {
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    STANDARD.encode(bytes)
}

/// The text for `pcm` from samples from -1 to 1. A sample out of that range
/// is clipped.
pub fn encode_f32(samples: &[f32]) -> String {
    let samples: Vec<i16> = samples.iter().map(|&s| to_i16(s)).collect();
    encode_i16(&samples)
}

/// The 16-bit samples of a `pcm` text.
pub fn decode_i16(pcm: &str) -> Result<Vec<i16>, PcmError> {
    let bytes = STANDARD.decode(pcm).map_err(|e| PcmError(format!("pcm is not base64: {e}")))?;
    if bytes.len() % 2 != 0 {
        return Err(PcmError(format!("pcm has {} bytes, but 16-bit samples need an even number", bytes.len())));
    }
    Ok(bytes.as_chunks::<2>().0.iter().map(|&pair| i16::from_le_bytes(pair)).collect())
}

/// The samples of a `pcm` text, from -1 to 1.
pub fn decode_f32(pcm: &str) -> Result<Vec<f32>, PcmError> {
    Ok(decode_i16(pcm)?.into_iter().map(|s| f32::from(s) / 32768.0).collect())
}

// The same scale as `decode_f32`, so a sample goes back to the same i16.
fn to_i16(sample: f32) -> i16 {
    (sample * 32768.0).round().clamp(-32768.0, 32767.0) as i16
}

/// A `pcm` text that is not base64 of 16-bit samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmError(String);

impl std::fmt::Display for PcmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PcmError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_little_endian_base64() {
        // 1 = 01 00, -2 = fe ff.
        assert_eq!(encode_i16(&[1, -2]), STANDARD.encode([0x01, 0x00, 0xfe, 0xff]));
        assert_eq!(decode_i16(&encode_i16(&[1, -2, i16::MAX, i16::MIN])).unwrap(), vec![1, -2, i16::MAX, i16::MIN]);
        assert_eq!(decode_i16("").unwrap(), Vec::<i16>::new());
    }

    #[test]
    fn floats_go_back_to_the_same_samples() {
        let samples: Vec<i16> = vec![0, 1, -1, 1000, -1000, i16::MAX, i16::MIN];
        let floats = decode_f32(&encode_i16(&samples)).unwrap();
        assert_eq!(decode_i16(&encode_f32(&floats)).unwrap(), samples);
        assert_eq!(decode_i16(&encode_f32(&[2.0, -2.0, 0.5])).unwrap(), vec![i16::MAX, i16::MIN, 16384], "clipped");
    }

    #[test]
    fn a_bad_pcm_text_is_an_error() {
        assert!(decode_i16("not base64!").is_err());
        assert!(decode_i16(&STANDARD.encode([1, 2, 3])).is_err(), "an odd number of bytes");
    }
}
