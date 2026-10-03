//! Audio input: WAV files, `stream_audio` PCM, loudness, and quiet cut points.

use crate::protocol::Result;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use std::ops::Range;
use std::path::Path;

/// Every model takes 16 kHz mono.
pub const SAMPLE_RATE: usize = 16_000;

/// Samples in `ms` milliseconds at 16 kHz.
pub const fn ms(ms: usize) -> usize {
    ms * SAMPLE_RATE / 1000
}

/// Read a WAV file as 16 kHz mono floats in -1..1.
///
/// The app writes 16 kHz mono 16-bit PCM. Other PCM and float formats, more
/// channels (mixed down), and other rates (linear resampling) also work.
pub fn read_wav(path: &Path) -> Result<Vec<f32>> {
    let reader =
        hound::WavReader::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    decode_wav(reader).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// Decode WAV bytes, for tests and in-memory audio.
pub fn wav_from_bytes(bytes: &[u8]) -> Result<Vec<f32>> {
    let reader = hound::WavReader::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    decode_wav(reader)
}

fn decode_wav<R: std::io::Read>(reader: hound::WavReader<R>) -> Result<Vec<f32>> {
    let spec = reader.spec();
    if spec.channels == 0 || spec.sample_rate == 0 {
        return Err("the WAV header has no channels or no sample rate".into());
    }
    let interleaved: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => reader
            .into_samples::<f32>()
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| e.to_string())?,
        (hound::SampleFormat::Int, bits @ 1..=32) => {
            let scale = 1.0 / (1u64 << (bits - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| s.map(|s| s as f32 * scale))
                .collect::<std::result::Result<_, _>>()
                .map_err(|e| e.to_string())?
        }
        (format, bits) => {
            return Err(format!(
                "unsupported WAV sample format {format:?} with {bits} bits"
            ));
        }
    };
    let mono = mix_down(&interleaved, usize::from(spec.channels));
    Ok(resample(&mono, spec.sample_rate as usize))
}

fn mix_down(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Linear resampling to 16 kHz. Good enough for speech models.
fn resample(samples: &[f32], rate: usize) -> Vec<f32> {
    if rate == SAMPLE_RATE || samples.is_empty() {
        return samples.to_vec();
    }
    let out_len = (samples.len() as u64 * SAMPLE_RATE as u64 / rate as u64) as usize;
    let step = rate as f64 / SAMPLE_RATE as f64;
    (0..out_len)
        .map(|i| {
            let position = i as f64 * step;
            let at = position as usize;
            let frac = (position - at as f64) as f32;
            let a = samples[at.min(samples.len() - 1)];
            let b = samples[(at + 1).min(samples.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// The `pcm` field of `stream_audio`: base64 of 16 kHz mono little-endian i16.
pub fn pcm_from_base64(pcm: &str) -> Result<Vec<f32>> {
    let bytes = STANDARD
        .decode(pcm.trim())
        .map_err(|_| "pcm is not base64".to_string())?;
    if bytes.len() % 2 != 0 {
        return Err("pcm has an odd number of bytes".into());
    }
    let (pairs, _) = bytes.as_chunks::<2>();
    Ok(pairs
        .iter()
        .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0)
        .collect())
}

/// Root mean square of a block of samples.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// The quietest point in `range`: the center of the 100 ms window with the
/// lowest energy, scanned in 20 ms steps. A cut there rarely splits a word.
pub fn quietest_point(samples: &[f32], range: Range<usize>) -> usize {
    let window = ms(100);
    let hop = ms(20);
    let end = range.end.min(samples.len());
    let start = range.start.min(end);
    if end - start <= window {
        return (start + end) / 2;
    }
    let mut best = (f32::MAX, (start + end) / 2);
    let mut at = start;
    while at + window <= end {
        let energy = rms(&samples[at..at + window]);
        if energy < best.0 {
            best = (energy, at + window / 2);
        }
        at += hop;
    }
    best.1
}

/// Cut a long recording into pieces of at most `max` samples. Each cut is at
/// the quietest point of the last `search` samples of a piece.
pub fn split_at_quiet_points(samples: &[f32], max: usize, search: usize) -> Vec<Range<usize>> {
    let mut pieces = Vec::new();
    let mut start = 0;
    while samples.len() - start > max {
        let end = start + max;
        let cut = quietest_point(samples, end.saturating_sub(search).max(start + 1)..end);
        pieces.push(start..cut);
        start = cut;
    }
    pieces.push(start..samples.len());
    pieces
}

/// Join texts with one space, skipping empty ones.
pub fn join_texts<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for part in parts.into_iter().map(str::trim).filter(|p| !p.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(
        spec: hound::WavSpec,
        write: impl FnOnce(&mut hound::WavWriter<std::io::Cursor<&mut Vec<u8>>>),
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut writer = hound::WavWriter::new(std::io::Cursor::new(&mut bytes), spec).unwrap();
            write(&mut writer);
            writer.finalize().unwrap();
        }
        bytes
    }

    #[test]
    fn reads_the_wav_the_app_writes() {
        let pcm: Vec<i16> = vec![0, 16384, -16384, 32767];
        let bytes = sayso_core::stt::encode_wav(&pcm);
        let samples = wav_from_bytes(&bytes).unwrap();
        assert_eq!(samples.len(), 4);
        assert!((samples[1] - 0.5).abs() < 1e-4);
        assert!((samples[2] + 0.5).abs() < 1e-4);
    }

    #[test]
    fn mixes_stereo_and_resamples() {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let bytes = wav(spec, |w| {
            for _ in 0..4800 {
                w.write_sample(16384i16).unwrap();
                w.write_sample(0i16).unwrap();
            }
        });
        let samples = wav_from_bytes(&bytes).unwrap();
        assert_eq!(samples.len(), 1600, "100 ms at 16 kHz");
        assert!(samples.iter().all(|s| (s - 0.25).abs() < 1e-3));
    }

    #[test]
    fn reads_float_wav() {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let bytes = wav(spec, |w| {
            w.write_sample(0.25f32).unwrap();
            w.write_sample(-1.0f32).unwrap();
        });
        assert_eq!(wav_from_bytes(&bytes).unwrap(), vec![0.25, -1.0]);
    }

    #[test]
    fn rejects_files_that_are_not_wav() {
        assert!(wav_from_bytes(b"hello").is_err());
        let error = read_wav(Path::new("does/not/exist.wav")).unwrap_err();
        assert!(error.contains("cannot read"), "{error}");
    }

    #[test]
    fn decodes_stream_pcm() {
        let samples = pcm_from_base64(&STANDARD.encode([0x00, 0x40, 0x00, 0xC0])).unwrap();
        assert_eq!(samples, vec![0.5, -0.5]);
        assert!(pcm_from_base64("***").is_err());
        assert!(pcm_from_base64(&STANDARD.encode([1, 2, 3])).is_err());
        assert!(pcm_from_base64("").unwrap().is_empty());
    }

    #[test]
    fn finds_the_quiet_gap() {
        // 3 s of tone with a 200 ms gap at 2.0 s.
        let mut samples: Vec<f32> = (0..ms(3000))
            .map(|i| (i as f32 * 0.1).sin() * 0.5)
            .collect();
        samples[ms(2000)..ms(2200)].fill(0.0);
        let cut = quietest_point(&samples, 0..samples.len());
        assert!((ms(2000)..ms(2200)).contains(&cut), "cut at {cut}");
    }

    #[test]
    fn splits_long_audio_at_quiet_points() {
        let mut samples: Vec<f32> = vec![0.3; ms(25_000)];
        samples[ms(8_500)..ms(8_700)].fill(0.0);
        samples[ms(17_000)..ms(17_200)].fill(0.0);
        let pieces = split_at_quiet_points(&samples, ms(10_000), ms(3_000));
        assert_eq!(pieces.len(), 3);
        assert_eq!(pieces[0].start, 0);
        assert!((ms(8_500)..ms(8_700)).contains(&pieces[0].end));
        assert!((ms(17_000)..ms(17_200)).contains(&pieces[1].end));
        assert_eq!(pieces[2].end, samples.len());
        assert!(pieces.windows(2).all(|w| w[0].end == w[1].start));
        assert_eq!(
            split_at_quiet_points(&samples[..100], ms(10_000), ms(3_000)),
            vec![0..100]
        );
    }

    #[test]
    fn joins_texts() {
        assert_eq!(join_texts(["Hello", "", " world. "]), "Hello world.");
        assert_eq!(join_texts(Vec::<&str>::new()), "");
    }
}
