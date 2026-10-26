//! The few texts of the components, and the interface locale.
//!
//! `sayso-ui` has no catalogs. It has English texts for the strings that its
//! components make (the screen reader names of the keycaps and of the stepper).
//! An app that wants other languages calls [`install`] once at startup with a
//! translator and a locale source. The components call them each time they
//! need a text, so a change of language shows at the next render.

use std::sync::RwLock;

/// Returns the text of a key in the current language, or None when the app
/// does not know the key. Then the English text of `sayso-ui` is used.
pub type Translate = fn(key: &str) -> Option<String>;

/// Returns the code of the current interface locale, for example `ja` or `pt-BR`.
pub type Locale = fn() -> String;

struct Hooks {
    translate: Option<Translate>,
    locale: Option<Locale>,
}

static HOOKS: RwLock<Hooks> = RwLock::new(Hooks { translate: None, locale: None });

/// Use the translator and the locale source of the app. Call it before
/// [`crate::init`]. A second call replaces the first.
pub fn install(translate: Translate, locale: Locale) {
    let mut hooks = HOOKS.write().unwrap_or_else(|e| e.into_inner());
    hooks.translate = Some(translate);
    hooks.locale = Some(locale);
}

/// The text of a key. The key is the text itself when `sayso-ui` has no
/// English text for it.
pub fn text(key: &str) -> String {
    let translate = HOOKS.read().unwrap_or_else(|e| e.into_inner()).translate;
    resolve(translate, key)
}

fn resolve(translate: Option<Translate>, key: &str) -> String {
    translate.and_then(|f| f(key)).or_else(|| english(key).map(str::to_string)).unwrap_or_else(|| key.to_string())
}

/// The code of the interface locale. `en` when no app installed a source.
pub fn locale() -> String {
    let locale = HOOKS.read().unwrap_or_else(|e| e.into_inner()).locale;
    locale.map(|f| f()).unwrap_or_else(|| "en".to_string())
}

/// The keys that `sayso-ui` asks for, with their built-in English texts. An
/// app that installs a translator must have these keys in its catalogs. The
/// app can test that with this list.
pub const TEXTS: &[(&str, &str)] = &[
    ("a11y.decrease", "Decrease"),
    ("a11y.increase", "Increase"),
    ("a11y.key.control", "Control"),
    ("a11y.key.option", "Option"),
    ("a11y.key.shift", "Shift"),
    ("a11y.key.command", "Command"),
    ("a11y.key.return", "Return"),
    ("a11y.key.tab", "Tab"),
    ("a11y.key.delete", "Delete"),
];

fn english(key: &str) -> Option<&'static str> {
    TEXTS.iter().find(|(k, _)| *k == key).map(|(_, text)| *text)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The hooks are global, so the test calls `resolve` and does not install one.
    #[test]
    fn english_is_the_default_and_a_translator_wins() {
        let spanish: Translate = |key| (key == "a11y.key.option").then(|| "Opción".to_string());
        assert_eq!(resolve(None, "a11y.key.option"), "Option");
        assert_eq!(resolve(None, "no.such.key"), "no.such.key");
        assert_eq!(resolve(Some(spanish), "a11y.key.option"), "Opción");
        assert_eq!(resolve(Some(spanish), "a11y.key.shift"), "Shift", "an unknown key falls back to English");
    }

    #[test]
    fn the_locale_is_english_without_a_source() {
        assert_eq!(locale(), "en");
    }
}
