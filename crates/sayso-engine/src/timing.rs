//! Times of words and segments for `transcribe` with `segments: true`
//! (`docs/engine-protocol.md`, "Segments").

use serde_json::{Value, json};

/// One word and its time, in milliseconds from the start of the audio.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// A part of the text with its time.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    /// The words, when the model gives word times.
    pub words: Option<Vec<Word>>,
}

/// The timing that a final pass gives with its text.
#[derive(Debug, Clone, PartialEq)]
pub enum Timing {
    /// The model gives no times.
    None,
    /// Word times (timestamp level `word`).
    Words(Vec<Word>),
    /// Segments of the model (timestamp level `segment`).
    Segments(Vec<TextSegment>),
}

/// The text of a final pass and its timing.
#[derive(Debug, Clone, PartialEq)]
pub struct Timed {
    pub text: String,
    pub timing: Timing,
}

impl Timed {
    pub fn untimed(text: String) -> Self {
        Timed { text, timing: Timing::None }
    }
}

/// A segment ends before a pause of this length between two words.
const PAUSE_MS: u64 = 700;
/// A segment ends before it would pass this length.
const MAX_SEGMENT_MS: u64 = 20_000;
/// The end of a token without a duration and without a token after it.
const LAST_TOKEN_MS: f32 = 400.0;

/// Words from the tokens of a model and the start time of each token, in
/// seconds. A token that starts with a space or `▁` starts a word, and so
/// does each CJK character. Other tokens (word pieces, punctuation) join the
/// word before them. `durations` gives the length of each token, when the
/// model has it. `audio_ms` is the length of the audio.
pub fn words_from_tokens(tokens: &[String], starts: &[f32], durations: Option<&[f32]>, audio_ms: u64) -> Vec<Word> {
    let count = tokens.len().min(starts.len());
    let mut words: Vec<Word> = Vec::new();
    // A token that is only a space marks the token after it.
    let mut space_before = false;
    for i in 0..count {
        let token = &tokens[i];
        let start = (starts[i].max(0.0) * 1000.0) as u64;
        let end = match durations.and_then(|d| d.get(i)) {
            Some(&d) if d > 0.0 => ((starts[i] + d).max(0.0) * 1000.0) as u64,
            _ => match starts.get(i + 1) {
                Some(&next) if i + 1 < count => (next.max(0.0) * 1000.0) as u64,
                _ => (starts[i].max(0.0) * 1000.0 + LAST_TOKEN_MS) as u64,
            },
        }
        .clamp(start, audio_ms.max(start));
        let marked = std::mem::take(&mut space_before) || token.starts_with(' ') || token.starts_with('▁');
        let piece = token.replace('▁', " ");
        let piece = piece.trim();
        if piece.is_empty() {
            space_before = marked || !token.is_empty();
            continue;
        }
        let cjk = piece.chars().next().is_some_and(is_cjk);
        match words.last_mut() {
            Some(word) if !marked && !cjk && !word.text.chars().last().is_some_and(is_cjk) => {
                word.text.push_str(piece);
                word.end_ms = word.end_ms.max(end);
            }
            // Punctuation after a CJK character joins it.
            Some(word) if !marked && !cjk && piece.chars().all(|c| c.is_ascii_punctuation() || is_cjk_punctuation(c)) => {
                word.text.push_str(piece);
                word.end_ms = word.end_ms.max(end);
            }
            _ => words.push(Word { start_ms: start, end_ms: end, text: piece.to_string() }),
        }
    }
    words
}

/// Cut words into segments: after a word that ends a sentence, before a pause
/// of 700 ms or more, and before a segment passes 20 s.
pub fn segments_from_words(words: &[Word]) -> Vec<TextSegment> {
    let mut segments = Vec::new();
    let mut current: Vec<Word> = Vec::new();
    for word in words {
        if let (Some(first), Some(last)) = (current.first(), current.last()) {
            let pause = word.start_ms.saturating_sub(last.end_ms) >= PAUSE_MS;
            let too_long = word.end_ms.saturating_sub(first.start_ms) > MAX_SEGMENT_MS;
            if pause || too_long {
                segments.push(segment_of(std::mem::take(&mut current)));
            }
        }
        current.push(word.clone());
        if ends_sentence(&word.text) {
            segments.push(segment_of(std::mem::take(&mut current)));
        }
    }
    if !current.is_empty() {
        segments.push(segment_of(current));
    }
    segments
}

fn segment_of(words: Vec<Word>) -> TextSegment {
    TextSegment {
        start_ms: words.first().map_or(0, |w| w.start_ms),
        end_ms: words.last().map_or(0, |w| w.end_ms),
        text: join_words(&words),
        words: Some(words),
    }
}

/// The text of words: a space between two words, except between CJK text.
fn join_words(words: &[Word]) -> String {
    let mut text = String::new();
    for word in words {
        let cjk_join = text.chars().last().is_some_and(is_cjk_or_punctuation) && word.text.chars().next().is_some_and(is_cjk);
        if !text.is_empty() && !cjk_join {
            text.push(' ');
        }
        text.push_str(&word.text);
    }
    text
}

fn ends_sentence(text: &str) -> bool {
    text.trim_end_matches(['"', '\'', ')', '”', '’', '」', '』'])
        .ends_with(['.', '?', '!', '。', '？', '！'])
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF   // Hiragana, Katakana
        | 0x3400..=0x4DBF // CJK Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0xAC00..=0xD7AF // Hangul syllables
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
    )
}

fn is_cjk_punctuation(c: char) -> bool {
    matches!(c as u32, 0x3000..=0x303F | 0xFF00..=0xFFEF)
}

fn is_cjk_or_punctuation(c: char) -> bool {
    is_cjk(c) || is_cjk_punctuation(c)
}

/// One piece of the audio that the final pass transcribed alone.
pub struct Piece {
    pub start_ms: u64,
    pub end_ms: u64,
    pub timed: Timed,
}

/// The segments of a whole recording from its pieces. Times move from the
/// start of each piece to the start of the recording. A piece without times
/// is one segment.
pub fn segments(pieces: &[Piece]) -> Vec<TextSegment> {
    let mut all = Vec::new();
    for piece in pieces {
        let shift = |ms: u64| (ms + piece.start_ms).min(piece.end_ms);
        match &piece.timed.timing {
            Timing::Words(words) => {
                for mut segment in segments_from_words(words) {
                    segment.start_ms = shift(segment.start_ms);
                    segment.end_ms = shift(segment.end_ms);
                    for word in segment.words.iter_mut().flatten() {
                        word.start_ms = shift(word.start_ms);
                        word.end_ms = shift(word.end_ms);
                    }
                    all.push(segment);
                }
            }
            Timing::Segments(segments) => all.extend(segments.iter().map(|s| TextSegment {
                start_ms: shift(s.start_ms),
                end_ms: shift(s.end_ms),
                text: s.text.trim().to_string(),
                words: None,
            })),
            Timing::None => all.push(TextSegment {
                start_ms: piece.start_ms,
                end_ms: piece.end_ms,
                text: piece.timed.text.trim().to_string(),
                words: None,
            }),
        }
    }
    all.retain(|s| !s.text.is_empty());
    // In order and without overlaps, also when a model gives a time twice.
    let mut last_end = 0;
    for segment in &mut all {
        segment.start_ms = segment.start_ms.max(last_end);
        segment.end_ms = segment.end_ms.max(segment.start_ms);
        last_end = segment.end_ms;
    }
    all
}

/// The `segments` field of the `final` reply.
pub fn to_json(segments: &[TextSegment]) -> Value {
    Value::Array(
        segments
            .iter()
            .map(|s| {
                let mut value = json!({ "start_ms": s.start_ms, "end_ms": s.end_ms, "text": s.text });
                if let Some(words) = &s.words {
                    value["words"] = words
                        .iter()
                        .map(|w| json!({ "start_ms": w.start_ms, "end_ms": w.end_ms, "text": w.text }))
                        .collect();
                }
                value
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(list: &[&str]) -> Vec<String> {
        list.iter().map(|t| t.to_string()).collect()
    }

    fn word(start_ms: u64, end_ms: u64, text: &str) -> Word {
        Word { start_ms, end_ms, text: text.into() }
    }

    #[test]
    fn word_pieces_and_punctuation_join_their_word() {
        let t = tokens(&[" Hel", "lo", " there", ".", " How", " are", " you", "?"]);
        let s = [0.32, 0.48, 0.70, 1.10, 1.90, 2.30, 2.60, 2.90];
        let words = words_from_tokens(&t, &s, None, 3_500);
        let texts: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["Hello", "there.", "How", "are", "you?"]);
        assert_eq!(words[0].start_ms, 320);
        assert_eq!(words[0].end_ms, 700);
        assert_eq!(words[1].end_ms, 1_900);
        assert_eq!(words[4].end_ms, 3_300, "the last token gets 400 ms");
    }

    #[test]
    fn sentencepiece_marks_and_durations_work() {
        let t = tokens(&["▁Hi", "▁all"]);
        let words = words_from_tokens(&t, &[0.1, 0.5], Some(&[0.2, 0.3]), 2_000);
        assert_eq!(words, [word(100, 300, "Hi"), word(500, 800, "all")]);
    }

    #[test]
    fn a_lone_space_token_starts_the_next_word() {
        let t = tokens(&["▁", "Hi", "▁", "all"]);
        let texts: Vec<_> = words_from_tokens(&t, &[0.0, 0.1, 0.3, 0.4], None, 1_000).into_iter().map(|w| w.text).collect();
        assert_eq!(texts, ["Hi", "all"]);
    }

    #[test]
    fn each_cjk_character_is_a_word_and_joins_without_spaces() {
        let t = tokens(&["你", "好", "。", "再", "见"]);
        let words = words_from_tokens(&t, &[0.0, 0.2, 0.4, 0.5, 0.7], None, 1_000);
        let texts: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["你", "好。", "再", "见"]);
        let segments = segments_from_words(&words);
        assert_eq!(segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["你好。", "再见"]);
    }

    #[test]
    fn segments_end_at_sentences_pauses_and_twenty_seconds() {
        let words = [
            word(0, 300, "One."),
            word(400, 700, "two"),
            word(1_500, 1_800, "three"),
            word(1_900, 21_600, "long"),
            word(21_700, 22_000, "four"),
        ];
        let texts: Vec<_> = segments_from_words(&words).into_iter().map(|s| s.text).collect();
        assert_eq!(texts, ["One.", "two", "three", "long", "four"]);
    }

    #[test]
    fn pieces_move_to_the_time_of_the_recording() {
        let pieces = [
            Piece { start_ms: 0, end_ms: 10_000, timed: Timed { text: "a b.".into(), timing: Timing::Words(vec![word(100, 200, "a"), word(300, 400, "b.")]) } },
            Piece { start_ms: 10_000, end_ms: 20_000, timed: Timed::untimed("rest".into()) },
            Piece { start_ms: 20_000, end_ms: 25_000, timed: Timed::untimed("  ".into()) },
            Piece {
                start_ms: 25_000,
                end_ms: 30_000,
                timed: Timed { text: "x".into(), timing: Timing::Segments(vec![TextSegment { start_ms: 1_000, end_ms: 9_000, text: " x ".into(), words: None }]) },
            },
        ];
        let all = segments(&pieces);
        assert_eq!(all.len(), 3, "the empty piece is left out");
        assert_eq!((all[0].start_ms, all[0].end_ms, all[0].text.as_str()), (100, 400, "a b."));
        assert_eq!((all[1].start_ms, all[1].end_ms, all[1].text.as_str()), (10_000, 20_000, "rest"));
        assert_eq!((all[2].start_ms, all[2].end_ms, all[2].text.as_str()), (26_000, 30_000, "x"), "clamped to the piece");
        let json = to_json(&all);
        assert_eq!(json[0]["words"][1]["start_ms"], 300);
        assert!(json[1].get("words").is_none());
    }

    #[test]
    fn segments_never_overlap() {
        let pieces = [Piece {
            start_ms: 0,
            end_ms: 5_000,
            timed: Timed {
                text: "a. b.".into(),
                timing: Timing::Segments(vec![
                    TextSegment { start_ms: 0, end_ms: 2_000, text: "a.".into(), words: None },
                    TextSegment { start_ms: 1_500, end_ms: 1_800, text: "b.".into(), words: None },
                ]),
            },
        }];
        let all = segments(&pieces);
        assert_eq!((all[1].start_ms, all[1].end_ms), (2_000, 2_000));
    }
}
