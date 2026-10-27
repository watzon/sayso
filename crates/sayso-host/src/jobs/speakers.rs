//! The speakers of a job (`docs/extensions.md`, "Diarization models").
//!
//! The engine gives turns: who speaks when, with speakers from 0. The host
//! then:
//!
//! 1. Folds a speaker with very little speech into another speaker
//!    ([`fold_small_speakers`]), when the extension did not give the number of
//!    speakers. Without a number, the diarizers find extra speakers that say
//!    almost nothing.
//! 2. Gives each segment the speaker with the most time in the segment
//!    ([`assign`]). A segment is not split at a change of speaker: the
//!    segments go to the extension while the job runs, and diarization runs
//!    after the last one. Segments of `word` models end at sentence ends and
//!    pauses of 700 ms, where speakers usually change.
//! 3. Makes the turns of the `speakers` event from the segments
//!    ([`segment_turns`]): each run of segments of one speaker is one turn, so
//!    each segment is inside exactly one turn. The labels are `"1"`, `"2"`,
//!    ... in the order in which the speakers first speak.

use sayso_core::stt::Turn;
use std::collections::{BTreeMap, HashMap};

/// A speaker with less than this share of all speech is small...
pub const SMALL_SPEAKER_SHARE: f64 = 0.02;
/// ...when it also has less than this much speech, in milliseconds.
pub const SMALL_SPEAKER_MS: u64 = 10_000;

/// Fold each small speaker (less than [`SMALL_SPEAKER_SHARE`] of all speech
/// and less than [`SMALL_SPEAKER_MS`]) into the other speakers. Each turn of a
/// small speaker goes to the speaker that overlaps it most, else to the
/// speaker with the nearest turn. The speaker with the most speech always
/// stays. The turns stay in the order of `start_ms`.
pub fn fold_small_speakers(turns: &[Turn]) -> Vec<Turn> {
    let mut speech: BTreeMap<u32, u64> = BTreeMap::new();
    for turn in turns {
        *speech.entry(turn.speaker).or_default() += turn.end_ms.saturating_sub(turn.start_ms);
    }
    let all: u64 = speech.values().sum();
    let largest = speech.iter().max_by_key(|&(speaker, ms)| (*ms, std::cmp::Reverse(*speaker))).map(|(s, _)| *s);
    let small = |speaker: u32| {
        let ms = speech[&speaker];
        Some(speaker) != largest && ms < SMALL_SPEAKER_MS && (ms as f64) < SMALL_SPEAKER_SHARE * all as f64
    };
    let kept: Vec<&Turn> = turns.iter().filter(|t| !small(t.speaker)).collect();
    let mut folded: Vec<Turn> = turns
        .iter()
        .map(|turn| {
            if !small(turn.speaker) {
                return *turn;
            }
            let speaker = best_speaker(turn.start_ms, turn.end_ms, kept.iter().copied()).unwrap_or(turn.speaker);
            Turn { speaker, ..*turn }
        })
        .collect();
    folded.sort_by_key(|t| (t.start_ms, t.end_ms));
    folded
}

/// The speaker of each segment (`(start_ms, end_ms)`): the speaker with the
/// most time inside the segment, else the speaker of the nearest turn. None
/// when there are no turns.
pub fn assign(segments: &[(u64, u64)], turns: &[Turn]) -> Vec<Option<u32>> {
    segments.iter().map(|&(start, end)| best_speaker(start, end, turns.iter())).collect()
}

/// The turns of the `speakers` event: one for each run of segments with the
/// same speaker, from the start of the first segment to the end of the last.
/// `speakers` has the speaker of each segment, from [`assign`]. A segment
/// without a speaker is in no turn.
pub fn segment_turns(segments: &[(u64, u64)], speakers: &[Option<u32>]) -> Vec<sayso_protocol::Turn> {
    let mut labels: HashMap<u32, String> = HashMap::new();
    let mut turns: Vec<(u32, u64, u64)> = Vec::new();
    let mut last_speaker = None;
    for (&(start, end), &speaker) in segments.iter().zip(speakers) {
        match (speaker, turns.last_mut()) {
            (Some(speaker), Some(turn)) if last_speaker == Some(speaker) => turn.2 = turn.2.max(end),
            (Some(speaker), _) => turns.push((speaker, start, end)),
            (None, _) => {}
        }
        last_speaker = speaker;
    }
    turns
        .into_iter()
        .map(|(speaker, start_ms, end_ms)| {
            let next = labels.len() + 1;
            let label = labels.entry(speaker).or_insert_with(|| next.to_string()).clone();
            sayso_protocol::Turn { start_ms, end_ms, speaker: label }
        })
        .collect()
}

/// The speaker of `turns` with the most time in `start..end`. When no turn
/// overlaps, the speaker of the nearest turn. A tie goes to the speaker with
/// the lower number, who spoke first.
fn best_speaker<'a>(start: u64, end: u64, turns: impl Iterator<Item = &'a Turn>) -> Option<u32> {
    let mut overlap: BTreeMap<u32, u64> = BTreeMap::new();
    let mut nearest: Option<(u64, u32)> = None;
    for turn in turns {
        let shared = end.min(turn.end_ms).saturating_sub(start.max(turn.start_ms));
        if shared > 0 {
            *overlap.entry(turn.speaker).or_default() += shared;
        }
        let gap = turn.start_ms.saturating_sub(end).max(start.saturating_sub(turn.end_ms));
        if nearest.is_none_or(|(g, s)| (gap, turn.speaker) < (g, s)) {
            nearest = Some((gap, turn.speaker));
        }
    }
    let most = overlap.iter().max_by_key(|&(speaker, ms)| (*ms, std::cmp::Reverse(*speaker))).map(|(s, _)| *s);
    most.or(nearest.map(|(_, speaker)| speaker))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(start_ms: u64, end_ms: u64, speaker: u32) -> Turn {
        Turn { start_ms, end_ms, speaker }
    }

    fn labels(turns: &[sayso_protocol::Turn]) -> Vec<(u64, u64, &str)> {
        turns.iter().map(|t| (t.start_ms, t.end_ms, t.speaker.as_str())).collect()
    }

    #[test]
    fn a_segment_gets_the_speaker_with_the_most_overlap() {
        let turns = [turn(0, 4_000, 0), turn(3_000, 10_000, 1), turn(12_000, 15_000, 0)];
        let segments = [(0, 3_500), (3_500, 9_000), (10_500, 11_000), (11_900, 14_000), (20_000, 21_000)];
        // The third segment overlaps no turn: the nearest turn is speaker 1 (500 ms away).
        assert_eq!(assign(&segments, &turns), [Some(0), Some(1), Some(1), Some(0), Some(0)]);
        assert_eq!(assign(&segments, &[]), [None; 5]);
        // A tie goes to the speaker who spoke first.
        assert_eq!(assign(&[(0, 2_000)], &[turn(0, 1_000, 1), turn(1_000, 2_000, 0)]), [Some(0)]);
    }

    #[test]
    fn labels_follow_the_first_speech_and_runs_join() {
        // The engine numbers speakers by first speech, but folding and the
        // segments can change the order: the labels follow the segments.
        let segments = [(0, 1_000), (1_000, 2_000), (2_500, 3_000), (3_000, 4_000), (5_000, 6_000)];
        let speakers = [Some(2), Some(2), Some(0), None, Some(2)];
        assert_eq!(labels(&segment_turns(&segments, &speakers)), [(0, 2_000, "1"), (2_500, 3_000, "2"), (5_000, 6_000, "1")]);
        assert!(segment_turns(&[], &[]).is_empty());
    }

    #[test]
    fn a_speaker_with_very_little_speech_is_folded() {
        // Speaker 0 and 1 speak for minutes. Speaker 2 says 1 s inside a turn
        // of speaker 1, and speaker 3 says 1.5 s alone, nearest to speaker 0.
        let turns = [
            turn(0, 100_000, 0),
            turn(100_000, 200_000, 1),
            turn(150_000, 151_000, 2),
            turn(200_000, 300_000, 0),
            turn(400_000, 401_500, 3),
        ];
        let folded = fold_small_speakers(&turns);
        let speakers: Vec<u32> = folded.iter().map(|t| t.speaker).collect();
        assert_eq!(speakers, [0, 1, 1, 0, 0]);
        assert_eq!(folded.len(), turns.len(), "the times stay");
    }

    #[test]
    fn a_short_speaker_in_a_short_file_or_with_a_real_share_stays() {
        // 9 s of 60 s is 15 %: a real speaker, although under 10 s.
        let turns = [turn(0, 51_000, 0), turn(51_000, 60_000, 1)];
        assert_eq!(fold_small_speakers(&turns), turns);
        // 1 % of a long meeting, but 30 s of speech: a real speaker.
        let turns = [turn(0, 3_000_000, 0), turn(3_000_000, 3_030_000, 1)];
        assert_eq!(fold_small_speakers(&turns), turns);
        // Both rules hold: folded.
        let turns = [turn(0, 3_000_000, 0), turn(3_000_000, 3_009_000, 1)];
        assert_eq!(fold_small_speakers(&turns), [turn(0, 3_000_000, 0), turn(3_000_000, 3_009_000, 0)]);
    }

    #[test]
    fn the_largest_speaker_and_files_without_turns_stay() {
        assert_eq!(fold_small_speakers(&[turn(0, 500, 0)]), [turn(0, 500, 0)]);
        assert!(fold_small_speakers(&[]).is_empty());
    }
}
