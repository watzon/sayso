//! Installed extensions (`docs/extensions.md`). An extension is installed in
//! `<data dir>/extensions/<id>/<version>/` ([`crate::paths::Paths::extensions_dir`]).
//!
//! For now Sayso only needs to know if one is installed: the Models page then
//! shows the VAD and diarization models. The install, the manifest, and
//! `state.toml` come later (step 8 of "Order of work").

use std::path::Path;

/// True when `extensions_dir` has at least one folder: an extension is
/// installed. A missing or unreadable folder means no extension.
pub fn any_installed(extensions_dir: &Path) -> bool {
    std::fs::read_dir(extensions_dir)
        .map(|entries| entries.filter_map(Result::ok).any(|entry| entry.file_type().is_ok_and(|t| t.is_dir())))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_in_the_extensions_folder_is_an_installed_extension() {
        let dir = tempfile::tempdir().unwrap();
        let extensions = dir.path().join("extensions");
        assert!(!any_installed(&extensions), "no folder");
        std::fs::create_dir(&extensions).unwrap();
        assert!(!any_installed(&extensions), "an empty folder");
        // `state.toml` is a file, not an extension.
        std::fs::write(extensions.join("state.toml"), "").unwrap();
        assert!(!any_installed(&extensions), "only a file");
        std::fs::create_dir_all(extensions.join("test-extension/0.1.0")).unwrap();
        assert!(any_installed(&extensions));
    }
}
