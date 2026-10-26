//! The result of diarization (`docs/engine-protocol.md`, "Diarization").
//!
//! The model ([`crate::recognizer::Diarizer`]) gives turns with its own speaker
//! numbers, in seconds. [`finish`] makes the turns of the protocol: whole
//! milliseconds, speakers numbered from 0 in the order in which they first speak, the
//! turns of one speaker joined where they touch or overlap, and the turns in order.

use serde_json::{Value, json};
use std::collections::HashMap;

/// The clustering threshold when the client gives no number of speakers. It is the default of
/// sherpa-onnx. A lower value finds more speakers.
pub const CLUSTER_THRESHOLD: f32 = 0.5;

/// A turn as the model gives it. The speaker number is the number of the model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawTurn {
    pub start: f32,
    pub end: f32,
    pub speaker: i32,
}

/// A turn of the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: u32,
}

/// The reply of `diarize`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diarized {
    pub speakers: u32,
    pub turns: Vec<Turn>,
}

impl Diarized {
    pub fn to_json(&self) -> Value {
        let turns: Vec<Value> = self
            .turns
            .iter()
            .map(|t| json!({ "start_ms": t.start_ms, "end_ms": t.end_ms, "speaker": t.speaker }))
            .collect();
        json!({ "speakers": self.speakers, "turns": turns })
    }
}

/// Make the turns of the protocol. `audio_ms` is the length of the audio: no turn passes it.
pub fn finish(raw: &[RawTurn], audio_ms: u64) -> Diarized {
    let ms = |seconds: f32| ((f64::from(seconds) * 1000.0).round().max(0.0) as u64).min(audio_ms);
    let mut turns: Vec<(u64, u64, i32)> = raw
        .iter()
        .map(|t| (ms(t.start), ms(t.end), t.speaker))
        .filter(|&(start, end, _)| end > start)
        .collect();
    // The order of the first turns gives the numbers.
    turns.sort_by_key(|&(start, end, speaker)| (start, end, speaker));
    let mut numbers: HashMap<i32, u32> = HashMap::new();
    for &(_, _, speaker) in &turns {
        let next = numbers.len() as u32;
        numbers.entry(speaker).or_insert(next);
    }
    // Join the turns of each speaker. The turns of one speaker are in start order now.
    let mut joined: Vec<Turn> = Vec::new();
    let mut last_of: HashMap<u32, usize> = HashMap::new();
    for (start_ms, end_ms, speaker) in turns {
        let speaker = numbers[&speaker];
        match last_of.get(&speaker) {
            Some(&at) if start_ms <= joined[at].end_ms => joined[at].end_ms = joined[at].end_ms.max(end_ms),
            _ => {
                last_of.insert(speaker, joined.len());
                joined.push(Turn { start_ms, end_ms, speaker });
            }
        }
    }
    joined.sort_by_key(|t| (t.start_ms, t.end_ms, t.speaker));
    Diarized { speakers: numbers.len() as u32, turns: joined }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(start: f32, end: f32, speaker: i32) -> RawTurn {
        RawTurn { start, end, speaker }
    }

    #[test]
    fn speakers_are_numbered_in_the_order_they_first_speak() {
        let found = finish(&[raw(5.0, 6.0, 0), raw(1.0, 2.0, 2), raw(3.0, 4.0, 1), raw(7.0, 8.0, 2)], 10_000);
        let speakers: Vec<u32> = found.turns.iter().map(|t| t.speaker).collect();
        assert_eq!(speakers, [0, 1, 2, 0], "the model's 2 spoke first, then 1, then 0");
        assert_eq!(found.speakers, 3);
        let starts: Vec<u64> = found.turns.iter().map(|t| t.start_ms).collect();
        assert_eq!(starts, [1000, 3000, 5000, 7000]);
    }

    #[test]
    fn turns_of_one_speaker_that_overlap_or_touch_become_one() {
        let found = finish(
            &[raw(0.0, 2.0, 0), raw(1.5, 3.0, 0), raw(3.0, 3.5, 0), raw(4.0, 5.0, 0), raw(2.0, 2.5, 1)],
            10_000,
        );
        assert_eq!(
            found.turns,
            [
                Turn { start_ms: 0, end_ms: 3500, speaker: 0 },
                Turn { start_ms: 2000, end_ms: 2500, speaker: 1 },
                Turn { start_ms: 4000, end_ms: 5000, speaker: 0 },
            ],
            "two speakers may overlap each other; one speaker may not overlap itself"
        );
        assert_eq!(found.speakers, 2);
    }

    #[test]
    fn a_turn_inside_a_longer_turn_of_the_same_speaker_does_not_shorten_it() {
        let found = finish(&[raw(0.0, 5.0, 3), raw(1.0, 2.0, 3)], 10_000);
        assert_eq!(found.turns, [Turn { start_ms: 0, end_ms: 5000, speaker: 0 }]);
    }

    #[test]
    fn times_are_whole_milliseconds_inside_the_audio_and_empty_turns_go() {
        let found = finish(&[raw(0.0004, 1.2346, 0), raw(2.0, 2.0, 1), raw(9.5, 12.0, 0), raw(-1.0, 0.5, 0)], 10_000);
        let spans: Vec<(u64, u64)> = found.turns.iter().map(|t| (t.start_ms, t.end_ms)).collect();
        assert_eq!(spans, [(0, 1235), (9500, 10_000)]);
        assert_eq!(found.speakers, 1, "a speaker with only an empty turn does not count");
    }

    #[test]
    fn no_turns_means_no_speakers() {
        let found = finish(&[], 10_000);
        assert_eq!(found, Diarized { speakers: 0, turns: Vec::new() });
        assert_eq!(found.to_json(), json!({"speakers": 0, "turns": []}));
    }
}
