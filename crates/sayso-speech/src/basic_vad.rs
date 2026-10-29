//! Basic VAD: voice activity detection by the energy of the audio, without
//! the engine. It needs no model and no download, so jobs can cut long audio on
//! the first day (`docs/extensions.md`, "VAD models").
//!
//! The audio is 16 kHz mono. Each frame of 10 ms is speech when its level is
//! a margin above the noise floor. The noise floor is the lowest frame level
//! of the last seconds, so a constant noise (a fan, a hum) is not speech, and
//! the floor follows the noise when it changes. The frames then follow the
//! rules of the engines (`docs/engine-protocol.md`, "VAD"):
//!
//! - Speech that is shorter than 250 ms is left out.
//! - A pause that is shorter than 500 ms does not end the speech.
//! - A part is at most 30 s long. Longer speech is cut into parts of at most
//!   30 s, and the next part starts where the last part ends.
//!
//! [`detect`] finds the parts of a whole recording. [`BasicVad`] takes the
//! audio in chunks of any size and gives the same parts.

use std::collections::VecDeque;

/// The sample rate of the audio.
pub const SAMPLE_RATE: u32 = 16_000;
/// Speech under this is left out.
pub const MIN_SPEECH_MS: u64 = 250;
/// A pause under this does not end speech.
pub const MIN_PAUSE_MS: u64 = 500;
/// The longest part. Longer speech is cut into parts of this length.
pub const MAX_PART_MS: u64 = 30_000;

/// The length of a frame.
const FRAME_MS: u64 = 10;
const FRAME: usize = (SAMPLE_RATE as u64 * FRAME_MS / 1000) as usize;
/// The noise floor is the lowest frame level in this time.
const FLOOR_MS: u64 = 5_000;
/// A frame is speech when its level is this much above the noise floor.
const MARGIN_DB: f32 = 10.0;
/// A frame below this level is never speech, also in digital silence.
const QUIETEST_SPEECH_DB: f32 = -55.0;
/// The level of a frame of zeros.
const SILENCE_DB: f32 = -100.0;

/// One part of the audio with speech. The times are in milliseconds from the
/// first sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Part {
    pub start_ms: u64,
    pub end_ms: u64,
}

/// The parts with speech in `samples` (16 kHz mono, -1.0 to 1.0), in order.
pub fn detect(samples: &[f32]) -> Vec<Part> {
    let mut vad = BasicVad::new();
    let mut parts = vad.accept(samples);
    parts.extend(vad.finish());
    parts
}

/// Basic VAD on audio that comes in chunks. Each chunk can have any size: the
/// parts are the same as [`detect`] gives for the whole audio.
#[derive(Debug, Default)]
pub struct BasicVad {
    /// Audio that does not fill a frame yet.
    pending: Vec<f32>,
    /// The frames heard.
    frames: u64,
    /// The candidates for the noise floor: (frame, level), the levels in
    /// rising order. The first one is the floor.
    floor: VecDeque<(u64, f32)>,
    /// The part that is open: its first frame, and true when it continues a
    /// part that was cut at 30 s.
    open: Option<(u64, bool)>,
    /// The speech frames in the open part.
    voiced: u64,
    /// The frame after the last speech frame.
    speech_end: u64,
}

impl BasicVad {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hear more audio. Gives the parts that ended.
    pub fn accept(&mut self, samples: &[f32]) -> Vec<Part> {
        let mut parts = Vec::new();
        self.pending.extend_from_slice(samples);
        let pending = std::mem::take(&mut self.pending);
        let (frames, rest) = pending.as_chunks::<FRAME>();
        for frame in frames {
            self.frame(level_db(frame), &mut parts);
        }
        self.pending = rest.to_vec();
        parts
    }

    /// No more audio comes. Gives the part that is open, which ends at the
    /// last speech frame. The audio of a frame that is not complete is left out.
    pub fn finish(mut self) -> Vec<Part> {
        let mut parts = Vec::new();
        self.close(&mut parts);
        parts
    }

    /// True while a part is open.
    pub fn speaking(&self) -> bool {
        self.open.is_some()
    }

    /// Decide one frame.
    fn frame(&mut self, level: f32, parts: &mut Vec<Part>) {
        let index = self.frames;
        self.frames += 1;
        while self.floor.back().is_some_and(|&(_, l)| l >= level) {
            self.floor.pop_back();
        }
        self.floor.push_back((index, level));
        while self.floor.front().is_some_and(|&(i, _)| i + FLOOR_MS / FRAME_MS <= index) {
            self.floor.pop_front();
        }
        let floor = self.floor.front().map_or(level, |&(_, l)| l);
        let speech = level > (floor + MARGIN_DB).max(QUIETEST_SPEECH_DB);

        if speech {
            if self.open.is_none() {
                self.open = Some((index, false));
                self.voiced = 0;
            }
            self.voiced += 1;
            self.speech_end = index + 1;
            self.cut_long_part(parts);
        } else if self.open.is_some() && self.frames - self.speech_end >= MIN_PAUSE_MS / FRAME_MS {
            self.close(parts);
        }
    }

    /// Cut the open part at 30 s. The cut waits until the speech has gone on
    /// for the shortest speech after the cut, so the next part is not too short.
    fn cut_long_part(&mut self, parts: &mut Vec<Part>) {
        let max = MAX_PART_MS / FRAME_MS;
        while let Some((start, _)) = self.open {
            if self.speech_end < start + max + MIN_SPEECH_MS / FRAME_MS {
                break;
            }
            parts.push(part(start, start + max));
            self.open = Some((start + max, true));
            self.voiced = 0;
        }
    }

    /// End the open part at the last speech frame. Speech that is too short is
    /// left out, unless it continues a part that was cut.
    fn close(&mut self, parts: &mut Vec<Part>) {
        let Some((start, continued)) = self.open.take() else { return };
        let end = self.speech_end;
        if !continued && self.voiced < MIN_SPEECH_MS / FRAME_MS {
            return;
        }
        let max = MAX_PART_MS / FRAME_MS;
        if end - start > max {
            // The speech ended less than a shortest speech after 30 s: two
            // parts, and the second one is not shorter than the shortest speech.
            let cut = (start + max).min(end - MIN_SPEECH_MS / FRAME_MS);
            parts.push(part(start, cut));
            parts.push(part(cut, end));
        } else {
            parts.push(part(start, end));
        }
    }
}

fn part(start_frame: u64, end_frame: u64) -> Part {
    Part { start_ms: start_frame * FRAME_MS, end_ms: end_frame * FRAME_MS }
}

/// The level of a frame: its RMS in dB below full scale.
fn level_db(frame: &[f32]) -> f32 {
    let power = frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32;
    if power > 0.0 { (10.0 * power.log10()).max(SILENCE_DB) } else { SILENCE_DB }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: usize = SAMPLE_RATE as usize;

    /// A small random number generator, so the noise is the same in each run.
    struct Noise(u64);

    impl Noise {
        /// White noise with this RMS level in dB below full scale.
        fn samples(&mut self, ms: u64, db: f32) -> Vec<f32> {
            let amplitude = 10f32.powf(db / 20.0) * 3f32.sqrt();
            (0..samples(ms))
                .map(|_| {
                    self.0 ^= self.0 << 13;
                    self.0 ^= self.0 >> 7;
                    self.0 ^= self.0 << 17;
                    ((self.0 >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0) * amplitude
                })
                .collect()
        }
    }

    fn samples(ms: u64) -> usize {
        (ms as usize) * RATE / 1000
    }

    fn silence(ms: u64) -> Vec<f32> {
        vec![0.0; samples(ms)]
    }

    /// Something like speech: a voice of 140 Hz with harmonics, in syllables
    /// of 200 ms with a short dip between them, about -20 dB.
    fn speech(ms: u64) -> Vec<f32> {
        (0..samples(ms))
            .map(|n| {
                let t = n as f32 / RATE as f32;
                let voice: f32 = (1..=5).map(|h| (2.0 * std::f32::consts::PI * 140.0 * h as f32 * t).sin() / h as f32).sum();
                let syllable = (t * 5.0).fract();
                let envelope = if syllable < 0.85 { (std::f32::consts::PI * syllable / 0.85).sin().max(0.3) } else { 0.05 };
                0.1 * voice * envelope
            })
            .collect()
    }

    fn join(pieces: &[Vec<f32>]) -> Vec<f32> {
        pieces.concat()
    }

    fn add(a: &[f32], b: &[f32]) -> Vec<f32> {
        a.iter().zip(b).map(|(x, y)| x + y).collect()
    }

    /// True when `ms` is within 60 ms of `want`.
    fn near(ms: u64, want: u64) -> bool {
        ms.abs_diff(want) <= 60
    }

    #[test]
    fn finds_speech_between_silences() {
        let audio = join(&[silence(1000), speech(2000), silence(1000), speech(1500), silence(500)]);
        let parts = detect(&audio);
        assert_eq!(parts.len(), 2, "{parts:?}");
        assert!(near(parts[0].start_ms, 1000) && near(parts[0].end_ms, 3000), "{parts:?}");
        assert!(near(parts[1].start_ms, 4000) && near(parts[1].end_ms, 5500), "{parts:?}");
    }

    #[test]
    fn silence_and_digital_zeros_have_no_speech() {
        assert!(detect(&silence(3000)).is_empty());
        assert!(detect(&[]).is_empty());
        assert!(detect(&Noise(7).samples(3000, -70.0)).is_empty(), "very quiet noise");
    }

    #[test]
    fn a_constant_noise_floor_is_not_speech() {
        let mut noise = Noise(42);
        for db in [-50.0, -40.0, -35.0] {
            assert!(detect(&noise.samples(6000, db)).is_empty(), "noise at {db} dB");
        }
        // Speech over the noise still counts. The noise is 15 dB under the speech.
        let speech = join(&[silence(2000), speech(2000), silence(2000)]);
        let parts = detect(&add(&speech, &noise.samples(6000, -38.0)));
        assert_eq!(parts.len(), 1, "{parts:?}");
        assert!(near(parts[0].start_ms, 2000) && near(parts[0].end_ms, 4000), "{parts:?}");
    }

    #[test]
    fn the_floor_follows_a_louder_noise() {
        // A fan starts: after the floor time, the louder noise is not speech any more.
        let mut noise = Noise(3);
        let audio = join(&[noise.samples(3000, -60.0), noise.samples(15_000, -35.0)]);
        let parts = detect(&audio);
        assert!(parts.iter().all(|p| p.end_ms <= 3000 + FLOOR_MS + 100), "{parts:?}");
    }

    #[test]
    fn a_short_pause_does_not_end_speech() {
        let audio = join(&[silence(500), speech(1000), silence(300), speech(1000), silence(1000)]);
        let parts = detect(&audio);
        assert_eq!(parts.len(), 1, "{parts:?}");
        assert!(near(parts[0].start_ms, 500) && near(parts[0].end_ms, 2800), "{parts:?}");
        // A pause of 600 ms does.
        let audio = join(&[silence(500), speech(1000), silence(600), speech(1000), silence(1000)]);
        assert_eq!(detect(&audio).len(), 2);
    }

    #[test]
    fn short_speech_is_left_out() {
        let click = join(&[silence(1000), speech(150), silence(1000)]);
        assert!(detect(&click).is_empty(), "{:?}", detect(&click));
        let word = join(&[silence(1000), speech(400), silence(1000)]);
        assert_eq!(detect(&word).len(), 1);
    }

    #[test]
    fn long_speech_is_cut_into_touching_parts_of_30_s() {
        let audio = join(&[silence(500), speech(70_000), silence(1000)]);
        let parts = detect(&audio);
        assert_eq!(parts.len(), 3, "{parts:?}");
        assert!(near(parts[0].start_ms, 500), "{parts:?}");
        for pair in parts.windows(2) {
            assert_eq!(pair[0].end_ms, pair[1].start_ms, "the next part starts where the last ends");
        }
        assert!(parts.iter().all(|p| p.end_ms - p.start_ms <= MAX_PART_MS), "{parts:?}");
        assert!(near(parts[2].end_ms, 70_500), "{parts:?}");
    }

    #[test]
    fn speech_that_ends_just_after_30_s_gives_a_second_part_of_the_shortest_speech() {
        // The speech ends 100 ms after 30 s: the cut moves back.
        let audio = join(&[speech(30_100), silence(1000)]);
        let parts = detect(&audio);
        assert_eq!(parts.len(), 2, "{parts:?}");
        assert_eq!(parts[0].end_ms, parts[1].start_ms);
        assert!(parts[1].end_ms - parts[1].start_ms >= MIN_SPEECH_MS, "{parts:?}");
    }

    #[test]
    fn speech_at_the_end_ends_at_the_last_speech() {
        let audio = join(&[silence(1000), speech(1500)]);
        let parts = detect(&audio);
        assert_eq!(parts.len(), 1, "{parts:?}");
        assert!(near(parts[0].end_ms, 2500), "{parts:?}");
    }

    #[test]
    fn chunks_of_any_size_give_the_same_parts() {
        let mut noise = Noise(9);
        let speech = join(&[silence(800), speech(1200), silence(350), speech(900), silence(900), speech(31_000), silence(700)]);
        let audio = add(&speech, &noise.samples(36_850, -45.0));
        let whole = detect(&audio);
        assert_eq!(whole.len(), 3, "{whole:?}");
        for size in [1, 7, 160, 161, 1000, 4096, 16_000] {
            let mut vad = BasicVad::new();
            let mut parts = Vec::new();
            for chunk in audio.chunks(size) {
                parts.extend(vad.accept(chunk));
            }
            parts.extend(vad.finish());
            assert_eq!(parts, whole, "chunks of {size}");
        }
    }

    #[test]
    fn a_part_is_given_when_its_pause_is_long_enough() {
        let mut vad = BasicVad::new();
        assert!(vad.accept(&join(&[silence(500), speech(1000)])).is_empty());
        assert!(vad.speaking());
        assert!(vad.accept(&silence(300)).is_empty(), "the pause is too short yet");
        let parts = vad.accept(&silence(300));
        assert_eq!(parts.len(), 1, "{parts:?}");
        assert!(!vad.speaking());
        assert!(vad.finish().is_empty());
    }

    /// The real recording of the conformance suite. The engines find speech
    /// from about 70 ms to 7980 ms.
    #[test]
    #[ignore = "needs /tmp/conf/audio/short.wav"]
    fn finds_the_speech_of_a_real_recording() {
        let bytes = std::fs::read("/tmp/conf/audio/short.wav").unwrap();
        let samples = wav_samples(&bytes);
        let mut noise = Noise(5);
        let noisy = add(&samples, &noise.samples(samples.len() as u64 * 1000 / RATE as u64, -45.0));
        for audio in [samples, noisy] {
            let parts = detect(&audio);
            println!("{parts:?}");
            assert!(!parts.is_empty());
            let (start, end) = (parts[0].start_ms, parts.last().unwrap().end_ms);
            assert!(start <= 200, "{parts:?}");
            assert!(end >= 7850, "{parts:?}");
        }
    }

    /// The samples of a 16 kHz mono 16-bit WAV file.
    fn wav_samples(bytes: &[u8]) -> Vec<f32> {
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            if &bytes[at..at + 4] == b"data" {
                let data = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
                return data.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b) as f32 / 32768.0).collect();
            }
            at += 8 + size + size % 2;
        }
        panic!("no data chunk");
    }
}
