//! Inks and the paper palette. The types live in the MIT crate `sayso-ink`, so
//! that `sayso-ui` can use them without a GPL dependency. This module re-exports
//! them and adds the one thing that needs the Sayso catalogs: the translated
//! name of a named ink.

pub use sayso_ink::*;

/// The translated name of a [`NamedInk`]. Import the trait to call `label`.
pub trait NamedInkLabel {
    /// The name to show, in the interface language. [`NamedInk::name`] is the
    /// English name.
    fn label(self) -> String;
}

impl NamedInkLabel for NamedInk {
    fn label(self) -> String {
        match self {
            NamedInk::Sumi => sayso_i18n::t!("core.ink.sumi"),
            NamedInk::Indigo => sayso_i18n::t!("core.ink.indigo"),
            NamedInk::IronGall => sayso_i18n::t!("core.ink.iron_gall"),
            NamedInk::Sepia => sayso_i18n::t!("core.ink.sepia"),
            NamedInk::Verdigris => sayso_i18n::t!("core.ink.verdigris"),
            NamedInk::Oxblood => sayso_i18n::t!("core.ink.oxblood"),
        }
    }
}
