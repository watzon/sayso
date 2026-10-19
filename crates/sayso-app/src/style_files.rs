//! A cheap check for a change in the styles folder.
//!
//! Loading the styles reads and parses every file, so the model loads them
//! again only when the fingerprint of the folder changes.

use sayso_core::style::BASE_PROMPT_FILE;
use std::ffi::OsString;
use std::path::Path;
use std::time::SystemTime;

/// What the loader reads, as the file system describes it: the name, the
/// modified time, and the length of each style file and of the base prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    /// The style files (`*.toml`), sorted by name.
    styles: Vec<(OsString, Option<SystemTime>, u64)>,
    /// The base prompt file, when it exists.
    base: Option<(Option<SystemTime>, u64)>,
}

impl Fingerprint {
    /// Read the metadata of the folder. A missing folder gives an empty fingerprint.
    pub fn of(dir: &Path) -> Self {
        let mut styles: Vec<_> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                    .filter_map(|p| {
                        let meta = std::fs::metadata(&p).ok()?;
                        Some((p.file_name()?.to_owned(), meta.modified().ok(), meta.len()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        styles.sort();
        let base = std::fs::metadata(dir.join(BASE_PROMPT_FILE)).ok().map(|m| (m.modified().ok(), m.len()));
        Fingerprint { styles, base }
    }
}

#[cfg(test)]
mod tests {
    use super::Fingerprint;
    use sayso_core::style::BASE_PROMPT_FILE;

    #[test]
    fn the_fingerprint_changes_with_a_style_file_or_the_base_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let empty = Fingerprint::of(dir.path());
        assert_eq!(empty, Fingerprint::of(dir.path()));

        let style = dir.path().join("mine.toml");
        std::fs::write(&style, "id = \"mine\"\n").unwrap();
        let one = Fingerprint::of(dir.path());
        assert_ne!(one, empty);
        assert_eq!(one, Fingerprint::of(dir.path()));

        // A longer file is a change, even when the clock has not moved on.
        std::fs::write(&style, "id = \"mine\"\nname = \"Mine\"\n").unwrap();
        let longer = Fingerprint::of(dir.path());
        assert_ne!(longer, one);

        std::fs::write(dir.path().join(BASE_PROMPT_FILE), "- Keep it short.\n").unwrap();
        let with_base = Fingerprint::of(dir.path());
        assert_ne!(with_base, longer);

        std::fs::remove_file(&style).unwrap();
        assert_ne!(Fingerprint::of(dir.path()), with_base);
    }

    #[test]
    fn other_files_and_a_missing_folder_change_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let before = Fingerprint::of(dir.path());
        std::fs::write(dir.path().join("notes.txt"), "not a style").unwrap();
        assert_eq!(Fingerprint::of(dir.path()), before);
        assert_eq!(Fingerprint::of(&dir.path().join("missing")), before);
    }
}
