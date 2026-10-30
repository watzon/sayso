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

/// A build of the engine: its name, and whether this processor runs it.
type Build = (&'static str, fn() -> bool);

/// The builds of the engine in a release file for Windows, best first, each
/// with the instructions that it needs. whisper.cpp is compiled for one
/// instruction set, and a processor without it stops the engine, so the
/// Windows file holds `sayso-engine.exe` (AVX2, FMA, F16C, BMI2) with
/// `sayso-engine-avx.exe` (AVX, SSE 4.2) and `sayso-engine-baseline.exe`
/// (x86-64 only). The other files have one build. The app picks the same way
/// (`sayso_core::cpu` in the Sayso repository; this crate keeps its own copy,
/// because it depends on no Sayso crate but the protocol).
const BUILDS: [Build; 3] = [("sayso-engine", runs_avx2), ("sayso-engine-avx", runs_avx), ("sayso-engine-baseline", || true)];

#[cfg(target_arch = "x86_64")]
fn runs_avx() -> bool {
    std::arch::is_x86_feature_detected!("avx") && std::arch::is_x86_feature_detected!("sse4.2")
}

#[cfg(target_arch = "x86_64")]
fn runs_avx2() -> bool {
    runs_avx()
        && std::arch::is_x86_feature_detected!("avx2")
        && std::arch::is_x86_feature_detected!("fma")
        && std::arch::is_x86_feature_detected!("f16c")
        && std::arch::is_x86_feature_detected!("bmi2")
}

// Other processors have the one build, under the engine's own name.
#[cfg(not(target_arch = "x86_64"))]
fn runs_avx() -> bool {
    false
}

#[cfg(not(target_arch = "x86_64"))]
fn runs_avx2() -> bool {
    true
}

/// The best build of the engine in `dir` that this processor runs, else
/// [`ENGINE_FILE`].
fn best_build(dir: &Path) -> PathBuf {
    let exe = if cfg!(windows) { ".exe" } else { "" };
    BUILDS
        .iter()
        .filter(|(_, runs)| runs())
        .map(|(name, _)| dir.join(format!("{name}{exe}")))
        .find(|path| path.is_file())
        .unwrap_or_else(|| dir.join(ENGINE_FILE))
}

/// Find the engine program in `folder`, where you unpacked a release file
/// (`sayso-engine-<version>-<system>.tar.gz`, or the `.zip` on Windows).
///
/// `folder` can be the folder `sayso-engine-<version>` of the file, which has
/// the program `sayso-engine` (`sayso-engine.exe` on Windows), or the folder
/// that holds it. In the second case, when more than one version is there,
/// this function takes the version that sorts last by name.
///
/// On Windows the folder has a build of the engine for each instruction set
/// (see [`BUILDS`]); this function returns the best one that this processor
/// runs.
pub fn find_engine(folder: impl AsRef<Path>) -> io::Result<PathBuf> {
    let folder = folder.as_ref();
    if folder.join(ENGINE_FILE).is_file() {
        return Ok(best_build(folder));
    }
    let mut versions: Vec<PathBuf> = std::fs::read_dir(folder)
        .map_err(|e| io::Error::new(e.kind(), format!("cannot read {}: {e}", folder.display())))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(FOLDER_PREFIX) && entry.path().join(ENGINE_FILE).is_file())
        .map(|entry| entry.path())
        .collect();
    versions.sort();
    match versions.pop() {
        Some(version) => Ok(best_build(&version)),
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
    fn the_engine_is_the_best_build_that_this_processor_runs() {
        let dir = tempfile::tempdir().unwrap();
        let exe = if cfg!(windows) { ".exe" } else { "" };
        for (name, _) in BUILDS {
            touch(&dir.path().join(format!("{name}{exe}")));
        }
        let best = BUILDS.iter().find(|(_, runs)| runs()).unwrap().0;
        assert_eq!(find_engine(dir.path()).unwrap(), dir.path().join(format!("{best}{exe}")));
        // Without the build under the engine's own name, the folder is no release file.
        std::fs::remove_file(dir.path().join(ENGINE_FILE)).unwrap();
        assert!(find_engine(dir.path()).is_err());
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
