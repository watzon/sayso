//! Language codes and names for the language setting and the model catalog.
//!
//! Codes are ISO 639-1 where one exists ("en", "de"), else ISO 639-3 ("yue").
//! [`AUTO`] asks the model to detect the language.

/// The `dictation.language` value that lets the model detect the language.
pub const AUTO: &str = "auto";

/// Code and English name of every language a catalog model lists.
const NAMES: &[(&str, &str)] = &[
    ("af", "Afrikaans"),
    ("am", "Amharic"),
    ("ar", "Arabic"),
    ("as", "Assamese"),
    ("az", "Azerbaijani"),
    ("ba", "Bashkir"),
    ("be", "Belarusian"),
    ("bg", "Bulgarian"),
    ("bn", "Bengali"),
    ("bo", "Tibetan"),
    ("br", "Breton"),
    ("bs", "Bosnian"),
    ("ca", "Catalan"),
    ("cs", "Czech"),
    ("cy", "Welsh"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("en", "English"),
    ("es", "Spanish"),
    ("et", "Estonian"),
    ("eu", "Basque"),
    ("fa", "Persian"),
    ("fi", "Finnish"),
    ("fo", "Faroese"),
    ("fr", "French"),
    ("gl", "Galician"),
    ("gu", "Gujarati"),
    ("ha", "Hausa"),
    ("haw", "Hawaiian"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hr", "Croatian"),
    ("ht", "Haitian Creole"),
    ("hu", "Hungarian"),
    ("hy", "Armenian"),
    ("id", "Indonesian"),
    ("ig", "Igbo"),
    ("is", "Icelandic"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("jw", "Javanese"),
    ("ka", "Georgian"),
    ("kk", "Kazakh"),
    ("km", "Khmer"),
    ("kn", "Kannada"),
    ("ko", "Korean"),
    ("ks", "Kashmiri"),
    ("ku", "Kurdish"),
    ("ky", "Kyrgyz"),
    ("la", "Latin"),
    ("lb", "Luxembourgish"),
    ("ln", "Lingala"),
    ("lo", "Lao"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("mai", "Maithili"),
    ("mg", "Malagasy"),
    ("mi", "Maori"),
    ("mk", "Macedonian"),
    ("ml", "Malayalam"),
    ("mn", "Mongolian"),
    ("mr", "Marathi"),
    ("ms", "Malay"),
    ("mt", "Maltese"),
    ("my", "Burmese"),
    ("ne", "Nepali"),
    ("nl", "Dutch"),
    ("nn", "Norwegian Nynorsk"),
    ("no", "Norwegian"),
    ("oc", "Occitan"),
    ("or", "Odia"),
    ("pa", "Punjabi"),
    ("pl", "Polish"),
    ("ps", "Pashto"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("rw", "Kinyarwanda"),
    ("sa", "Sanskrit"),
    ("sd", "Sindhi"),
    ("si", "Sinhala"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("sn", "Shona"),
    ("so", "Somali"),
    ("sq", "Albanian"),
    ("sr", "Serbian"),
    ("su", "Sundanese"),
    ("sv", "Swedish"),
    ("sw", "Swahili"),
    ("ta", "Tamil"),
    ("te", "Telugu"),
    ("tg", "Tajik"),
    ("th", "Thai"),
    ("tk", "Turkmen"),
    ("tl", "Tagalog"),
    ("tr", "Turkish"),
    ("tt", "Tatar"),
    ("uk", "Ukrainian"),
    ("ur", "Urdu"),
    ("uz", "Uzbek"),
    ("vi", "Vietnamese"),
    ("yi", "Yiddish"),
    ("yo", "Yoruba"),
    ("yue", "Cantonese"),
    ("zh", "Chinese"),
    ("zu", "Zulu"),
];

/// The English name of a language code. None when Sayso does not know the code.
pub fn name(code: &str) -> Option<&'static str> {
    NAMES.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

/// The name to show for a `dictation.language` value.
pub fn display(code: &str) -> String {
    if code == AUTO {
        return "Detect automatically".into();
    }
    name(code).map(str::to_string).unwrap_or_else(|| code.to_string())
}

/// True for [`AUTO`] and for a code Sayso knows.
pub fn is_valid(code: &str) -> bool {
    code == AUTO || name(code).is_some()
}

/// Every known code, sorted by English name. For a model that takes any language.
pub fn all() -> Vec<String> {
    let mut list: Vec<(&str, &str)> = NAMES.to_vec();
    list.sort_by_key(|(_, n)| *n);
    list.into_iter().map(|(c, _)| c.to_string()).collect()
}

fn codes(list: &[&str]) -> Vec<String> {
    list.iter().map(|c| c.to_string()).collect()
}

pub fn english() -> Vec<String> {
    codes(&["en"])
}

/// The 99 languages of OpenAI Whisper.
pub fn whisper() -> Vec<String> {
    codes(&[
        "en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar", "sv", "it", "id", "hi", "fi", "vi",
        "he", "uk", "el", "ms", "cs", "ro", "da", "hu", "ta", "no", "th", "ur", "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk",
        "te", "fa", "lv", "bn", "sr", "az", "sl", "kn", "et", "mk", "br", "eu", "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw",
        "gl", "mr", "pa", "si", "km", "sn", "yo", "so", "af", "oc", "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo",
        "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "haw", "ln", "ha", "ba", "jw", "su",
    ])
}

/// The 25 European languages of Parakeet TDT v3 and the models trained from it.
pub fn parakeet_v3() -> Vec<String> {
    codes(&[
        "en", "de", "es", "fr", "it", "pt", "nl", "pl", "ru", "uk", "sv", "da", "fi", "cs", "sk", "sl", "hr", "bg", "ro", "hu",
        "el", "et", "lv", "lt", "mt",
    ])
}

/// The 14 languages of Cohere Transcribe.
pub fn cohere() -> Vec<String> {
    codes(&["en", "fr", "de", "es", "it", "pt", "nl", "pl", "el", "ar", "ja", "zh", "ko", "vi"])
}

/// The languages SenseVoice Small can be told to use. It also detects more.
pub fn sensevoice() -> Vec<String> {
    codes(&["zh", "yue", "en", "ja", "ko"])
}

/// The languages Nemotron Multilingual has a language prompt for.
pub fn nemotron() -> Vec<String> {
    codes(&[
        "en", "es", "zh", "hi", "ar", "fr", "de", "ja", "ru", "pt", "ko", "it", "nl", "pl", "tr", "uk", "ro", "el", "cs", "hu",
        "sv", "da", "fi", "no", "nn", "sk", "hr", "bg", "lt", "et", "lv", "sl", "th", "vi", "id", "ms", "bn", "ur", "fa", "ta",
        "te", "mr", "gu", "kn", "ml", "si", "ne", "km", "sw", "am", "ha", "zu", "yo", "ig", "af", "rw", "so", "ln", "he", "ku",
        "az", "ka", "hy", "uz", "tg", "ky", "mi", "haw", "mt",
    ])
}

/// The languages of Apple Speech (SpeechTranscriber) on macOS 26.
pub fn apple_speech() -> Vec<String> {
    codes(&[
        "en", "zh", "yue", "ja", "ko", "de", "fr", "es", "it", "pt", "hi", "bn", "gu", "kn", "ks", "mai", "ml", "mr", "ne", "or",
        "pa", "ta", "te", "ur",
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_language_has_a_name() {
        for list in [english(), whisper(), parakeet_v3(), cohere(), sensevoice(), nemotron(), apple_speech()] {
            for code in list {
                assert!(name(&code).is_some(), "no name for {code}");
            }
        }
    }

    #[test]
    fn codes_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for (code, _) in NAMES {
            assert!(seen.insert(code), "{code} is listed twice");
        }
        assert_eq!(whisper().len(), 99);
        assert_eq!(parakeet_v3().len(), 25);
    }

    #[test]
    fn auto_is_valid_and_has_a_label() {
        assert!(is_valid(AUTO));
        assert!(is_valid("de"));
        assert!(!is_valid("klingon"));
        assert_eq!(display(AUTO), "Detect automatically");
        assert_eq!(display("yue"), "Cantonese");
    }
}
