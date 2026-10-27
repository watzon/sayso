//! The language that a model finds in the speech, for `final.language`
//! (`docs/engine-protocol.md`, "Detected language").

/// True when the request lets the model find the language: `auto` or an
/// empty `language`.
pub fn is_auto(requested: &str) -> bool {
    let code = requested.trim();
    code.is_empty() || code.eq_ignore_ascii_case("auto")
}

/// A language code as the catalog has it, from the form that a library gives.
/// SenseVoice gives `<|en|>`, Whisper gives `en`. None for an empty value.
pub fn normalize(raw: &str) -> Option<String> {
    let code = raw.trim().trim_start_matches("<|").trim_end_matches("|>").trim().to_ascii_lowercase();
    (!code.is_empty()).then_some(code)
}

/// The languages of the pieces of one recording, by the audio of each piece.
#[derive(Debug, Default)]
pub struct Tally {
    /// Each language and its samples, in the order that the languages came.
    samples: Vec<(String, usize)>,
}

impl Tally {
    /// Add a piece of `samples` samples. A piece with no text or no language
    /// does not count.
    pub fn add(&mut self, samples: usize, text: &str, language: Option<&str>) {
        let Some(language) = language.filter(|_| !text.trim().is_empty()) else {
            return;
        };
        match self.samples.iter_mut().find(|(code, _)| code == language) {
            Some((_, total)) => *total += samples,
            None => self.samples.push((language.to_string(), samples)),
        }
    }

    /// The language of the most audio. With equal audio, the language that came first.
    pub fn most_audio(&self) -> Option<&str> {
        let mut best: Option<&(String, usize)> = None;
        for entry in &self.samples {
            if best.is_none_or(|b| entry.1 > b.1) {
                best = Some(entry);
            }
        }
        best.map(|(code, _)| code.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_or_empty_lets_the_model_find_the_language() {
        for auto in ["auto", "AUTO", "", "  ", " auto "] {
            assert!(is_auto(auto), "{auto:?}");
        }
        for named in ["en", "de-DE", "yue"] {
            assert!(!is_auto(named), "{named}");
        }
    }

    #[test]
    fn library_forms_become_catalog_codes() {
        assert_eq!(normalize("<|en|>").as_deref(), Some("en"));
        assert_eq!(normalize("<|yue|>").as_deref(), Some("yue"));
        assert_eq!(normalize("de").as_deref(), Some("de"));
        assert_eq!(normalize(" EN ").as_deref(), Some("en"));
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("<||>"), None);
    }

    #[test]
    fn the_language_of_the_most_audio_wins() {
        let mut tally = Tally::default();
        tally.add(28 * 16_000, "Guten Tag.", Some("de"));
        tally.add(10 * 16_000, "Hello.", Some("en"));
        tally.add(25 * 16_000, "Hello again.", Some("en"));
        assert_eq!(tally.most_audio(), Some("en"), "35 s of English against 28 s of German");
    }

    #[test]
    fn equal_audio_keeps_the_first_language() {
        let mut tally = Tally::default();
        tally.add(100, "Hallo", Some("de"));
        tally.add(100, "Hello", Some("en"));
        assert_eq!(tally.most_audio(), Some("de"));
    }

    #[test]
    fn pieces_without_text_or_language_do_not_count() {
        let mut tally = Tally::default();
        tally.add(1_000_000, "  ", Some("en"));
        tally.add(1_000_000, "words", None);
        assert_eq!(tally.most_audio(), None);
        tally.add(10, "Hallo", Some("de"));
        assert_eq!(tally.most_audio(), Some("de"));
    }
}
