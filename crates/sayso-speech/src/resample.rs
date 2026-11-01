//! Streaming sample-rate conversion to 16 kHz mono.
//!
//! A Hann-windowed sinc interpolator. When the input rate is higher than the
//! output rate, the sinc is widened so it low-passes at the new Nyquist
//! frequency. That keeps fricatives above 8 kHz from folding back into the
//! speech band, which plain linear interpolation would do.
//!
//! Chunk boundaries do not matter: feeding samples in any split gives the
//! same output.
//!
//! The ratio of two sample rates is a fraction `p / q` (48 kHz to 16 kHz is 3/1,
//! 44.1 kHz to 16 kHz is 441/160). The distance of an output sample to the
//! input samples repeats every `q` outputs, so the kernel weights, and their
//! sum, are computed once for each of the `q` phases. Each output is then one
//! dot product. A ratio with more than [`MAX_PHASES`] phases keeps the direct
//! path, which computes every weight again for every output.

use std::f64::consts::PI;

/// Zero crossings of the sinc on each side of the center, at the output rate.
const ZERO_CROSSINGS: f64 = 10.0;

/// The most phases that get a weight table. 44.1 kHz to 16 kHz needs 160.
const MAX_PHASES: u64 = 1024;

pub struct StreamResampler {
    kernel: Kernel,
    /// Input history plus samples not yet consumed.
    buf: Vec<f32>,
}

enum Kernel {
    Passthrough,
    Polyphase(Polyphase),
    Direct(Direct),
}

/// The kernel of one phase: where its taps start, and their weights.
struct Phase {
    /// The first tap, in input samples from the integer part of the output position.
    first: isize,
    /// Start of the weights in `Polyphase::weights`.
    at: usize,
    len: usize,
}

struct Polyphase {
    /// Input samples per period.
    p: usize,
    /// Output samples per period.
    q: usize,
    phases: Vec<Phase>,
    /// The weights of all phases, each divided by the sum of its phase.
    weights: Vec<f32>,
    /// Integer part of the position of the next output, in `buf` coordinates.
    index: usize,
    /// Fraction of that position, in units of `1 / q`.
    phase: usize,
}

struct Direct {
    /// Input samples per output sample.
    step: f64,
    /// Kernel stretch: 1 when upsampling, the ratio when downsampling.
    scale: f64,
    /// Kernel half width in input samples.
    half_width: f64,
    /// Position of the next output sample, in `buf` coordinates.
    pos: f64,
}

impl StreamResampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        assert!(in_rate > 0 && out_rate > 0, "sample rates must be positive");
        if in_rate == out_rate {
            return Self { kernel: Kernel::Passthrough, buf: Vec::new() };
        }
        let divisor = gcd(u64::from(in_rate), u64::from(out_rate));
        let (p, q) = (u64::from(in_rate) / divisor, u64::from(out_rate) / divisor);
        if q <= MAX_PHASES { Self::polyphase(p as usize, q as usize) } else { Self::direct(in_rate, out_rate) }
    }

    fn polyphase(p: usize, q: usize) -> Self {
        // The half width in units of 1 / q input samples: ZERO_CROSSINGS * max(p / q, 1) * q.
        let half = ZERO_CROSSINGS as usize * p.max(q);
        let scale = (p as f64 / q as f64).max(1.0);
        let half_width = half as f64 / q as f64;
        let mut phases = Vec::with_capacity(q);
        let mut weights = Vec::new();
        for phase in 0..q {
            // Taps at the whole input samples inside `center +- half_width`.
            let first = -(((half - phase) / q) as isize);
            let last = ((phase + half) / q) as isize;
            let at = weights.len();
            let mut exact: Vec<f64> = (first..=last)
                .map(|tap| {
                    let x = tap as f64 - phase as f64 / q as f64;
                    sinc(x / scale) * hann(x / half_width)
                })
                .collect();
            // Dividing by the weight sum gives exactly unity gain at DC.
            let norm: f64 = exact.iter().sum();
            if norm.abs() > 1e-9 {
                exact.iter_mut().for_each(|w| *w /= norm);
            } else {
                exact.fill(0.0);
            }
            weights.extend(exact.iter().map(|&w| w as f32));
            phases.push(Phase { first, at, len: exact.len() });
        }
        // Start with silence before the first sample, so the first output
        // lines up with the first input sample.
        let lead = half.div_ceil(q);
        let polyphase = Polyphase { p, q, phases, weights, index: lead, phase: 0 };
        Self { kernel: Kernel::Polyphase(polyphase), buf: vec![0.0; lead] }
    }

    fn direct(in_rate: u32, out_rate: u32) -> Self {
        let step = f64::from(in_rate) / f64::from(out_rate);
        let scale = step.max(1.0);
        let half_width = ZERO_CROSSINGS * scale;
        let lead = half_width.ceil() as usize;
        let direct = Direct { step, scale, half_width, pos: lead as f64 };
        Self { kernel: Kernel::Direct(direct), buf: vec![0.0; lead] }
    }

    /// Convert `input` and append the finished output samples to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if let Kernel::Passthrough = self.kernel {
            out.extend_from_slice(input);
            return;
        }
        self.buf.extend_from_slice(input);
        let keep_from = match &mut self.kernel {
            Kernel::Passthrough => unreachable!("returned above"),
            Kernel::Polyphase(k) => k.run(&self.buf, out),
            Kernel::Direct(k) => k.run(&self.buf, out),
        };
        self.buf.drain(..keep_from);
    }
}

impl Polyphase {
    /// Write every output that `buf` has the input for. Returns how many
    /// samples at the start of `buf` no later output needs.
    fn run(&mut self, buf: &[f32], out: &mut Vec<f32>) -> usize {
        loop {
            let phase = &self.phases[self.phase];
            let first = (self.index as isize + phase.first) as usize;
            let Some(window) = buf.get(first..first + phase.len) else { break };
            out.push(dot(window, &self.weights[phase.at..phase.at + phase.len]));
            self.phase += self.p;
            self.index += self.phase / self.q;
            self.phase %= self.q;
        }
        // Keep only what later outputs still need. The first tap never moves back.
        let keep_from = ((self.index as isize + self.phases[self.phase].first).max(0) as usize).min(buf.len());
        self.index -= keep_from;
        keep_from
    }
}

impl Direct {
    fn run(&mut self, buf: &[f32], out: &mut Vec<f32>) -> usize {
        loop {
            let first = (self.pos - self.half_width).ceil() as usize;
            let last = (self.pos + self.half_width).floor() as usize;
            if last >= buf.len() {
                break;
            }
            let (mut acc, mut norm) = (0.0f64, 0.0f64);
            for (i, &sample) in buf[first..=last].iter().enumerate() {
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
        let keep_from = ((self.pos - self.half_width).floor().max(0.0) as usize).min(buf.len());
        self.pos -= keep_from as f64;
        keep_from
    }
}

/// The dot product, in eight independent sums so the compiler can vectorize it.
fn dot(samples: &[f32], weights: &[f32]) -> f32 {
    let mut lanes = [0.0f32; 8];
    let ((samples8, sample_tail), (weights8, weight_tail)) = (samples.as_chunks::<8>(), weights.as_chunks::<8>());
    let tail: f32 = sample_tail.iter().zip(weight_tail).map(|(s, w)| s * w).sum();
    for (s, w) in samples8.iter().zip(weights8) {
        for lane in 0..8 {
            lanes[lane] += s[lane] * w[lane];
        }
    }
    lanes.iter().sum::<f32>() + tail
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
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

    /// The direct path computes every weight for every output sample, as the
    /// first version of this module did. It is the reference for the tables.
    fn convert_direct(input: &[f32], in_rate: u32, chunk: usize) -> Vec<f32> {
        let mut r = StreamResampler::direct(in_rate, 16_000);
        let mut out = Vec::new();
        for c in input.chunks(chunk) {
            r.process(c, &mut out);
        }
        out
    }

    /// White noise in -0.8..0.8 from a fixed seed.
    fn noise(len: usize) -> Vec<f32> {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 1.6
            })
            .collect()
    }

    #[test]
    fn tables_match_the_direct_path() {
        // 48 kHz is one phase, 44.1 kHz is 160, 22.05 kHz is 320, 37 kHz is 16 with a
        // ratio that is not a common rate, and 8 kHz upsamples.
        for rate in [48_000u32, 44_100, 32_000, 96_000, 22_050, 37_000, 8_000] {
            let signals = [("noise", noise(rate as usize)), ("sine", sine(1_234.5, rate, 1.0))];
            for (name, input) in signals {
                for chunk in [input.len(), 4_800, 441, 97, 1] {
                    // Chunk size 1 is slow, so it only runs on a short slice.
                    let input = if chunk == 1 { &input[..4_000] } else { &input[..] };
                    let tables = convert(input, rate, chunk);
                    let direct = convert_direct(input, rate, chunk);
                    // The direct path adds `step` to a float for every output, and the sum drifts.
                    // When the last window ends exactly at the end of the input, where its weight
                    // is zero, the drift can add or remove that one output.
                    assert!(tables.len().abs_diff(direct.len()) <= 1, "{rate} Hz {name}, chunk {chunk}: length");
                    let worst = tables.iter().zip(&direct).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
                    assert!(worst < 1e-6, "{rate} Hz {name}, chunk {chunk}: largest difference {worst}");
                }
            }
        }
    }

    #[test]
    fn a_ratio_with_too_many_phases_uses_the_direct_path() {
        assert!(matches!(StreamResampler::new(44_101, 16_000).kernel, Kernel::Direct(_)));
        assert!(matches!(StreamResampler::new(44_100, 16_000).kernel, Kernel::Polyphase(_)));
        let input = noise(44_101);
        let out = convert(&input, 44_101, 441);
        assert_eq!(out, convert_direct(&input, 44_101, 441));
        assert!((out.len() as f64 - 16_000.0).abs() <= 32.0);
    }

    /// Prints how fast each path converts 60 s of 16-bit-range noise. Run it with
    /// `cargo test --release -p sayso-speech resample::tests::bench -- --ignored --nocapture`.
    #[test]
    #[ignore = "benchmark"]
    fn bench_throughput() {
        for rate in [48_000u32, 44_100, 96_000] {
            let input = noise(rate as usize * 60);
            let chunk = rate as usize / 50; // 20 ms, like the audio callback
            let run = |direct: bool| {
                let mut best = f64::MAX;
                for _ in 0..3 {
                    let mut r = if direct { StreamResampler::direct(rate, 16_000) } else { StreamResampler::new(rate, 16_000) };
                    let mut out = Vec::with_capacity(16_000 * 61);
                    let started = std::time::Instant::now();
                    for c in input.chunks(chunk) {
                        r.process(c, &mut out);
                    }
                    best = best.min(started.elapsed().as_secs_f64());
                    std::hint::black_box(&out);
                }
                best
            };
            let (direct, tables) = (run(true), run(false));
            println!(
                "{rate} Hz -> 16 kHz, 60 s: direct {:.1} ms ({:.0}x real time), tables {:.2} ms ({:.0}x real time), {:.1}x faster",
                direct * 1e3,
                60.0 / direct,
                tables * 1e3,
                60.0 / tables,
                direct / tables
            );
        }
    }
}
