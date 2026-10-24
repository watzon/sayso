//! The text of the interface in each language. See `docs/i18n.md`.
//!
//! Each locale has one catalog, `locales/<code>.yml`, compiled into the
//! binary. Code gets its text with [`t!`]:
//!
//! ```
//! use sayso_i18n::t;
//! let title = t!("settings.general.title");
//! let greeting = t!("home.greeting", name = "Ada");
//! let count = t!("history.dictations", count = 3);
//! ```
//!
//! A text with `count` has one key for each plural category of the locale
//! (`history.dictations.one`, `history.dictations.other`). `count` must be the
//! first argument.

pub mod dates;
pub mod numbers;
mod plural;
mod pseudo;
mod resolve;

pub use resolve::{resolve, system_locales};

use std::fmt::Display;

rust_i18n::i18n!("locales", fallback = "en");

/// A locale that the Language menu shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Locale {
    pub code: &'static str,
    /// The name of the language in that language: "Español", "日本語".
    pub name: &'static str,
    /// A first translation that no native speaker has reviewed yet. Only a
    /// debug build offers it (see [`available`]).
    pub draft: bool,
}

/// The locales with a catalog folder. The catalog tests check that each one
/// has every key of English. Set `draft` to false when a native speaker has
/// reviewed the catalog.
pub const LOCALES: &[Locale] = &[
    Locale { code: "en", name: "English", draft: false },
    Locale { code: "es", name: "Español", draft: false },
    Locale { code: "pt-BR", name: "Português (Brasil)", draft: false },
    Locale { code: "zh-CN", name: "简体中文", draft: false },
    Locale { code: "ja", name: "日本語", draft: false },
    Locale { code: "de", name: "Deutsch", draft: false },
    Locale { code: "fr", name: "Français", draft: false },
    Locale { code: "it", name: "Italiano", draft: false },
    Locale { code: "ko", name: "한국어", draft: false },
    Locale { code: "ru", name: "Русский", draft: false },
];

/// The locales that the Language menu offers and that "system" can select:
/// all in a debug build, the reviewed ones in a release.
pub fn available() -> impl Iterator<Item = &'static Locale> {
    LOCALES.iter().filter(|l| !l.draft || cfg!(debug_assertions))
}

/// The value of `general.language` that follows the system.
pub const SYSTEM: &str = "system";

/// The name of a locale's language in the interface language: "Spanish"
/// in English, "スペイン語" in Japanese. The Language menu shows it next to
/// the name in the language itself.
pub fn language_name(code: &str) -> String {
    match code {
        "en" => t!("language.en"),
        "es" => t!("language.es"),
        "pt-BR" => t!("language.pt-BR"),
        "zh-CN" => t!("language.zh-CN"),
        "ja" => t!("language.ja"),
        "de" => t!("language.de"),
        "fr" => t!("language.fr"),
        "it" => t!("language.it"),
        "ko" => t!("language.ko"),
        "ru" => t!("language.ru"),
        _ => code.to_string(),
    }
}

/// The pseudo-locale for development. It shows the English text with accents
/// and in brackets, so text that is not in a catalog stands out.
pub const PSEUDO: &str = "en-XA";

/// True when this build offers the pseudo-locale: debug builds only. A
/// release treats `en-XA` as an unknown setting.
pub const HAS_PSEUDO: bool = cfg!(debug_assertions);

/// The locale of a code, if it is in [`LOCALES`] (also a draft).
pub fn find(code: &str) -> Option<&'static Locale> {
    LOCALES.iter().find(|l| l.code.eq_ignore_ascii_case(code))
}

/// True for a value that `general.language` can have.
pub fn is_valid_setting(value: &str) -> bool {
    value == SYSTEM || (value == PSEUDO && HAS_PSEUDO) || find(value).is_some()
}

/// Use this locale for all text from now on, also for the text of the kit
/// widgets (they share the locale of rust-i18n).
pub fn set_locale(code: &str) {
    rust_i18n::set_locale(code);
}

/// The current locale.
pub fn locale() -> String {
    rust_i18n::locale().to_string()
}

/// Get the text of a key in the current locale. See the crate docs.
#[macro_export]
macro_rules! t {
    ($key:expr, count = $n:expr $(, $name:ident = $value:expr)* $(,)?) => {{
        let n = $n;
        $crate::__plural($key, n as u64, &[("count", &n as &dyn ::std::fmt::Display) $(, (stringify!($name), &$value as &dyn ::std::fmt::Display))*])
    }};
    ($key:expr $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::__translate($key, &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*])
    };
}

/// The text of a key that is known only at run time, in the current locale
/// with the fallback to English. None when no catalog has the key. Use [`t!`]
/// when the key is a literal: the catalog tests check those keys.
pub fn translate(key: &str) -> Option<String> {
    lookup(&rust_i18n::locale(), key)
}

#[doc(hidden)]
pub fn __translate(key: &str, args: &[(&str, &dyn Display)]) -> String {
    let locale = rust_i18n::locale();
    let text = lookup(&locale, key).unwrap_or_else(|| key.to_string());
    interpolate(&text, args)
}

#[doc(hidden)]
pub fn __plural(key: &str, n: u64, args: &[(&str, &dyn Display)]) -> String {
    let locale = rust_i18n::locale();
    let category = plural::category(&locale, n);
    let text = lookup(&locale, &format!("{key}.{category}"))
        .or_else(|| lookup(&locale, &format!("{key}.other")))
        .unwrap_or_else(|| key.to_string());
    interpolate(&text, args)
}

/// The text of a key in a locale, with the fallback to English.
fn lookup(locale: &str, key: &str) -> Option<String> {
    if locale == PSEUDO {
        return _rust_i18n_try_translate("en", key).map(|text| pseudo::transform(&text));
    }
    _rust_i18n_try_translate(locale, key).map(|text| text.into_owned())
}

/// Put each argument in the place of its `%{name}`.
fn interpolate(text: &str, args: &[(&str, &dyn Display)]) -> String {
    if args.is_empty() || !text.contains("%{") {
        return text.to_string();
    }
    let mut out = text.to_string();
    for (name, value) in args {
        out = out.replace(&format!("%{{{name}}}"), &value.to_string());
    }
    out
}

/// The plural categories that the counts of a locale need, for the tests.
#[doc(hidden)]
pub fn __plural_categories(locale: &str) -> &'static [&'static str] {
    plural::categories(locale)
}

/// All keys and texts of a catalog, for the tests. None for a locale without
/// a catalog.
#[doc(hidden)]
pub fn __messages(locale: &str) -> Option<Vec<(String, String)>> {
    _rust_i18n_backend()
        .messages_for_locale(locale)
        .map(|m| m.into_iter().filter(|(k, _)| k != "_version").map(|(k, v)| (k.into_owned(), v.into_owned())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_named_arguments() {
        let name = "Ada";
        assert_eq!(interpolate("Hi, %{name}. %{name}!", &[("name", &name)]), "Hi, Ada. Ada!");
        assert_eq!(interpolate("No arguments", &[]), "No arguments");
        assert_eq!(interpolate("%{other} stays", &[("name", &name)]), "%{other} stays");
    }

    #[test]
    fn every_setting_in_locales_is_valid() {
        assert!(is_valid_setting(SYSTEM));
        assert_eq!(is_valid_setting(PSEUDO), HAS_PSEUDO);
        assert!(is_valid_setting("en"));
        assert!(!is_valid_setting("xx"));
    }
}
