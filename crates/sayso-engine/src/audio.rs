//! Audio input: WAV files, base64 PCM, and the cuts for long recordings.

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use std::path::Path;

pub const SAMPLE_RATE: usize = 16_000;

/// The samples of a 16 kHz 16-bit PCM WAV file, as floats. More than one
/// channel is mixed down to mono.
pub fn read_wav(path: &Path) -> Result<Vec<f32>> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    parse_wav(&bytes).with_context(|| format!("cannot read {}", path.display()))
}

pub fn parse_wav(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a WAV file");
    }
    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into()?) as usize;
        let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
        match id {
            b"fmt " if body.len() >= 16 => {
                let u16_at = |i: usize| u16::from_le_bytes([body[i], body[i + 1]]);
                let rate = u32::from_le_bytes(body[4..8].try_into()?);
                format = Some((u16_at(0), u16_at(2), rate, u16_at(14)));
            }
            b"data" => {
                let Some((tag, channels, rate, bits)) = format else {
                    bail!("the data chunk comes before the format chunk");
                };
                // 1 is PCM. 0xFFFE is the extensible format, which Sayso does not write
                // but other tools do for plain PCM too.
                if !(tag == 1 || tag == 0xFFFE) || bits != 16 {
                    bail!("only 16-bit PCM is supported (format {tag}, {bits} bits)");
                }
                if rate as usize != SAMPLE_RATE {
                    bail!("audio must be 16 kHz, not {rate} Hz");
                }
                let channels = usize::from(channels.max(1));
                let (frames, _) = body.as_chunks::<2>();
                return Ok(frames
                    .chunks_exact(channels)
                    .map(|frame| {
                        let sum: f32 = frame
                            .iter()
                            .map(|b| f32::from(i16::from_le_bytes(*b)))
                            .sum();
                        sum / channels as f32 / 32768.0
                    })
                    .collect());
            }
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    bail!("no data chunk")
}

/// The `pcm` field of `stream_audio`: base64 of little-endian 16-bit samples.
pub fn decode_pcm(pcm: &str) -> Result<Vec<f32>> {
    let bytes = STANDARD.decode(pcm).context("pcm is not base64")?;
    let (pairs, _) = bytes.as_chunks::<2>();
    Ok(pairs
        .iter()
        .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0)
        .collect())
}

/// Pieces of at most `max_seconds`. Each cut is at the quietest point in the
/// last five seconds of a piece, so it rarely falls inside a word.
pub fn split(samples: &[f32], max_seconds: usize) -> Vec<&[f32]> {
    let limit = max_seconds.saturating_mul(SAMPLE_RATE);
    let window = SAMPLE_RATE / 5; // 200 ms
    let search = (5 * SAMPLE_RATE).min(limit / 2);
    let mut pieces = Vec::new();
    let mut start = 0;
    while samples.len() - start > limit {
        let end = start + limit;
        let mut best = end;
        let mut best_energy = f32::MAX;
        let mut position = end - search;
        while position + window <= end {
            let energy: f32 = samples[position..position + window]
                .iter()
                .map(|s| s * s)
                .sum();
            if energy < best_energy {
                best_energy = energy;
                best = position + window / 2;
            }
            position += window / 2;
        }
        pieces.push(&samples[start..best]);
        start = best;
    }
    pieces.push(&samples[start..]);
    pieces
}

/// Join the texts of the pieces. A space goes between two pieces only when
/// the left one ends in a script that uses spaces (not Chinese or Japanese).
pub fn join(parts: &[String]) -> String {
    let mut result = String::new();
    for part in parts.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        if result.chars().last().is_some_and(|c| (c as u32) < 0x2E80) {
            result.push(' ');
        }
        result.push_str(part);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(channels: u16, rate: u32, samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32 + 10).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        // An unknown chunk with an odd size, which needs a pad byte.
        out.extend_from_slice(b"LIST");
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2 * u32::from(channels)).to_le_bytes());
        out.extend_from_slice(&(2 * channels).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        out
    }

    #[test]
    fn reads_mono_and_mixes_stereo() {
        assert_eq!(
            parse_wav(&wav(1, 16_000, &[16384, -16384])).unwrap(),
            vec![0.5, -0.5]
        );
        assert_eq!(
            parse_wav(&wav(2, 16_000, &[16384, 0, -32768, -32768])).unwrap(),
            vec![0.25, -1.0]
        );
        assert!(
            parse_wav(&wav(1, 44_100, &[0]))
                .unwrap_err()
                .to_string()
                .contains("16 kHz")
        );
        assert!(parse_wav(b"nope").is_err());
    }

    #[test]
    fn the_real_test_clip_is_readable() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/engine/audio/short.wav");
        let samples = read_wav(&path).unwrap();
        assert!(samples.len() > SAMPLE_RATE, "{} samples", samples.len());
    }

    #[test]
    fn decodes_base64_pcm() {
        let encoded = STANDARD.encode([0x00, 0x40, 0x00, 0xC0]);
        assert_eq!(decode_pcm(&encoded).unwrap(), vec![0.5, -0.5]);
        assert!(decode_pcm("***").is_err());
    }

    #[test]
    fn split_cuts_long_audio_at_a_quiet_point() {
        let short = vec![0.5; SAMPLE_RATE * 3];
        assert_eq!(split(&short, 10).len(), 1);

        // 25 s of noise with one quiet second at 8 s.
        let mut samples = vec![0.5; SAMPLE_RATE * 25];
        samples[SAMPLE_RATE * 8..SAMPLE_RATE * 9].fill(0.0);
        let pieces = split(&samples, 10);
        assert!(pieces.len() >= 3);
        let first = pieces[0].len();
        assert!(
            (SAMPLE_RATE * 8..=SAMPLE_RATE * 9).contains(&first),
            "cut at {first}"
        );
        assert_eq!(pieces.iter().map(|p| p.len()).sum::<usize>(), samples.len());
        assert!(pieces.iter().all(|p| p.len() <= SAMPLE_RATE * 10));
    }

    #[test]
    fn join_uses_spaces_only_where_the_script_does() {
        let parts = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(join(&parts(&[" hello ", "", "world"])), "hello world");
        assert_eq!(join(&parts(&["你好", "世界"])), "你好世界");
    }
}
