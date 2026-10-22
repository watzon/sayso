//! "Import from Pindrop": copy the user's Pindrop dictations, dictionary, and
//! prompt presets into Sayso. The reader is in `sayso_store::pindrop`. Pindrop
//! and its data stay as they are.

use crate::model::AppModel;
use gpui_kit::*;
use sayso_i18n::t;
use sayso_core::paths::{PathEnv, SystemEnv};
use sayso_core::stats::format_count;
use sayso_core::style::{Style, StyleLibrary};
use sayso_store::pindrop::{self, PindropPreset};
use std::path::PathBuf;

/// What an import added. Items that were in Sayso already are not counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PindropSummary {
    pub dictations: usize,
    pub words: usize,
    pub replacements: usize,
    pub styles: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PindropImport {
    #[default]
    Idle,
    Running,
    Done(PindropSummary),
    Failed(String),
}

impl PindropImport {
    /// The line under "Import from Pindrop" in Settings and in onboarding.
    pub fn message(&self) -> String {
        match self {
            PindropImport::Idle => t!("app.pindrop.idle"),
            PindropImport::Running => t!("app.pindrop.running"),
            PindropImport::Done(s) => {
                // One text for each kind of item, so that each language plurals its own nouns.
                let parts: Vec<String> = [
                    (s.dictations, t!("app.pindrop.dictations", count = s.dictations, number = format_count(s.dictations))),
                    (s.words, t!("app.pindrop.words", count = s.words, number = format_count(s.words))),
                    (s.replacements, t!("app.pindrop.replacements", count = s.replacements, number = format_count(s.replacements))),
                    (s.styles, t!("app.pindrop.styles", count = s.styles, number = format_count(s.styles))),
                ]
                .into_iter()
                .filter(|(n, _)| *n > 0)
                .map(|(_, text)| text)
                .collect();
                match parts.as_slice() {
                    [] => t!("app.pindrop.nothing_new"),
                    [one] => t!("app.pindrop.added", items = one),
                    [first @ .., last] => t!("app.pindrop.added", items = t!("app.pindrop.list_end", first = first.join(&t!("app.pindrop.list_separator")), last = last)),
                }
            }
            PindropImport::Failed(reason) => t!("app.pindrop.failed", reason = reason),
        }
    }
}

/// The Pindrop data folder. `SAYSO_PINDROP_DIR` selects another folder, for tests.
pub fn pindrop_dir() -> PathBuf {
    match SystemEnv.var("SAYSO_PINDROP_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => SystemEnv.home().join("Library/Application Support/Pindrop"),
    }
}

/// The style for a preset. The id starts with "pindrop-", so a second import
/// finds the style and does not add it again.
fn preset_style(preset: &PindropPreset) -> Style {
    let mut slug = String::new();
    for ch in preset.name.to_lowercase().chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            slug.push(ch);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    Style {
        id: if slug.is_empty() { "pindrop-preset".into() } else { format!("pindrop-{slug}") },
        name: preset.name.trim().to_string(),
        ink: "#1D1B18".into(),
        description: t!("app.pindrop.style_description"),
        prompt: preset.prompt.clone(),
        provider: None,
        model: None,
        temperature: None,
        timeout_ms: None,
        // A preset was written as a complete prompt.
        standalone: true,
        cleanup: Default::default(),
        inherited_cleanup: None,
    }
}

impl AppModel {
    /// Read the Pindrop store and add its content, off the UI thread.
    pub fn import_pindrop(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.services.store.clone() else { return };
        if self.pindrop_import == PindropImport::Running {
            return;
        }
        self.pindrop_import = PindropImport::Running;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let data = pindrop::read(&pindrop_dir())?;
                    let report = store.import_pindrop(&data)?;
                    Ok::<_, sayso_store::StoreError>((report, data.presets))
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                m.pindrop_import = match result {
                    Ok((report, presets)) => {
                        let styles_dir = m.paths.styles_dir();
                        let mut styles = 0;
                        for style in presets.iter().map(preset_style) {
                            if m.styles.get(&style.id).is_some() {
                                continue;
                            }
                            match sayso_core::style::save_style(&styles_dir, &style) {
                                Ok(_) => styles += 1,
                                Err(e) => log::warn!("import from Pindrop: could not save the style {}: {e}", style.id),
                            }
                        }
                        m.styles = StyleLibrary::load(&styles_dir);
                        m.reload_dictionary();
                        m.reload_history(cx);
                        log::info!("import from Pindrop: {report:?}, {styles} styles");
                        PindropImport::Done(PindropSummary { dictations: report.dictations, words: report.words, replacements: report.replacements, styles })
                    }
                    Err(e) => {
                        log::error!("import from Pindrop: {e}");
                        PindropImport::Failed(e.to_string())
                    }
                };
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::{PindropImport, PindropPreset, PindropSummary, preset_style};

    #[test]
    fn the_message_lists_only_what_was_added() {
        let done = |dictations, words, replacements, styles| PindropImport::Done(PindropSummary { dictations, words, replacements, styles }).message();
        assert_eq!(done(3182, 6, 7, 1), "Added 3,182 dictations, 6 words, 7 replacements, and 1 style.");
        assert_eq!(done(1, 0, 0, 0), "Added 1 dictation.");
        assert_eq!(done(0, 2, 1, 0), "Added 2 words, and 1 replacement.");
        assert_eq!(done(0, 0, 0, 0), "Everything from Pindrop is in Sayso already.");
    }

    #[test]
    fn a_preset_becomes_a_style_with_a_valid_id() {
        let style = preset_style(&PindropPreset { name: " Release Notes (v2)! ".into(), prompt: "Write notes.".into() });
        assert_eq!(style.id, "pindrop-release-notes-v2");
        assert_eq!(style.name, "Release Notes (v2)!");
        assert!(sayso_core::style::validate_id(&style.id).is_ok());
        let unnamed = preset_style(&PindropPreset { name: "日本語".into(), prompt: "p".into() });
        assert_eq!(unnamed.id, "pindrop-preset");
    }
}
