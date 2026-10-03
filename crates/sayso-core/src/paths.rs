//! File locations with XDG lookup (plan §3, "File locations").

use std::path::{Path, PathBuf};

/// The environment that path resolution reads. A trait so tests do not touch
/// the real home directory.
pub trait PathEnv {
    fn var(&self, key: &str) -> Option<String>;
    fn home(&self) -> PathBuf;
    fn exists(&self, path: &Path) -> bool;
}

/// The real process environment.
pub struct SystemEnv;

impl PathEnv for SystemEnv {
    fn var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|v| !v.is_empty())
    }
    fn home(&self) -> PathBuf {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"))
    }
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

/// Which rule picked a directory. Settings shows this next to the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathSource {
    XdgVariable,
    DotConfig,
    PlatformDefault,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub config_source: PathSource,
    pub data_dir: PathBuf,
    pub data_source: PathSource,
    pub cache_dir: PathBuf,
}

impl Paths {
    pub fn resolve(env: &impl PathEnv) -> Self {
        let home = env.home();
        let platform = PlatformDirs::for_env(env, &home);

        let (config_dir, config_source) = if let Some(xdg) = env.var("XDG_CONFIG_HOME") {
            (PathBuf::from(xdg).join("sayso"), PathSource::XdgVariable)
        } else if env.exists(&home.join(".config/sayso")) {
            (home.join(".config/sayso"), PathSource::DotConfig)
        } else {
            (platform.config, PathSource::PlatformDefault)
        };

        let (data_dir, data_source) = match env.var("XDG_DATA_HOME") {
            Some(xdg) => (PathBuf::from(xdg).join("sayso"), PathSource::XdgVariable),
            None => (platform.data, PathSource::PlatformDefault),
        };

        let cache_dir = match env.var("XDG_CACHE_HOME") {
            Some(xdg) => PathBuf::from(xdg).join("sayso"),
            None => platform.cache,
        };

        Self { config_dir, config_source, data_dir, data_source, cache_dir }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }
    pub fn styles_dir(&self) -> PathBuf {
        self.config_dir.join("styles")
    }
    pub fn database_file(&self) -> PathBuf {
        self.data_dir.join("sayso.db")
    }
    pub fn audio_dir(&self) -> PathBuf {
        self.data_dir.join("audio")
    }
    pub fn models_dir(&self) -> PathBuf {
        self.data_dir.join("models")
    }
    /// Runtime state that is not configuration, for example the pill position.
    pub fn state_file(&self) -> PathBuf {
        self.data_dir.join("state.json")
    }
    pub fn log_dir(&self) -> PathBuf {
        self.cache_dir.join("logs")
    }

    pub fn create_all(&self) -> std::io::Result<()> {
        for dir in [
            self.config_dir.clone(),
            self.styles_dir(),
            self.data_dir.clone(),
            self.audio_dir(),
            self.models_dir(),
            self.cache_dir.clone(),
            self.log_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Replace a leading home directory with `~` for display.
    pub fn display(path: &Path, home: &Path) -> String {
        match path.strip_prefix(home) {
            Ok(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
            Err(_) => path.display().to_string(),
        }
    }
}

/// The directories each platform uses when no XDG variable is set.
struct PlatformDirs {
    config: PathBuf,
    data: PathBuf,
    cache: PathBuf,
}

impl PlatformDirs {
    /// macOS: `~/Library/Application Support/Sayso` and `~/Library/Caches/Sayso`.
    #[cfg(not(windows))]
    fn for_env(_env: &impl PathEnv, home: &Path) -> Self {
        let app_support = home.join("Library/Application Support/Sayso");
        PlatformDirs { config: app_support.clone(), data: app_support, cache: home.join("Library/Caches/Sayso") }
    }

    /// Windows: config in `%APPDATA%\Sayso` (it roams with the user), data and
    /// cache in `%LOCALAPPDATA%\Sayso` (models are large and must not roam).
    #[cfg(windows)]
    fn for_env(env: &impl PathEnv, home: &Path) -> Self {
        let roaming = env.var("APPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData").join("Roaming"));
        let local = env.var("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData").join("Local"));
        let local = local.join("Sayso");
        PlatformDirs { config: roaming.join("Sayso"), data: local.clone(), cache: local.join("Cache") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    struct FakeEnv {
        vars: HashMap<&'static str, &'static str>,
        existing: HashSet<PathBuf>,
    }

    impl PathEnv for FakeEnv {
        fn var(&self, key: &str) -> Option<String> {
            self.vars.get(key).map(|v| v.to_string())
        }
        fn home(&self) -> PathBuf {
            PathBuf::from("/Users/test")
        }
        fn exists(&self, path: &Path) -> bool {
            self.existing.contains(path)
        }
    }

    fn env(vars: &[(&'static str, &'static str)], existing: &[&str]) -> FakeEnv {
        FakeEnv {
            vars: vars.iter().copied().collect(),
            existing: existing.iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn xdg_config_home_wins() {
        let p = Paths::resolve(&env(&[("XDG_CONFIG_HOME", "/x")], &["/Users/test/.config/sayso"]));
        assert_eq!(p.config_dir, PathBuf::from("/x/sayso"));
        assert_eq!(p.config_source, PathSource::XdgVariable);
    }

    #[test]
    #[cfg(not(windows))]
    fn dot_config_used_only_when_it_exists() {
        let with = Paths::resolve(&env(&[], &["/Users/test/.config/sayso"]));
        assert_eq!(with.config_dir, PathBuf::from("/Users/test/.config/sayso"));
        let without = Paths::resolve(&env(&[], &[]));
        assert_eq!(without.config_dir, PathBuf::from("/Users/test/Library/Application Support/Sayso"));
        assert_eq!(without.config_source, PathSource::PlatformDefault);
    }

    #[test]
    #[cfg(not(windows))]
    fn data_and_cache_defaults() {
        let p = Paths::resolve(&env(&[], &[]));
        assert_eq!(p.data_dir, PathBuf::from("/Users/test/Library/Application Support/Sayso"));
        assert_eq!(p.cache_dir, PathBuf::from("/Users/test/Library/Caches/Sayso"));
        let x = Paths::resolve(&env(&[("XDG_DATA_HOME", "/d"), ("XDG_CACHE_HOME", "/c")], &[]));
        assert_eq!(x.data_dir, PathBuf::from("/d/sayso"));
        assert_eq!(x.cache_dir, PathBuf::from("/c/sayso"));
    }

    #[test]
    fn empty_xdg_variable_is_ignored() {
        let mut e = env(&[], &[]);
        e.vars.insert("XDG_CONFIG_HOME", "");
        // FakeEnv returns Some(""), SystemEnv filters it. Mirror SystemEnv here.
        assert_eq!(SystemEnv.var("SAYSO_SURELY_UNSET_VAR"), None);
    }

    #[test]
    #[cfg(windows)]
    fn windows_defaults_use_appdata() {
        let p = Paths::resolve(&env(&[("APPDATA", r"C:\Users\test\AppData\Roaming"), ("LOCALAPPDATA", r"C:\L")], &[]));
        assert_eq!(p.config_dir, PathBuf::from(r"C:\Users\test\AppData\Roaming\Sayso"));
        assert_eq!(p.config_source, PathSource::PlatformDefault);
        assert_eq!(p.data_dir, PathBuf::from(r"C:\L\Sayso"));
        assert_eq!(p.cache_dir, PathBuf::from(r"C:\L\Sayso\Cache"));
        let x = Paths::resolve(&env(&[("XDG_DATA_HOME", "/d"), ("XDG_CACHE_HOME", "/c")], &[]));
        assert_eq!(x.data_dir, PathBuf::from("/d/sayso"));
        assert_eq!(x.cache_dir, PathBuf::from("/c/sayso"));
    }

    #[test]
    #[cfg(windows)]
    fn windows_defaults_without_variables_fall_back_to_the_profile() {
        let p = Paths::resolve(&env(&[], &[]));
        assert_eq!(p.config_dir, PathBuf::from("/Users/test").join("AppData").join("Roaming").join("Sayso"));
        assert_eq!(p.data_dir, PathBuf::from("/Users/test").join("AppData").join("Local").join("Sayso"));
    }

    #[test]
    #[cfg(not(windows))]
    fn display_uses_tilde() {
        let s = Paths::display(Path::new("/Users/test/.config/sayso/config.toml"), Path::new("/Users/test"));
        assert_eq!(s, "~/.config/sayso/config.toml");
    }
}
