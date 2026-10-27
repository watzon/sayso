//! Find the engine in an unpacked release file.

use std::io;
use std::path::{Path, PathBuf};

/// The file name of the engine in a release file. The Swift engine on macOS
/// has this name in the file too (`scripts/package-engine.sh`).
#[cfg(windows)]
const ENGINE_FILE: &str = "sayso-engine.exe";
#[cfg(not(windows))]
const ENGINE_FILE: &str = "sayso-engine";

/// The folders in a release file are named `sayso-engine-<version>`.
const FOLDER_PREFIX: &str = "sayso-engine-";

/// Find the engine program in `folder`, where you unpacked a release file
/// (`sayso-engine-<version>-<system>.tar.gz`, or the `.zip` on Windows).
///
/// `folder` can be the folder `sayso-engine-<version>` of the file, which has
/// the program `sayso-engine` (`sayso-engine.exe` on Windows), or the folder
/// that holds it. In the second case, when more than one version is there,
/// this function takes the version that sorts last by name.
pub fn find_engine(folder: impl AsRef<Path>) -> io::Result<PathBuf> {
    let folder = folder.as_ref();
    let direct = folder.join(ENGINE_FILE);
    if direct.is_file() {
        return Ok(direct);
    }
    let mut versions: Vec<PathBuf> = std::fs::read_dir(folder)
        .map_err(|e| io::Error::new(e.kind(), format!("cannot read {}: {e}", folder.display())))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(FOLDER_PREFIX) && entry.path().join(ENGINE_FILE).is_file())
        .map(|entry| entry.path())
        .collect();
    versions.sort();
    match versions.pop() {
        Some(version) => Ok(version.join(ENGINE_FILE)),
        None => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("there is no {ENGINE_FILE} in {}, and no {FOLDER_PREFIX}<version> folder with one. Unpack a release file of the engine there.", folder.display()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
    }

    #[test]
    fn the_engine_is_in_the_folder_of_the_release_file() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("sayso-engine-0.11.0").join(ENGINE_FILE));
        let direct = dir.path().join("sayso-engine-0.11.0");
        assert_eq!(find_engine(&direct).unwrap(), direct.join(ENGINE_FILE));
    }

    #[test]
    fn the_engine_is_in_a_folder_below() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("sayso-engine-0.11.0").join(ENGINE_FILE));
        touch(&dir.path().join("sayso-engine-0.12.0").join(ENGINE_FILE));
        touch(&dir.path().join("sayso-engine-0.13.0").join("README.md"));
        touch(&dir.path().join("other").join(ENGINE_FILE));
        assert_eq!(find_engine(dir.path()).unwrap(), dir.path().join("sayso-engine-0.12.0").join(ENGINE_FILE), "the last version with a program");
    }

    #[test]
    fn a_folder_without_the_engine_is_an_error_that_says_what_to_do() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("other").join(ENGINE_FILE));
        let error = find_engine(dir.path()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(error.to_string().contains("Unpack a release file"), "{error}");
        assert_eq!(find_engine(dir.path().join("missing")).unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}
