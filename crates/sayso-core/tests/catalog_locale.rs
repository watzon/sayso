//! The catalog is cached per locale. It has its own test binary because the
//! locale is global to the process, and other tests read it in parallel.

use sayso_core::models;

fn descriptions() -> Vec<String> {
    models::catalog().iter().map(|m| m.description.clone()).collect()
}

#[test]
fn the_catalog_follows_a_change_of_the_interface_language() {
    sayso_i18n::set_locale("en");
    let english = descriptions();
    assert!(
        english.iter().any(|d| d.contains("model")),
        "the English catalog has English text: {english:?}"
    );
    assert!(std::ptr::eq(models::catalog(), models::catalog()), "a second read uses the cache");

    sayso_i18n::set_locale("de");
    let german = descriptions();
    assert_eq!(german.len(), english.len());
    assert_ne!(german, english, "the catalog changes with the locale");
    assert!(german.iter().any(|d| d.contains("Modell")), "the German catalog has German text: {german:?}");
    assert_eq!(models::find(&models::catalog()[0].id).unwrap().description, german[0], "find reads the same catalog");

    sayso_i18n::set_locale("en");
    assert_eq!(descriptions(), english, "the English catalog comes back");
}
