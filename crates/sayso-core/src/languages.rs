//! Language codes and names for the language setting and the model catalog.
//!
//! Codes are ISO 639-1 where one exists ("en", "de"), else ISO 639-3 ("yue").
//! [`AUTO`] asks the model to detect the language.

use sayso_i18n::t;

/// The `dictation.language` value that lets the model detect the language.
pub const AUTO: &str = "auto";

/// Code and English name of every language a catalog model lists. The
/// interface shows [`display`], the name in the interface language.
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

/// The name to show for a `dictation.language` value, in the interface
/// language. The code itself when Sayso does not know it.
pub fn display(code: &str) -> String {
    match code {
        AUTO => t!("languages.auto"),
        "af" => t!("languages.af"),
        "am" => t!("languages.am"),
        "ar" => t!("languages.ar"),
        "as" => t!("languages.as"),
        "az" => t!("languages.az"),
        "ba" => t!("languages.ba"),
        "be" => t!("languages.be"),
        "bg" => t!("languages.bg"),
        "bn" => t!("languages.bn"),
        "bo" => t!("languages.bo"),
        "br" => t!("languages.br"),
        "bs" => t!("languages.bs"),
        "ca" => t!("languages.ca"),
        "cs" => t!("languages.cs"),
        "cy" => t!("languages.cy"),
        "da" => t!("languages.da"),
        "de" => t!("languages.de"),
        "el" => t!("languages.el"),
        "en" => t!("languages.en"),
        "es" => t!("languages.es"),
        "et" => t!("languages.et"),
        "eu" => t!("languages.eu"),
        "fa" => t!("languages.fa"),
        "fi" => t!("languages.fi"),
        "fo" => t!("languages.fo"),
        "fr" => t!("languages.fr"),
        "gl" => t!("languages.gl"),
        "gu" => t!("languages.gu"),
        "ha" => t!("languages.ha"),
        "haw" => t!("languages.haw"),
        "he" => t!("languages.he"),
        "hi" => t!("languages.hi"),
        "hr" => t!("languages.hr"),
        "ht" => t!("languages.ht"),
        "hu" => t!("languages.hu"),
        "hy" => t!("languages.hy"),
        "id" => t!("languages.id"),
        "ig" => t!("languages.ig"),
        "is" => t!("languages.is"),
        "it" => t!("languages.it"),
        "ja" => t!("languages.ja"),
        "jw" => t!("languages.jw"),
        "ka" => t!("languages.ka"),
        "kk" => t!("languages.kk"),
        "km" => t!("languages.km"),
        "kn" => t!("languages.kn"),
        "ko" => t!("languages.ko"),
        "ks" => t!("languages.ks"),
        "ku" => t!("languages.ku"),
        "ky" => t!("languages.ky"),
        "la" => t!("languages.la"),
        "lb" => t!("languages.lb"),
        "ln" => t!("languages.ln"),
        "lo" => t!("languages.lo"),
        "lt" => t!("languages.lt"),
        "lv" => t!("languages.lv"),
        "mai" => t!("languages.mai"),
        "mg" => t!("languages.mg"),
        "mi" => t!("languages.mi"),
        "mk" => t!("languages.mk"),
        "ml" => t!("languages.ml"),
        "mn" => t!("languages.mn"),
        "mr" => t!("languages.mr"),
        "ms" => t!("languages.ms"),
        "mt" => t!("languages.mt"),
        "my" => t!("languages.my"),
        "ne" => t!("languages.ne"),
        "nl" => t!("languages.nl"),
        "nn" => t!("languages.nn"),
        "no" => t!("languages.no"),
        "oc" => t!("languages.oc"),
        "or" => t!("languages.or"),
        "pa" => t!("languages.pa"),
        "pl" => t!("languages.pl"),
        "ps" => t!("languages.ps"),
        "pt" => t!("languages.pt"),
        "ro" => t!("languages.ro"),
        "ru" => t!("languages.ru"),
        "rw" => t!("languages.rw"),
        "sa" => t!("languages.sa"),
        "sd" => t!("languages.sd"),
        "si" => t!("languages.si"),
        "sk" => t!("languages.sk"),
        "sl" => t!("languages.sl"),
        "sn" => t!("languages.sn"),
        "so" => t!("languages.so"),
        "sq" => t!("languages.sq"),
        "sr" => t!("languages.sr"),
        "su" => t!("languages.su"),
        "sv" => t!("languages.sv"),
        "sw" => t!("languages.sw"),
        "ta" => t!("languages.ta"),
        "te" => t!("languages.te"),
        "tg" => t!("languages.tg"),
        "th" => t!("languages.th"),
        "tk" => t!("languages.tk"),
        "tl" => t!("languages.tl"),
        "tr" => t!("languages.tr"),
        "tt" => t!("languages.tt"),
        "uk" => t!("languages.uk"),
        "ur" => t!("languages.ur"),
        "uz" => t!("languages.uz"),
        "vi" => t!("languages.vi"),
        "yi" => t!("languages.yi"),
        "yo" => t!("languages.yo"),
        "yue" => t!("languages.yue"),
        "zh" => t!("languages.zh"),
        "zu" => t!("languages.zu"),
        _ => code.to_string(),
    }
}

/// True for [`AUTO`] and for a code Sayso knows.
pub fn is_valid(code: &str) -> bool {
    code == AUTO || name(code).is_some()
}

/// Every known code, sorted by English name. For a model that takes any language.
pub fn all() -> &'static [&'static str] {
    static ALL: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
        let mut list: Vec<(&str, &str)> = NAMES.to_vec();
        list.sort_by_key(|(_, n)| *n);
        list.into_iter().map(|(c, _)| c).collect()
    });
    &ALL
}

/// The dictation language for the languages of the system, in the order of
/// preference: the first one Sayso knows, else English. A tag such as
/// "zh-Hans-CN" or "pt_BR" gives its language ("zh", "pt").
pub fn from_system(system: &[String]) -> String {
    system
        .iter()
        .filter_map(|tag| {
            let primary = tag.split(['-', '_', '.', '@']).next()?.to_ascii_lowercase();
            // Old and other codes for languages Sayso has under one code.
            let code = match primary.as_str() {
                "nb" => "no",
                "fil" => "tl",
                "iw" => "he",
                "in" => "id",
                "jv" => "jw",
                other => other,
            };
            name(code).map(|_| code.to_string())
        })
        .next()
        .unwrap_or_else(|| "en".into())
}

pub fn english() -> &'static [&'static str] {
    &["en"]
}

/// The 99 languages of OpenAI Whisper.
pub fn whisper() -> &'static [&'static str] {
    &[
        "en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar", "sv", "it", "id", "hi", "fi", "vi",
        "he", "uk", "el", "ms", "cs", "ro", "da", "hu", "ta", "no", "th", "ur", "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk",
        "te", "fa", "lv", "bn", "sr", "az", "sl", "kn", "et", "mk", "br", "eu", "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw",
        "gl", "mr", "pa", "si", "km", "sn", "yo", "so", "af", "oc", "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo",
        "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "haw", "ln", "ha", "ba", "jw", "su",
    ]
}

/// The 25 European languages of Parakeet TDT v3 and the models trained from it.
pub fn parakeet_v3() -> &'static [&'static str] {
    &[
        "en", "de", "es", "fr", "it", "pt", "nl", "pl", "ru", "uk", "sv", "da", "fi", "cs", "sk", "sl", "hr", "bg", "ro", "hu",
        "el", "et", "lv", "lt", "mt",
    ]
}

/// The 14 languages of Cohere Transcribe.
pub fn cohere() -> &'static [&'static str] {
    &["en", "fr", "de", "es", "it", "pt", "nl", "pl", "el", "ar", "ja", "zh", "ko", "vi"]
}

/// The languages SenseVoice Small can be told to use. It also detects more.
pub fn sensevoice() -> &'static [&'static str] {
    &["zh", "yue", "en", "ja", "ko"]
}

/// The languages Nemotron Multilingual has a language prompt for.
pub fn nemotron() -> &'static [&'static str] {
    &[
        "en", "es", "zh", "hi", "ar", "fr", "de", "ja", "ru", "pt", "ko", "it", "nl", "pl", "tr", "uk", "ro", "el", "cs", "hu",
        "sv", "da", "fi", "no", "nn", "sk", "hr", "bg", "lt", "et", "lv", "sl", "th", "vi", "id", "ms", "bn", "ur", "fa", "ta",
        "te", "mr", "gu", "kn", "ml", "si", "ne", "km", "sw", "am", "ha", "zu", "yo", "ig", "af", "rw", "so", "ln", "he", "ku",
        "az", "ka", "hy", "uz", "tg", "ky", "mi", "haw", "mt",
    ]
}

/// The languages of Apple Speech (SpeechTranscriber) on macOS 26.
pub fn apple_speech() -> &'static [&'static str] {
    &[
        "en", "zh", "yue", "ja", "ko", "de", "fr", "es", "it", "pt", "hi", "bn", "gu", "kn", "ks", "mai", "ml", "mr", "ne", "or",
        "pa", "ta", "te", "ur",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dictation_language_is_the_first_system_language_sayso_knows() {
        let tags = |list: &[&str]| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(from_system(&tags(&["zh-Hans-CN", "en-US"])), "zh");
        assert_eq!(from_system(&tags(&["pt_BR.UTF-8"])), "pt");
        assert_eq!(from_system(&tags(&["yue-Hant-HK"])), "yue");
        assert_eq!(from_system(&tags(&["nb-NO"])), "no");
        assert_eq!(from_system(&tags(&["xx-YY", "de-AT"])), "de");
        assert_eq!(from_system(&tags(&["C"])), "en");
        assert_eq!(from_system(&[]), "en");
    }

    #[test]
    fn every_model_language_has_a_name() {
        for list in [english(), whisper(), parakeet_v3(), cohere(), sensevoice(), nemotron(), apple_speech()] {
            for code in list {
                assert!(name(code).is_some(), "no name for {code}");
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

    #[test]
    fn every_code_has_a_name_in_the_catalog() {
        for (code, name) in NAMES {
            assert_eq!(display(code), *name, "the catalog name of {code}");
        }
    }
}
