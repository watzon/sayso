//! Spoken punctuation: "comma" becomes ",", "new line" becomes a line break.
//!
//! This step is off by default (`dictation.spoken_punctuation`). Most speech
//! models write punctuation themselves, and then "the trial period" must stay
//! as it is. The commands are English.

use regex::Regex;
use std::sync::LazyLock;

/// A command with the punctuation and spaces that the speech model put around it.
static COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)[ \t]*([,.;:!?]*)[ \t]*\b(new paragraph|new line|full stop|period|comma|question mark|exclamation mark|exclamation point|semicolon|colon)\b[,.;:!?]*[ \t]*",
    )
    .expect("the pattern is valid")
});

/// Change the spoken punctuation commands in `text` into marks and line breaks.
///
/// A mark attaches to the word before it. The word after a sentence end or a
/// line break starts with a capital letter.
pub fn apply(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    // What the command before the current text asks of that text.
    let mut space = false;
    let mut capital = false;
    let push_text = |out: &mut String, part: &str, space: bool, capital: bool| {
        let mut chars = part.chars();
        let Some(first) = chars.next() else { return };
        if space {
            out.push(' ');
        }
        if capital {
            out.extend(first.to_uppercase());
        } else {
            out.push(first);
        }
        out.push_str(chars.as_str());
    };
    for found in COMMAND.captures_iter(text) {
        let all = found.get(0).expect("group 0 is the match");
        push_text(&mut out, &text[last..all.start()], space, capital);
        last = all.end();
        let written = match found[2].to_lowercase().as_str() {
            "new paragraph" => "\n\n",
            "new line" => "\n",
            "comma" => ",",
            "question mark" => "?",
            "exclamation mark" | "exclamation point" => "!",
            "semicolon" => ";",
            "colon" => ":",
            _ => ".",
        };
        let line_break = written.starts_with('\n');
        if line_break {
            // A line break keeps the mark that ends the line before it.
            out.push_str(&found[1]);
        }
        out.push_str(written);
        space = !line_break;
        capital = line_break || matches!(written, "." | "?" | "!");
    }
    push_text(&mut out, &text[last..], space, capital);
    out
}

#[cfg(test)]
mod tests {
    use super::apply;

    #[test]
    fn marks_attach_to_the_word_before_them() {
        assert_eq!(apply("hello comma how are you question mark"), "hello, how are you?");
        assert_eq!(apply("wait semicolon no colon yes exclamation point"), "wait; no: yes!");
    }

    #[test]
    fn a_sentence_end_and_a_line_break_start_a_capital_letter() {
        assert_eq!(apply("it works period ship it full stop"), "it works. Ship it.");
        assert_eq!(apply("first item new line second item new paragraph the end"), "first item\nSecond item\n\nThe end");
        assert_eq!(apply("one comma two"), "one, two");
    }

    #[test]
    fn punctuation_from_the_speech_model_around_a_command_is_removed() {
        assert_eq!(apply("Hello, comma, world. Period."), "Hello, world.");
        assert_eq!(apply("Is it done? New line. Yes."), "Is it done?\nYes.");
    }

    #[test]
    fn commands_match_whole_words_in_any_case() {
        assert_eq!(apply("the periodic table and a comma-free line"), "the periodic table and a, -free line");
        assert_eq!(apply("Done PERIOD"), "Done.");
    }

    #[test]
    fn text_without_a_command_does_not_change() {
        for text in ["", "Send it to Dana on Friday.", "a  b\nc"] {
            assert_eq!(apply(text), text);
        }
    }
}
