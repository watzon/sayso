//! Read the file of a job: WAV, FLAC, MP3, AAC (ADTS), and M4A, with
//! `symphonia` (`docs/extensions.md`, "Requests"). The audio becomes 16 kHz
//! mono, the format of the engines.
//!
//! A job keeps the audio as 16-bit samples: 2 hours take 230 MB, and not
//! the 460 MB of floats. A part becomes floats only when it goes to the
//! engine. The file is decoded in one pass before the job starts, because the
//! compressed formats cannot start in the middle cheaply.

use crate::resample::StreamResampler;
use sayso_protocol::pcm::SAMPLE_RATE;
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Samples in one millisecond.
const PER_MS: u64 = SAMPLE_RATE as u64 / 1000;

/// What the error messages name.
const FORMATS: &str = "Sayso reads WAV, FLAC, MP3, AAC, and M4A files.";

/// 16 kHz mono audio, as 16-bit samples.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Audio {
    samples: Vec<i16>,
}

impl Audio {
    pub fn from_samples(samples: Vec<i16>) -> Audio {
        Audio { samples }
    }

    pub fn samples(&self) -> &[i16] {
        &self.samples
    }

    /// The length in milliseconds.
    pub fn len_ms(&self) -> u64 {
        self.samples.len() as u64 / PER_MS
    }

    /// The samples from `start_ms` to `end_ms` as floats from -1 to 1, for the
    /// engine. Times past the end stop at the end.
    pub fn floats(&self, start_ms: u64, end_ms: u64) -> Vec<f32> {
        let index = |ms: u64| usize::try_from(ms.saturating_mul(PER_MS)).unwrap_or(usize::MAX).min(self.samples.len());
        let (start, end) = (index(start_ms), index(end_ms));
        self.samples[start..end.max(start)].iter().map(|&s| f32::from(s) / 32768.0).collect()
    }

    /// All the samples as floats.
    pub fn all_floats(&self) -> Vec<f32> {
        self.floats(0, u64::MAX)
    }
}

/// Read and decode `path`. The error is an English sentence for `job_failed`.
pub fn read(path: &Path) -> Result<Audio, String> {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let file = std::fs::File::open(path).map_err(|e| format!("Sayso cannot open {}: {e}", path.display()))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    // Gapless: an MP3 or M4A file does not get the silence of the encoder at the start.
    let options = FormatOptions { enable_gapless: true, ..Default::default() };
    let probed = symphonia::default::get_probe()
        .format(&hint, stream, &options, &MetadataOptions::default())
        .map_err(|e| match e {
            Error::IoError(e) => format!("Sayso cannot read {name}: {e}"),
            _ => format!("{name} is not an audio file that Sayso can read. {FORMATS}"),
        })?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| format!("{name} has no audio."))?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("Sayso cannot decode the audio in {name} ({e}). {FORMATS}"))?;

    let mut out = Mono::default();
    // Reserve the whole length when the file says it, so the buffer does not grow by doubling.
    if let (Some(frames), Some(rate)) = (track.codec_params.n_frames, track.codec_params.sample_rate)
        && rate > 0
    {
        let expected = frames.saturating_mul(u64::from(SAMPLE_RATE)) / u64::from(rate);
        out.samples.reserve(usize::try_from(expected).unwrap_or(0).saturating_add(SAMPLE_RATE as usize));
    }
    let mut last_error = None;
    let mut buffer: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            // A new chain of tracks starts: the job reads the first one only.
            Err(Error::ResetRequired) => break,
            Err(Error::IoError(e)) => return Err(format!("Sayso cannot read {name}: {e}")),
            Err(e) => return Err(format!("{name} is damaged: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let channels = spec.channels.count().max(1);
                // One buffer for the file, made again only when a packet does not fit.
                let fits = buffer.as_ref().is_some_and(|b| b.capacity() >= decoded.capacity() * channels);
                if !fits {
                    buffer = Some(SampleBuffer::new(decoded.capacity() as u64, spec));
                }
                let Some(buffer) = buffer.as_mut() else { continue };
                buffer.copy_interleaved_ref(decoded);
                out.push(buffer.samples(), channels, spec.rate);
            }
            // A damaged packet: leave it out, as players do.
            Err(Error::DecodeError(e)) => last_error = Some(e),
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(format!("Sayso cannot decode the audio in {name}: {e}")),
        }
    }
    if out.samples.is_empty()
        && let Some(e) = last_error
    {
        return Err(format!("Sayso cannot decode the audio in {name}: {e}"));
    }
    Ok(Audio { samples: out.finish() })
}

/// Mixes to mono, converts to 16 kHz, and keeps 16-bit samples.
#[derive(Default)]
struct Mono {
    samples: Vec<i16>,
    rate: Option<Rate>,
    mono: Vec<f32>,
    resampled: Vec<f32>,
}

/// The conversion of one input rate.
struct Rate {
    rate: u32,
    resampler: StreamResampler,
    /// The input samples so far.
    heard: u64,
    /// Where the output of this rate starts in `Mono::samples`.
    start: usize,
}

impl Mono {
    fn push(&mut self, interleaved: &[f32], channels: usize, rate: u32) {
        self.mono.clear();
        self.mono.extend(interleaved.chunks_exact(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32));
        if self.rate.as_ref().is_none_or(|r| r.rate != rate) {
            // A stream that changes its rate: end the old rate, start the new one.
            self.flush();
            let start = self.samples.len();
            self.rate = Some(Rate { rate, resampler: StreamResampler::new(rate, SAMPLE_RATE), heard: 0, start });
        }
        let Some(current) = &mut self.rate else { return };
        current.heard += self.mono.len() as u64;
        self.resampled.clear();
        current.resampler.process(&self.mono, &mut self.resampled);
        self.samples.extend(self.resampled.iter().map(|&s| to_i16(s)));
    }

    /// The resampler holds back the last samples. Push 100 ms of silence
    /// through it, then keep exactly the length of the input at 16 kHz.
    fn flush(&mut self) {
        let Some(mut current) = self.rate.take() else { return };
        self.resampled.clear();
        current.resampler.process(&vec![0.0; current.rate as usize / 10], &mut self.resampled);
        self.samples.extend(self.resampled.iter().map(|&s| to_i16(s)));
        let expected = current.heard * u64::from(SAMPLE_RATE) / u64::from(current.rate);
        let keep = current.start.saturating_add(usize::try_from(expected).unwrap_or(usize::MAX));
        self.samples.truncate(keep);
    }

    fn finish(mut self) -> Vec<i16> {
        self.flush();
        self.samples
    }
}

fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The fixtures in `tests/fixtures` are 1 s of a 440 Hz tone from 0.3 s
    /// to 0.7 s, with silence around it. They were made for this test with
    /// ffmpeg and lame (see `tests/fixtures/README.md`).
    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    /// A 16 kHz mono 16-bit WAV file of `samples`.
    fn encode_wav(samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut wav = Vec::with_capacity(44 + samples.len() * 2);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        for field in [16u32.to_le_bytes().as_slice(), &1u16.to_le_bytes(), &1u16.to_le_bytes(), &SAMPLE_RATE.to_le_bytes(), &(SAMPLE_RATE * 2).to_le_bytes(), &2u16.to_le_bytes(), &16u16.to_le_bytes()] {
            wav.extend_from_slice(field);
        }
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        wav
    }

    /// The RMS level of the audio from `start_ms` to `end_ms`.
    fn rms(audio: &Audio, start_ms: u64, end_ms: u64) -> f32 {
        let floats = audio.floats(start_ms, end_ms);
        (floats.iter().map(|s| s * s).sum::<f32>() / floats.len().max(1) as f32).sqrt()
    }

    /// The tone is where the file has it, and the silence is silent. The
    /// level is loose: 0.5 peak is 0.35 RMS, and the AAC encoder made the
    /// tone of its files about 3 dB louder (ffmpeg reads them the same way).
    fn check_tone(audio: &Audio, name: &str) {
        // MP3 and AAC pad the last frame, so their files can be some tens of ms longer.
        assert!(audio.len_ms().abs_diff(1_000) <= 70, "{name}: {} ms", audio.len_ms());
        let tone = rms(audio, 350, 650);
        assert!((0.3..0.55).contains(&tone), "{name}: tone at {tone}");
        for (start, end) in [(0, 250), (750, 1_000)] {
            let quiet = rms(audio, start, end);
            assert!(quiet < 0.02, "{name}: {start}..{end} ms has {quiet}");
        }
    }

    #[test]
    fn a_wav_file_of_another_rate_with_two_channels_becomes_16_khz_mono() {
        // 44.1 kHz stereo 16-bit: the tone on the left, silence on the right,
        // so the mix halves it.
        let rate = 44_100u32;
        let frames: Vec<[i16; 2]> = (0..rate)
            .map(|n| {
                let t = n as f32 / rate as f32;
                let tone = if (0.3..0.7).contains(&t) { (2.0 * std::f32::consts::PI * 440.0 * t).sin() } else { 0.0 };
                [(tone * 32767.0) as i16, 0]
            })
            .collect();
        let mut wav = Vec::new();
        let data_len = (frames.len() * 4) as u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        for field in [16u32.to_le_bytes().as_slice(), &1u16.to_le_bytes(), &2u16.to_le_bytes(), &rate.to_le_bytes(), &(rate * 4).to_le_bytes(), &4u16.to_le_bytes(), &16u16.to_le_bytes()] {
            wav.extend_from_slice(field);
        }
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for [left, right] in &frames {
            wav.extend_from_slice(&left.to_le_bytes());
            wav.extend_from_slice(&right.to_le_bytes());
        }
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("stereo.wav");
        std::fs::write(&path, wav).unwrap();

        let audio = read(&path).unwrap();
        assert_eq!(audio.samples().len(), 16_000, "exactly 1 s at 16 kHz");
        check_tone(&audio, "stereo.wav");
    }

    #[test]
    fn a_16_khz_wav_file_keeps_its_samples() {
        let samples: Vec<i16> = (0..3_200).map(|n| (n % 200) as i16 * 100 - 10_000).collect();
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("same.wav");
        std::fs::write(&path, encode_wav(&samples)).unwrap();
        assert_eq!(read(&path).unwrap().samples(), samples);
    }

    #[test]
    fn compressed_files_are_decoded() {
        for name in ["tone.mp3", "tone.flac", "tone.m4a", "tone.aac"] {
            let audio = read(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            check_tone(&audio, name);
        }
    }

    #[test]
    fn a_file_that_is_not_audio_gets_a_clear_message() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "Meeting notes: nothing to hear here.").unwrap();
        let message = read(&path).unwrap_err();
        assert_eq!(message, "notes.txt is not an audio file that Sayso can read. Sayso reads WAV, FLAC, MP3, AAC, and M4A files.");
        let message = read(&dir.path().join("missing.wav")).unwrap_err();
        assert!(message.starts_with("Sayso cannot open ") && message.contains("missing.wav"), "{message}");
    }

    #[test]
    fn floats_stop_at_the_end() {
        let audio = Audio::from_samples(vec![16_384; 32]);
        assert_eq!(audio.len_ms(), 2);
        assert_eq!(audio.floats(1, 10), vec![0.5; 16]);
        assert!(audio.floats(5, 10).is_empty());
        assert_eq!(audio.all_floats().len(), 32);
    }
}
