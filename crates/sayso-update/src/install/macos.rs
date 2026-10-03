//! The macOS installer: a staged copy of `Sayso.app` beside the installed
//! bundle, and one atomic exchange of the two.

use super::{Installer, Manual};
use semver::Version;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The signature that a staged bundle must have: a Developer ID signature
/// (the two certificate fields) of the Sayso team, for the Sayso bundle id.
const REQUIREMENT: &str = "=anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"MB5789APU7\" and identifier \"dev.sayso.Sayso\"";

/// The variables that a start through `open` must get again. `open` gives
/// the app the environment of the login session, not of this process.
const PASS_ENV: &[&str] = &["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "SAYSO_UPDATE_URL", "SAYSO_UPDATE_KEY"];

pub struct MacInstall {
    /// The installed bundle, for example `/Applications/Sayso.app`.
    app: PathBuf,
}

impl MacInstall {
    pub fn detect() -> Result<Self, Manual> {
        let exe = std::env::current_exe().and_then(|p| p.canonicalize()).map_err(|_| Manual::WrongFolder)?;
        Self::for_executable(&exe)
    }

    /// `exe` is `<bundle>.app/Contents/MacOS/<name>`.
    fn for_executable(exe: &Path) -> Result<Self, Manual> {
        let app = exe.parent().and_then(Path::parent).and_then(Path::parent).ok_or(Manual::WrongFolder)?;
        if app.extension().is_none_or(|e| e != "app") || app.to_string_lossy().contains("/AppTranslocation/") {
            return Err(Manual::WrongFolder);
        }
        let parent = app.parent().ok_or(Manual::WrongFolder)?;
        let c_parent = CString::new(parent.as_os_str().as_bytes()).map_err(|_| Manual::WrongFolder)?;
        // SAFETY: `c_parent` is a valid C string, and `fs` is a plain struct that `statfs` fills.
        let read_only = unsafe {
            let mut fs: libc::statfs = std::mem::zeroed();
            libc::statfs(c_parent.as_ptr(), &mut fs) != 0 || fs.f_flags & libc::MNT_RDONLY as u32 != 0
        };
        if read_only {
            return Err(Manual::WrongFolder);
        }
        // SAFETY: `c_parent` is a valid C string.
        if unsafe { libc::access(c_parent.as_ptr(), libc::W_OK) } != 0 {
            return Err(Manual::NoPermission);
        }
        let install = Self { app: app.to_path_buf() };
        install.make_staging_dir().map_err(|_| Manual::NoPermission)?;
        let exchange = install.can_exchange();
        // Gone again when it is empty: no update is in progress.
        let _ = std::fs::remove_dir(install.staging_dir());
        if exchange { Ok(install) } else { Err(Manual::NoExchange) }
    }

    /// Beside the bundle and named after it, so two bundles in one folder
    /// do not share it: `.Sayso-update` for `Sayso.app`.
    fn staging_dir(&self) -> PathBuf {
        let stem = self.app.file_stem().unwrap_or_default().to_string_lossy();
        self.app.with_file_name(format!(".{stem}-update"))
    }

    fn staged_app(&self) -> PathBuf {
        self.staging_dir().join(self.app.file_name().unwrap_or_default())
    }

    /// Make the staging folder, for this user only. An entry that is there
    /// must be a real folder that this user owns.
    fn make_staging_dir(&self) -> std::io::Result<()> {
        let dir = self.staging_dir();
        match std::fs::symlink_metadata(&dir) {
            // SAFETY: `getuid` has no arguments and cannot fail.
            Ok(meta) if meta.is_dir() && meta.uid() == unsafe { libc::getuid() } => Ok(()),
            Ok(_) => Err(std::io::Error::other(format!("{} is not a folder of this user", dir.display()))),
            Err(_) => std::fs::DirBuilder::new().mode(0o700).create(&dir),
        }
    }

    /// Try the exchange on two empty folders.
    fn can_exchange(&self) -> bool {
        let (a, b) = (self.staging_dir().join(".probe-a"), self.staging_dir().join(".probe-b"));
        let made = std::fs::create_dir(&a).is_ok() && std::fs::create_dir(&b).is_ok();
        let ok = made && swap(&a, &b).is_ok();
        let _ = std::fs::remove_dir(&a);
        let _ = std::fs::remove_dir(&b);
        ok
    }

    fn copy_from_image(&self, image: &Path) -> Result<(), String> {
        let name = self.app.file_name().unwrap_or_default();
        let root = self.staging_dir().join(".mount");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).map_err(|e| format!("cannot make {}: {e}", root.display()))?;
        run(Command::new("/usr/bin/hdiutil").args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountroot"]).arg(&root).arg(image))?;
        let volume = std::fs::read_dir(&root).ok().and_then(|mut entries| entries.find_map(|e| e.ok().map(|e| e.path())));
        let copied = match &volume {
            Some(volume) if volume.join(name).is_dir() => run(Command::new("/usr/bin/ditto").arg(volume.join(name)).arg(self.staged_app())),
            _ => Err(format!("the disk image has no {}", name.to_string_lossy())),
        };
        if let Some(volume) = &volume {
            let _ = run(Command::new("/usr/bin/hdiutil").args(["detach", "-force"]).arg(volume));
        }
        let _ = std::fs::remove_dir_all(&root);
        copied
    }
}

impl Installer for MacInstall {
    fn install_path(&self) -> &Path {
        &self.app
    }

    /// In the cache folder of the user, by the path of the install, so two
    /// Saysos with different data folders share it.
    fn lock_path(&self) -> PathBuf {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
        let name: String = self.app.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        home.join("Library/Caches/dev.sayso.Sayso/install-locks").join(format!("{name}.lock"))
    }

    fn record_path(&self) -> PathBuf {
        self.staging_dir().join("update.json")
    }

    fn installed_version(&self) -> Option<Version> {
        bundle_version(&self.app).ok()?.trim().parse().ok()
    }

    fn stage(&self, file: &Path, version: &Version) -> Result<(), String> {
        self.remove_staging();
        self.make_staging_dir().map_err(|e| e.to_string())?;
        let staged = self.copy_from_image(file).and_then(|()| self.verify_staged()).and_then(|()| {
            let found = bundle_version(&self.staged_app())?;
            if found.trim() == version.to_string() {
                Ok(())
            } else {
                Err(format!("the disk image has version {}, and the update information says {version}", found.trim()))
            }
        });
        if staged.is_err() {
            self.remove_staging();
        }
        staged
    }

    fn staging_exists(&self) -> bool {
        self.staged_app().is_dir()
    }

    fn verify_staged(&self) -> Result<(), String> {
        let app = self.staged_app();
        run(Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict", "-R", REQUIREMENT]).arg(&app))
            .map_err(|e| format!("the signature of the new version is not the signature of Sayso ({e})"))?;
        run(Command::new("/usr/sbin/spctl").args(["--assess", "--type", "execute"]).arg(&app))
            .map_err(|e| format!("macOS did not accept the new version ({e})"))
    }

    fn exchange(&self) -> std::io::Result<()> {
        swap(&self.app, &self.staged_app())?;
        crate::files::sync_dir(self.app.parent().unwrap_or(Path::new("/")));
        Ok(())
    }

    /// A detached shell waits until this process is gone, then opens the
    /// bundle through Launch Services. `-n` makes Launch Services start the
    /// bundle also when it still lists the old process, or another copy of
    /// Sayso. The wait keeps it to one Sayso for this bundle.
    fn relaunch(&self) -> std::io::Result<()> {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", r#"pid=$1; shift; while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done; exec /usr/bin/open -n "$@""#, "sayso-relaunch"]);
        command.arg(std::process::id().to_string());
        for name in PASS_ENV {
            if let Ok(value) = std::env::var(name) {
                command.arg("--env").arg(format!("{name}={value}"));
            }
        }
        command.arg(&self.app);
        // Its own process group, so the shell lives when this app quits.
        command.process_group(0).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        command.spawn().map(|_| ())
    }

    fn remove_staging(&self) {
        let dir = self.staging_dir();
        // Only inside a real folder: a symlink there is not followed.
        if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            return;
        }
        let _ = std::fs::remove_dir_all(self.staged_app());
        let _ = std::fs::remove_dir_all(dir.join(".mount"));
        // Gone when the record is gone too.
        let _ = std::fs::remove_dir(&dir);
    }
}

/// Exchange two paths in one step (`renamex_np` with `RENAME_SWAP`).
fn swap(a: &Path, b: &Path) -> std::io::Result<()> {
    let a = CString::new(a.as_os_str().as_bytes())?;
    let b = CString::new(b.as_os_str().as_bytes())?;
    // SAFETY: both arguments are valid C strings.
    if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) } == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// `CFBundleShortVersionString` of a bundle.
fn bundle_version(app: &Path) -> Result<String, String> {
    output(Command::new("/usr/libexec/PlistBuddy").args(["-c", "Print :CFBundleShortVersionString"]).arg(app.join("Contents/Info.plist")))
}

fn run(command: &mut Command) -> Result<(), String> {
    output(command).map(|_| ())
}

/// Run a system tool. An error has the name of the tool and what it wrote.
fn output(command: &mut Command) -> Result<String, String> {
    let tool = command.get_program().to_string_lossy().into_owned();
    let out = command.output().map_err(|e| format!("{tool}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("{tool}: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(dir: &Path, marker: &str) -> PathBuf {
        let app = dir.join("Sayso.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/Sayso"), marker).unwrap();
        app
    }

    #[test]
    fn an_install_in_a_folder_of_the_user_can_update_itself() {
        let dir = tempfile::tempdir().unwrap();
        let app = bundle(&dir.path().canonicalize().unwrap(), "old");
        let install = MacInstall::for_executable(&app.join("Contents/MacOS/Sayso")).unwrap();
        assert_eq!(install.install_path(), app);
        // The probe leaves nothing behind.
        assert!(!install.staging_dir().exists());
    }

    #[test]
    fn a_binary_outside_a_bundle_and_a_translocated_app_are_manual() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(MacInstall::for_executable(&dir.path().join("target/debug/sayso")).err(), Some(Manual::WrongFolder));
        let moved = Path::new("/private/var/folders/x/AppTranslocation/ABC/d/Sayso.app/Contents/MacOS/Sayso");
        assert_eq!(MacInstall::for_executable(moved).err(), Some(Manual::WrongFolder));
    }

    #[test]
    fn a_folder_that_the_user_cannot_write_is_manual() {
        // The root user can write everywhere, so the check says nothing there.
        if unsafe { libc::getuid() } == 0 {
            return;
        }
        let result = MacInstall::for_executable(Path::new("/usr/Sayso.app/Contents/MacOS/Sayso")).err();
        assert!(matches!(result, Some(Manual::NoPermission | Manual::WrongFolder)), "{result:?}");
    }

    #[test]
    fn the_exchange_swaps_both_bundles_and_a_second_call_puts_them_back() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let app = bundle(&root, "old");
        let install = MacInstall::for_executable(&app.join("Contents/MacOS/Sayso")).unwrap();
        install.make_staging_dir().unwrap();
        bundle(&install.staging_dir(), "new");
        assert!(install.staging_exists());
        let read = |p: PathBuf| std::fs::read_to_string(p.join("Contents/MacOS/Sayso")).unwrap();
        install.exchange().unwrap();
        assert_eq!(read(app.clone()), "new");
        assert_eq!(read(install.staged_app()), "old");
        install.exchange().unwrap();
        assert_eq!(read(app.clone()), "old");
        install.remove_staging();
        assert!(!install.staging_exists());
        assert!(!install.staging_dir().exists());
        assert!(app.exists());
    }

    #[test]
    fn two_bundles_in_one_folder_have_their_own_staging_folder_and_lock() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let a = MacInstall { app: root.join("Sayso.app") };
        let b = MacInstall { app: root.join("Sayso copy.app") };
        assert_ne!(a.staging_dir(), b.staging_dir());
        assert_ne!(a.record_path(), b.record_path());
        assert_ne!(a.lock_path(), b.lock_path());
    }

    #[test]
    fn remove_staging_keeps_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let install = MacInstall { app: bundle(&root, "old") };
        install.make_staging_dir().unwrap();
        bundle(&install.staging_dir(), "new");
        std::fs::write(install.record_path(), "{}").unwrap();
        install.remove_staging();
        assert!(!install.staging_exists());
        assert!(install.record_path().exists());
    }

    #[test]
    fn a_staging_path_that_is_a_symlink_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let app = bundle(&root, "old");
        let install = MacInstall { app };
        let target = root.join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep"), "x").unwrap();
        std::os::unix::fs::symlink(&target, install.staging_dir()).unwrap();
        assert!(install.make_staging_dir().is_err());
        // Cleanup does not follow the link.
        install.remove_staging();
        assert!(target.join("keep").exists());
    }

    #[test]
    fn an_unsigned_bundle_does_not_verify() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let install = MacInstall { app: bundle(&root, "old") };
        install.make_staging_dir().unwrap();
        bundle(&install.staging_dir(), "new");
        assert!(install.verify_staged().is_err());
    }
}
