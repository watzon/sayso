//! The builds of the cleanup runtime for the processor of this computer.
//!
//! llama.cpp compiles its CPU kernels for one instruction set, and a processor
//! without that set stops the runtime with SIGILL at the first load (issue
//! #37). So on x86_64 the packages hold three builds, and the app starts the
//! best one that this processor runs. Other systems have one build.
//! `scripts/build-cleanup.sh` (and `scripts/bundle-windows.ps1`) make them.

use std::path::Path;

/// One build of the runtime, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Build {
    /// AVX2, FMA, F16C, and BMI2 (Intel Haswell and AMD Excavator, 2013 and
    /// later). The build that every system has, as `sayso-cleanup`.
    Avx2,
    /// AVX and SSE 4.2 (Intel Sandy Bridge and AMD Bulldozer, 2011 and later).
    Avx,
    /// No instruction set past x86_64 itself: slow, but it runs everywhere.
    Baseline,
}

impl Build {
    pub const ALL: [Build; 3] = [Build::Avx2, Build::Avx, Build::Baseline];

    /// The name of the file, without `.exe`.
    pub fn stem(self) -> &'static str {
        match self {
            Build::Avx2 => "sayso-cleanup",
            Build::Avx => "sayso-cleanup-avx",
            Build::Baseline => "sayso-cleanup-baseline",
        }
    }

    pub fn file_name(self) -> String {
        if cfg!(windows) { format!("{}.exe", self.stem()) } else { self.stem().to_string() }
    }

    /// The build that a file of the packages is, by its name.
    pub fn of_file(path: &Path) -> Option<Build> {
        let stem = path.file_stem()?.to_str()?;
        Build::ALL.into_iter().find(|b| b.stem() == stem)
    }

    /// The instructions that the build needs, for a message.
    pub fn needs(self) -> &'static str {
        match self {
            Build::Avx2 => "AVX2, FMA, F16C, and BMI2",
            Build::Avx => "AVX and SSE 4.2",
            Build::Baseline => "x86-64",
        }
    }

    /// True when this processor runs the build. On a system other than
    /// x86_64 only [`Build::Avx2`], the one build there, is true.
    pub fn runs_here(self) -> bool {
        #[cfg(target_arch = "x86_64")]
        {
            match self {
                Build::Avx2 => {
                    Build::Avx.runs_here()
                        && std::arch::is_x86_feature_detected!("avx2")
                        && std::arch::is_x86_feature_detected!("fma")
                        && std::arch::is_x86_feature_detected!("f16c")
                        && std::arch::is_x86_feature_detected!("bmi2")
                }
                Build::Avx => std::arch::is_x86_feature_detected!("avx") && std::arch::is_x86_feature_detected!("sse4.2"),
                Build::Baseline => true,
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            self == Build::Avx2
        }
    }

    /// The builds that this processor runs, best first.
    pub fn for_this_cpu() -> impl Iterator<Item = Build> {
        Build::ALL.into_iter().filter(|b| b.runs_here())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_name_gives_its_build() {
        for build in Build::ALL {
            assert_eq!(Build::of_file(Path::new(&format!("/usr/lib/sayso/{}", build.file_name()))), Some(build));
        }
        assert_eq!(Build::of_file(Path::new("/opt/sayso-cleanup-native")), None);
    }

    #[test]
    fn a_build_that_runs_here_lets_the_ones_below_it_run() {
        let runs: Vec<bool> = Build::ALL.into_iter().map(Build::runs_here).collect();
        #[cfg(target_arch = "x86_64")]
        {
            assert!(runs[2], "the baseline build runs on every x86_64 processor");
            assert!(!runs[0] || runs[1], "AVX2 without AVX");
        }
        #[cfg(not(target_arch = "x86_64"))]
        assert_eq!(runs, [true, false, false]);
        assert_eq!(Build::for_this_cpu().next().map(Build::runs_here), Some(true));
    }
}
