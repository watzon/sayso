// Copied from sayso-platform-macos; keep the two in step until they move to a shared crate.
//! Streaming sample-rate conversion to 16 kHz mono.
//!
//! A Hann-windowed sinc interpolator. When the input rate is higher than the
//! output rate, the sinc is widened so it low-passes at the new Nyquist
//! frequency. That keeps fricatives above 8 kHz from folding back into the
//! speech band, which plain linear interpolation would do.
//!
//! Chunk boundaries do not matter: feeding samples in any split gives the
//! same output.

use std::f64::consts::PI;

/// Zero crossings of the sinc on each side of the center, at the output rate.
const ZERO_CROSSINGS: f64 = 10.0;

pub struct StreamResampler {
    /// Input samples per output sample.
    step: f64,
    /// Kernel stretch: 1 when upsampling, the ratio when downsampling.
    scale: f64,
    /// Kernel half width in input samples.
    half_width: f64,
    /// Input history plus samples not yet consumed.
    buf: Vec<f32>,
    /// Position of the next output sample, in `buf` coordinates.
    pos: f64,
    passthrough: bool,
}

impl StreamResampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        assert!(in_rate > 0 && out_rate > 0, "sample rates must be positive");
        let step = f64::from(in_rate) / f64::from(out_rate);
        let scale = step.max(1.0);
        let half_width = ZERO_CROSSINGS * scale;
        // Start with silence before the first sample, so the first output
        // lines up with the first input sample.
        let lead = half_width.ceil() as usize;
        Self {
            step,
            scale,
            half_width,
            buf: vec![0.0; lead],
            pos: lead as f64,
            passthrough: in_rate == out_rate,
        }
    }

    /// Convert `input` and append the finished output samples to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.passthrough {
            out.extend_from_slice(input);
            return;
        }
        self.buf.extend_from_slice(input);
        loop {
            let first = (self.pos - self.half_width).ceil() as usize;
            let last = (self.pos + self.half_width).floor() as usize;
            if last >= self.buf.len() {
                break;
            }
            let (mut acc, mut norm) = (0.0f64, 0.0f64);
            for (i, &sample) in self.buf[first..=last].iter().enumerate() {
                let x = (first + i) as f64 - self.pos;
                let weight = sinc(x / self.scale) * hann(x / self.half_width);
                acc += f64::from(sample) * weight;
                norm += weight;
            }
            // Dividing by the weight sum gives exactly unity gain at DC.
            out.push(if norm.abs() > 1e-9 { (acc / norm) as f32 } else { 0.0 });
            self.pos += self.step;
        }
        // Keep only what later outputs still need.
        let keep_from = ((self.pos - self.half_width).floor().max(0.0) as usize).min(self.buf.len());
        self.buf.drain(..keep_from);
        self.pos -= keep_from as f64;
    }
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 { 1.0 } else { (PI * x).sin() / (PI * x) }
}

/// Hann window over `-1..=1`.
fn hann(t: f64) -> f64 {
    0.5 * (1.0 + (PI * t).cos())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, rate: u32, seconds: f64) -> Vec<f32> {
        (0..(f64::from(rate) * seconds) as usize)
            .map(|n| (2.0 * PI * freq * n as f64 / f64::from(rate)).sin() as f32 * 0.5)
            .collect()
    }

    fn convert(input: &[f32], in_rate: u32, chunk: usize) -> Vec<f32> {
        let mut r = StreamResampler::new(in_rate, 16_000);
        let mut out = Vec::new();
        for c in input.chunks(chunk) {
            r.process(c, &mut out);
        }
        out
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn output_length_follows_the_rate_ratio() {
        for rate in [48_000u32, 44_100, 24_000, 8_000] {
            let input = sine(440.0, rate, 2.0);
            let out = convert(&input, rate, 441);
            let expected = 2.0 * 16_000.0;
            // The last samples stay inside the filter: its half width is 10 output samples
            // when downsampling and 20 when upsampling 8 kHz.
            assert!((out.len() as f64 - expected).abs() <= 32.0, "{rate}: {} vs {expected}", out.len());
        }
    }

    #[test]
    fn same_rate_passes_through_unchanged() {
        let input = sine(300.0, 16_000, 0.1);
        assert_eq!(convert(&input, 16_000, 100), input);
    }

    #[test]
    fn a_tone_in_the_speech_band_keeps_its_level() {
        let out = convert(&sine(1_000.0, 48_000, 1.0), 48_000, 480);
        let steady = &out[1_000..out.len() - 1_000];
        let expected = 0.5 / 2f32.sqrt();
        assert!((rms(steady) - expected).abs() < 0.01, "rms {}", rms(steady));
    }

    #[test]
    fn a_tone_keeps_its_frequency() {
        let out = convert(&sine(1_000.0, 44_100, 1.0), 44_100, 441);
        let steady = &out[1_000..out.len() - 1_000];
        let crossings = steady.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        let seconds = steady.len() as f64 / 16_000.0;
        assert!((crossings as f64 / seconds - 1_000.0).abs() < 10.0, "{crossings} crossings in {seconds} s");
    }

    #[test]
    fn content_above_the_new_nyquist_is_removed() {
        // 12 kHz is above 8 kHz, so linear interpolation would alias it to 4 kHz.
        let out = convert(&sine(12_000.0, 48_000, 1.0), 48_000, 480);
        let steady = &out[1_000..out.len() - 1_000];
        assert!(rms(steady) < 0.01, "aliased energy {}", rms(steady));
    }

    #[test]
    fn dc_level_is_preserved() {
        let out = convert(&vec![0.25; 48_000], 48_000, 1_000);
        for s in &out[200..out.len() - 200] {
            assert!((s - 0.25).abs() < 1e-4);
        }
    }

    #[test]
    fn chunk_size_does_not_change_the_output() {
        let input = sine(700.0, 44_100, 0.5);
        let whole = convert(&input, 44_100, input.len());
        let small = convert(&input, 44_100, 97);
        assert_eq!(whole.len(), small.len());
        for (a, b) in whole.iter().zip(&small) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn upsampling_works() {
        let out = convert(&sine(500.0, 8_000, 1.0), 8_000, 160);
        assert!((out.len() as f64 - 16_000.0).abs() <= 32.0);
        let steady = &out[1_000..out.len() - 1_000];
        assert!((rms(steady) - 0.5 / 2f32.sqrt()).abs() < 0.01);
    }

    #[test]
    fn empty_input_gives_no_output() {
        let mut r = StreamResampler::new(48_000, 16_000);
        let mut out = Vec::new();
        r.process(&[], &mut out);
        assert!(out.is_empty());
    }
}
