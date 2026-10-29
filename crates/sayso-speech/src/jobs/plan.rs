//! The plan of a job: the speech parts of the file, and the chunks that go
//! to the engine one at a time (`docs/extensions.md`, "Scheduling").
//!
//! VAD gives the speech parts. The engines and Basic VAD already cut a part
//! at 30 s and leave out speech under 250 ms. Neighbouring parts then join
//! into chunks, so a file has fewer engine calls:
//!
//! - A chunk is at most [`MAX_CHUNK_MS`] long, because a dictation that
//!   starts waits for the chunk that runs. A model without times
//!   (`Timestamps::None`) gets one segment for each chunk, so its chunks are
//!   at most [`MAX_CHUNK_MS_WITHOUT_TIMES`] long.
//! - Two parts join only when the pause between them is at most
//!   [`MAX_JOIN_PAUSE_MS`]. A longer pause would cost engine time on silence.
//! - Each chunk gets [`PAD_MS`] of audio on each side, so the first and the
//!   last sound of the speech are not cut. The padding never goes into the
//!   next chunk or past the length limit.

use super::audio::Audio;
use crate::basic_vad::BasicVad;
use crate::{ModelId, SpeechEngine, SpeechPart, Timestamps};

/// The longest chunk.
pub const MAX_CHUNK_MS: u64 = 30_000;
/// The longest chunk for a model without times.
pub const MAX_CHUNK_MS_WITHOUT_TIMES: u64 = 10_000;
/// The longest pause inside a chunk.
pub const MAX_JOIN_PAUSE_MS: u64 = 3_000;
/// The audio before and after the speech of a chunk.
pub const PAD_MS: u64 = 200;
/// A VAD model gets the file in windows of this length, so a long file does
/// not become floats all at once, and a dictation waits for one window at most.
pub const VAD_WINDOW_MS: u64 = 10 * 60 * 1000;

/// A part of the file that goes to the engine in one call. Times are in
/// milliseconds from the start of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    pub start_ms: u64,
    pub end_ms: u64,
}

/// The longest chunk for a model with this timestamp level.
pub fn max_chunk_ms(timestamps: Timestamps) -> u64 {
    match timestamps {
        Timestamps::None => MAX_CHUNK_MS_WITHOUT_TIMES,
        Timestamps::Word | Timestamps::Segment => MAX_CHUNK_MS,
    }
}

/// Join the speech `parts` of a file of `len_ms` into chunks of at most
/// `max_ms` (see the module).
pub fn chunks(parts: &[SpeechPart], len_ms: u64, max_ms: u64) -> Vec<Chunk> {
    let mut parts: Vec<SpeechPart> = parts
        .iter()
        .map(|p| SpeechPart { start_ms: p.start_ms.min(len_ms), end_ms: p.end_ms.min(len_ms) })
        .filter(|p| p.end_ms > p.start_ms)
        .collect();
    parts.sort_by_key(|p| p.start_ms);

    // Join neighbours.
    let mut joined: Vec<Chunk> = Vec::new();
    for part in parts {
        match joined.last_mut() {
            Some(last)
                if part.start_ms <= last.end_ms.saturating_add(MAX_JOIN_PAUSE_MS)
                    && part.end_ms.max(last.end_ms) - last.start_ms <= max_ms =>
            {
                last.end_ms = last.end_ms.max(part.end_ms);
            }
            _ => joined.push(Chunk { start_ms: part.start_ms, end_ms: part.end_ms }),
        }
    }

    // Pad, without overlap and within the limit.
    let mut padded = Vec::with_capacity(joined.len());
    for (i, chunk) in joined.iter().enumerate() {
        let room = max_ms.saturating_sub(chunk.end_ms - chunk.start_ms) / 2;
        let pad = PAD_MS.min(room);
        let previous_end = padded.last().map_or(0, |c: &Chunk| c.end_ms);
        let next_start = joined.get(i + 1).map_or(len_ms, |c| c.start_ms);
        let start_ms = chunk.start_ms.saturating_sub(pad).max(previous_end);
        let end_ms = (chunk.end_ms + pad).min(next_start).min(len_ms);
        padded.push(Chunk { start_ms, end_ms });
    }
    padded
}

/// The VAD of a job, after the model checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VadRun {
    /// Basic VAD, in this crate.
    Basic,
    /// A VAD model of the engine, loaded.
    Model(ModelId),
}

/// Why the plan stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stopped {
    /// The job was cancelled.
    Cancelled,
    /// The engine failed. The text is English.
    Failed(String),
}

/// The speech parts of `audio`.
///
/// Basic VAD reads the audio one second at a time. A VAD model gets windows
/// of `window_ms` (normally [`VAD_WINDOW_MS`]), and `before_call` runs before
/// each window: it waits while a dictation runs, and stops a cancelled job.
///
/// A speech part that reaches the end of a window can be cut there. So the
/// next window starts at the start of that part, and the part comes from the
/// next window whole.
pub fn speech_parts(
    audio: &Audio,
    vad: &VadRun,
    engine: &dyn SpeechEngine,
    window_ms: u64,
    before_call: &mut dyn FnMut() -> Result<(), Stopped>,
) -> Result<Vec<SpeechPart>, Stopped> {
    let model = match vad {
        VadRun::Basic => return Ok(basic(audio)),
        VadRun::Model(model) => model,
    };
    let len_ms = audio.len_ms();
    let window_ms = window_ms.max(1);
    let mut parts = Vec::new();
    let mut start = 0;
    while start < len_ms {
        before_call()?;
        let end = start.saturating_add(window_ms).min(len_ms);
        let found = engine
            .detect_speech(model, &audio.floats(start, end))
            .map_err(|e| Stopped::Failed(format!("Voice detection failed: {e}")))?;
        let mut found: Vec<SpeechPart> = found
            .into_iter()
            .map(|p| SpeechPart { start_ms: start + p.start_ms, end_ms: (start + p.end_ms).min(end) })
            .collect();
        let mut next = end;
        if end < len_ms
            && let Some(last) = found.last()
            && last.end_ms + REACHES_END_MS >= end
            && last.start_ms > start
        {
            next = last.start_ms;
            found.pop();
        }
        parts.extend(found);
        start = next;
    }
    Ok(parts)
}

/// A part that ends this close to the end of a window may go on after it.
const REACHES_END_MS: u64 = 1_000;

/// Basic VAD on all of `audio`, one second at a time.
fn basic(audio: &Audio) -> Vec<SpeechPart> {
    let mut vad = BasicVad::new();
    let mut parts = Vec::new();
    let mut floats = Vec::with_capacity(16_000);
    for second in audio.samples().chunks(16_000) {
        floats.clear();
        floats.extend(second.iter().map(|&s| f32::from(s) / 32768.0));
        parts.extend(vad.accept(&floats));
    }
    parts.extend(vad.finish());
    parts.into_iter().map(|p| SpeechPart { start_ms: p.start_ms, end_ms: p.end_ms }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(start_ms: u64, end_ms: u64) -> SpeechPart {
        SpeechPart { start_ms, end_ms }
    }

    fn chunk(start_ms: u64, end_ms: u64) -> Chunk {
        Chunk { start_ms, end_ms }
    }

    #[test]
    fn close_parts_join_and_get_padding() {
        let parts = [part(1_000, 5_000), part(6_000, 9_000), part(20_000, 25_000)];
        assert_eq!(chunks(&parts, 60_000, MAX_CHUNK_MS), [chunk(800, 9_200), chunk(19_800, 25_200)]);
    }

    #[test]
    fn a_long_pause_ends_a_chunk() {
        let parts = [part(0, 2_000), part(2_000 + MAX_JOIN_PAUSE_MS + 1, 8_000)];
        assert_eq!(chunks(&parts, 8_000, MAX_CHUNK_MS).len(), 2);
        let parts = [part(0, 2_000), part(2_000 + MAX_JOIN_PAUSE_MS, 8_000)];
        assert_eq!(chunks(&parts, 8_000, MAX_CHUNK_MS), [chunk(0, 8_000)]);
    }

    #[test]
    fn chunks_stay_under_the_limit() {
        // Parts of 30 s from VAD, back to back, and many short parts.
        let long: Vec<SpeechPart> = (0..4).map(|i| part(i * 30_000, (i + 1) * 30_000)).collect();
        let short: Vec<SpeechPart> = (0..100).map(|i| part(i * 1_000, i * 1_000 + 700)).collect();
        for (parts, max) in [(long, MAX_CHUNK_MS), (short.clone(), MAX_CHUNK_MS), (short, MAX_CHUNK_MS_WITHOUT_TIMES)] {
            let found = chunks(&parts, 200_000, max);
            assert!(found.iter().all(|c| c.end_ms - c.start_ms <= max), "{found:?}");
            assert!(found.windows(2).all(|w| w[0].end_ms <= w[1].start_ms), "{found:?}");
        }
        // 100 short parts in 100 s, with chunks of 10 s at most: about 10 chunks.
        let short: Vec<SpeechPart> = (0..100).map(|i| part(i * 1_000, i * 1_000 + 700)).collect();
        let found = chunks(&short, 200_000, MAX_CHUNK_MS_WITHOUT_TIMES);
        assert!((10..=12).contains(&found.len()), "{}", found.len());
    }

    #[test]
    fn padding_stops_at_the_file_and_the_next_chunk() {
        let parts = [part(50, 1_000), part(4_100, 4_950)];
        assert_eq!(chunks(&parts, 5_000, MAX_CHUNK_MS), [chunk(0, 1_200), chunk(3_900, 5_000)]);
        // A part of exactly the limit gets no padding.
        assert_eq!(chunks(&[part(1_000, 31_000)], 40_000, MAX_CHUNK_MS), [chunk(1_000, 31_000)]);
    }

    #[test]
    fn parts_out_of_order_or_past_the_end_are_cleaned() {
        let parts = [part(9_000, 12_000), part(1_000, 2_000), part(3_000, 3_000)];
        assert_eq!(chunks(&parts, 10_000, MAX_CHUNK_MS), [chunk(800, 2_200), chunk(8_800, 10_000)]);
        assert!(chunks(&[], 10_000, MAX_CHUNK_MS).is_empty());
    }
}
