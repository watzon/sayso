//! The builds of a native program for the processor of this computer.
//!
//! ggml (llama.cpp in the cleanup runtime, whisper.cpp in the Windows speech
//! engine) compiles its CPU kernels for one instruction set, and a processor
//! without that set stops the program with an illegal instruction at the first
//! use (issue #37). So on x86_64 the packages hold a program in up to three
//! builds, and the app starts the best one that this processor runs:
//!
//! - `<program>`: AVX2, FMA, F16C, and BMI2
//! - `<program>-avx`: AVX and SSE 4.2
//! - `<program>-baseline`: x86-64 only
//!
//! A package with one build has `<program>` alone, and gets it.
//! `scripts/build-cleanup.sh` and `scripts/bundle-windows.ps1` make the builds.

use std::path::{Path, PathBuf};
use std::process::ExitStatus;

/// One build of a program, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// AVX2, FMA, F16C, and BMI2 (Intel Haswell and AMD Excavator, 2013 and
    /// later). The build that every package has, under the program's own
    /// name. On a system other than x86_64, the one build.
    Avx2,
    /// AVX and SSE 4.2 (Intel Sandy Bridge and AMD Bulldozer, 2011 and later).
    Avx,
    /// No instruction set past x86_64 itself: slow, but it runs everywhere.
    Baseline,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Avx2, Level::Avx, Level::Baseline];

    /// What the build adds to the name of the program.
    pub fn suffix(self) -> &'static str {
        match self {
            Level::Avx2 => "",
            Level::Avx => "-avx",
            Level::Baseline => "-baseline",
        }
    }

    /// The file name of the build of `program` (`sayso-cleanup`), with `.exe`
    /// on Windows.
    pub fn file_name(self, program: &str) -> String {
        let exe = if cfg!(windows) { ".exe" } else { "" };
        format!("{program}{}{exe}", self.suffix())
    }

    /// The build of `program` that a file is, by its name.
    pub fn of_file(program: &str, path: &Path) -> Option<Level> {
        let stem = path.file_stem()?.to_str()?;
        let suffix = stem.strip_prefix(program)?;
        Level::ALL.into_iter().find(|level| level.suffix() == suffix)
    }

    /// The instructions that the build needs, for a message.
    pub fn needs(self) -> &'static str {
        match self {
            Level::Avx2 => "AVX2, FMA, F16C, and BMI2",
            Level::Avx => "AVX and SSE 4.2",
            Level::Baseline => "x86-64",
        }
    }

    /// True when this processor runs the build. On a system other than
    /// x86_64 only [`Level::Avx2`], the one build there, is true.
    pub fn runs_here(self) -> bool {
        #[cfg(target_arch = "x86_64")]
        {
            match self {
                Level::Avx2 => {
                    Level::Avx.runs_here()
                        && std::arch::is_x86_feature_detected!("avx2")
                        && std::arch::is_x86_feature_detected!("fma")
                        && std::arch::is_x86_feature_detected!("f16c")
                        && std::arch::is_x86_feature_detected!("bmi2")
                }
                Level::Avx => std::arch::is_x86_feature_detected!("avx") && std::arch::is_x86_feature_detected!("sse4.2"),
                Level::Baseline => true,
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            self == Level::Avx2
        }
    }

    /// The builds that this processor runs, best first.
    pub fn for_this_cpu() -> impl Iterator<Item = Level> {
        Level::ALL.into_iter().filter(|level| level.runs_here())
    }
}

/// The best build of `program` in `dir` that this processor runs, else the
/// build under the program's own name when it is there (a package with one
/// build), else `None`.
pub fn best_in(dir: &Path, program: &str) -> Option<PathBuf> {
    Level::for_this_cpu()
        .map(|level| dir.join(level.file_name(program)))
        .find(|path| path.exists())
        .or_else(|| Some(dir.join(Level::Avx2.file_name(program))).filter(|path| path.exists()))
}

/// True when a process ended on an instruction that the processor does not
/// have: SIGILL on Unix, STATUS_ILLEGAL_INSTRUCTION on Windows.
pub fn illegal_instruction(status: &ExitStatus) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        status.signal() == Some(4)
    }
    #[cfg(windows)]
    {
        status.code() == Some(0xC000_001D_u32 as i32)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = status;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_name_gives_its_build() {
        for level in Level::ALL {
            let path = Path::new("/usr/lib/sayso").join(level.file_name("sayso-cleanup"));
            assert_eq!(Level::of_file("sayso-cleanup", &path), Some(level));
            assert_eq!(Level::of_file("sayso-engine", &path), None);
        }
        assert_eq!(Level::of_file("sayso-cleanup", Path::new("/opt/sayso-cleanup-native")), None);
    }

    #[test]
    fn a_build_that_runs_here_lets_the_ones_below_it_run() {
        let runs: Vec<bool> = Level::ALL.into_iter().map(Level::runs_here).collect();
        #[cfg(target_arch = "x86_64")]
        {
            assert!(runs[2], "the baseline build runs on every x86_64 processor");
            assert!(!runs[0] || runs[1], "AVX2 without AVX");
        }
        #[cfg(not(target_arch = "x86_64"))]
        assert_eq!(runs, [true, false, false]);
        assert_eq!(Level::for_this_cpu().next().map(Level::runs_here), Some(true));
    }

    #[test]
    fn the_best_build_in_a_folder_is_one_that_runs_here() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(best_in(dir.path(), "prog"), None);
        // A package with one build gets it, whatever the processor.
        std::fs::write(dir.path().join(Level::Avx2.file_name("prog")), b"").unwrap();
        assert_eq!(best_in(dir.path(), "prog"), Some(dir.path().join(Level::Avx2.file_name("prog"))));
        for level in [Level::Avx, Level::Baseline] {
            std::fs::write(dir.path().join(level.file_name("prog")), b"").unwrap();
        }
        let best = best_in(dir.path(), "prog").unwrap();
        assert_eq!(Some(Level::of_file("prog", &best).unwrap()), Level::for_this_cpu().next());
    }
}
