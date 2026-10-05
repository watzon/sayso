//! Words that the speaker spells: "send it to Dana cats, that's K A T Z"
//! becomes "send it to Dana katz".
//!
//! This runs before the model of a style. A small model does not do it from a
//! rule: it keeps the wrong word, the cue, and the letters. Capital letters
//! are left to the model, which knows a name from a command.

use crate::dictionary::edit_distance;
use regex::Regex;
use std::sync::LazyLock;

/// A cue and the letters after it. The letters can have spaces, hyphens,
/// commas, or periods between them.
static SPELLING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[,;:]?\s*\b(that's|that’s|that is|spelled|spelt)[,:]?\s+(\p{L}(?:(?: |-|, |\. )\p{L})+)\b\.?")
        .expect("the pattern is valid")
});

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{L}\p{N}'’]+").expect("the pattern is valid"));

/// The most words before a cue that one spelled word can replace: "cube
/// control, spelled k u b e c t l".
const MAX_REPLACED_WORDS: usize = 3;

/// Join the letters of each spelled word, and put the word in place of the
/// cue and of the words that it corrects.
pub fn apply(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(found) = SPELLING.captures(rest) {
        let all = found.get(0).expect("group 0 is the match");
        let letters: Vec<char> = found[2].chars().filter(|c| c.is_alphabetic()).flat_map(char::to_lowercase).collect();
        // "that's a b" is more often a sentence than a spelling.
        let cue = found[1].to_lowercase();
        if letters.len() < 3 && !cue.starts_with("spel") {
            out.push_str(&rest[..all.end()]);
            rest = &rest[all.end()..];
            continue;
        }
        let before = &rest[..all.start()];
        let start = corrected_from(before, &letters);
        let kept = &before[..start.unwrap_or(before.len())];
        out.push_str(kept);
        if start.is_none() && !kept.is_empty() {
            out.push(' ');
        }
        // The spelled word starts as the word that it replaces does.
        let capital = start.is_some_and(|at| before[at..].starts_with(char::is_uppercase));
        let mut word = letters.into_iter();
        if capital && let Some(first) = word.next() {
            out.extend(first.to_uppercase());
        }
        out.extend(word);
        if all.as_str().ends_with('.') {
            out.push('.');
        }
        rest = &rest[all.end()..];
    }
    out.push_str(rest);
    out
}

/// Where the words that `letters` corrects start in `before`: the last one to
/// three words that are most like the spelled word. None when no words are
/// like it, as in "the name is spelled k a t z".
fn corrected_from(before: &str, letters: &[char]) -> Option<usize> {
    let words: Vec<regex::Match> = WORD.find_iter(before).collect();
    // Only words that touch the cue: no sentence end between them.
    if words.last().is_none_or(|last| !before[last.end()..].trim().is_empty()) {
        return None;
    }
    let mut best: Option<(usize, usize)> = None;
    let mut heard: Vec<char> = Vec::new();
    for word in words.iter().rev().take(MAX_REPLACED_WORDS) {
        let mut key: Vec<char> = word.as_str().chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
        key.append(&mut heard);
        heard = key;
        let distance = edit_distance(&heard, letters);
        if distance * 2 <= heard.len().max(letters.len()) && best.is_none_or(|(least, _)| distance < least) {
            best = Some((distance, word.start()));
        }
    }
    best.map(|(_, start)| start)
}

#[cfg(test)]
mod tests {
    use super::apply;

    #[test]
    fn a_spelled_word_replaces_the_word_before_the_cue() {
        assert_eq!(apply("send it to dana cats that's k a t z"), "send it to dana katz");
        assert_eq!(apply("Ask Shawn, that's S-E-A-N, about it."), "Ask Sean, about it.");
        assert_eq!(apply("Her name is Katz, spelled K, A, T, Z."), "Her name is Katz.");
    }

    #[test]
    fn a_spelled_word_can_replace_more_than_one_word() {
        assert_eq!(apply("run cube control spelled k u b e c t l to see the pods"), "run kubectl to see the pods");
        assert_eq!(apply("open engine x spelt n g i n x and reload it"), "open nginx and reload it");
    }

    #[test]
    fn words_that_are_not_like_the_spelled_word_stay() {
        assert_eq!(apply("the name is spelled k a t z"), "the name is katz");
        assert_eq!(apply("spelled k a t z"), "katz");
        assert_eq!(apply("It is done. That is K A T Z speaking."), "It is done. katz speaking.");
    }

    #[test]
    fn two_letters_after_that_is_are_not_a_spelling() {
        assert_eq!(apply("the grade that's a b"), "the grade that's a b");
        assert_eq!(apply("the unit is spelled k g"), "the unit is kg");
    }

    #[test]
    fn text_without_a_spelling_does_not_change() {
        for text in ["", "that's a good idea", "I spelled it wrong and that is all", "plan a b c is ready", "Call me at 5 p m."] {
            assert_eq!(apply(text), text);
        }
    }

    #[test]
    fn each_spelling_in_a_text_is_joined() {
        assert_eq!(apply("ask shawn that's s e a n and john that's j o n"), "ask sean and jon");
    }
}
