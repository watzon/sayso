//! The swap: the staged update takes the place of the installed app.
//!
//! The order and the rules are in `docs/updates.md`, "The swap on macOS and
//! Linux, step by step" and "Recovery at start".

use crate::install::Installer;
use crate::lock::InstallLock;
use crate::manifest::{self, PublicKey, Verified};
use crate::record::{Phase, Record};
use crate::state::UpdateState;
use semver::Version;
use std::path::PathBuf;

/// What the swap needs to know about this Sayso.
pub struct Context {
    /// `update-state.json` in the data folder.
    pub state_path: PathBuf,
    pub keys: Vec<PublicKey>,
    /// [`manifest::URL_PREFIX`], except with a local test server.
    pub url_prefix: String,
    pub running: Version,
}

/// What the start of Sayso found.
#[derive(Debug, Clone, PartialEq)]
pub enum AtStart {
    /// No update is in progress.
    Nothing,
    /// The swap ran, and the new version starts. This process must exit now.
    Relaunching,
    /// This process is the old version, and the new version is installed:
    /// another Sayso did the swap after this process started. That Sayso
    /// starts the new version, so this process must exit now.
    Stale,
    /// A staged update waits, because another Sayso runs from this install.
    Ready(Version),
    /// This is the first start of the new version. Call [`confirm`] when it runs well.
    Updated(Version),
    /// The last update failed.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SwapError {
    /// Another Sayso runs from this install, or this Sayso prepares an
    /// update at this moment. The staged update stays.
    Busy,
    /// The update is gone, with the cause for the user.
    Failed(String),
}

/// Run "Recovery at start". Call it before the engine starts, with the
/// shared lock of the install.
pub fn at_start(context: &Context, installer: &dyn Installer, lock: &InstallLock) -> AtStart {
    let path = installer.record_path();
    let Some(record) = Record::load(&path) else {
        // A staged app without a record is left over from a crash during staging.
        installer.remove_staging();
        return AtStart::Nothing;
    };
    // A record for another path: the bundle got another name after staging.
    if record.install_path != installer.install_path() {
        installer.remove_staging();
        Record::delete(&path);
        return AtStart::Nothing;
    }
    let running_new = context.running == record.to_version;
    match record.phase {
        Phase::Staged if !installer.staging_exists() => {
            Record::delete(&path);
            AtStart::Nothing
        }
        Phase::Staged => match swap(context, installer, lock) {
            Ok(()) => AtStart::Relaunching,
            Err(SwapError::Busy) => AtStart::Ready(record.to_version),
            Err(SwapError::Failed(message)) => AtStart::Failed(message),
        },
        // The exchange happened, and this is the new version.
        Phase::Swapping | Phase::Swapped if running_new => AtStart::Updated(record.to_version),
        // The exchange happened after this old process started. The staging
        // folder holds the old version, which the new version still needs.
        Phase::Swapping | Phase::Swapped if installer.installed_version().as_ref() == Some(&record.to_version) => AtStart::Stale,
        // The exchange did not happen, or it was put back.
        Phase::Swapping | Phase::Swapped => {
            installer.remove_staging();
            let message = "Sayso could not install the update. You still have the version from before.";
            record.fail(&path, message);
            AtStart::Failed(message.into())
        }
        Phase::Failed => AtStart::Failed(record.error.unwrap_or_else(|| "The update failed.".into())),
    }
}

/// Do the swap and start the new version. After `Ok`, the caller must quit
/// at once: the installed app is the new version.
pub fn swap(context: &Context, installer: &dyn Installer, lock: &InstallLock) -> Result<(), SwapError> {
    let Some(_exclusive) = lock.try_exclusive() else {
        return Err(SwapError::Busy);
    };
    // Read the record with the lock held: a staging step can have changed it.
    let Some(mut record) = Record::load(&installer.record_path()).filter(|r| r.phase == Phase::Staged) else {
        return Err(SwapError::Failed("No update is ready to install.".into()));
    };
    swap_locked(context, installer, &mut record)
}

fn swap_locked(context: &Context, installer: &dyn Installer, record: &mut Record) -> Result<(), SwapError> {
    let path = installer.record_path();
    let give_up = |record: &Record, message: String| {
        installer.remove_staging();
        record.clone().fail(&path, message.clone());
        SwapError::Failed(message)
    };
    if let Err(message) = check(context, installer, record) {
        return Err(give_up(record, message));
    }
    record.phase = Phase::Swapping;
    record.attempts += 1;
    record.save(&path).map_err(|e| give_up(record, format!("Sayso could not write the update record: {e}")))?;
    installer.exchange().map_err(|e| give_up(record, format!("Sayso could not replace the app: {e}")))?;
    record.phase = Phase::Swapped;
    if let Err(e) = record.save(&path) {
        // The exchange is done. `swapping` with the new version means the same at the next start.
        log::warn!("update: could not write the update record after the exchange: {e}");
    }
    if let Err(e) = installer.relaunch() {
        // Put the old version back, so this process and the installed app are the same again.
        return Err(match installer.exchange() {
            Ok(()) => give_up(record, format!("Sayso could not start the new version: {e}. You still have the version from before.")),
            Err(back) => {
                // The staging folder holds the old version. Keep it for a repair by hand.
                let message = format!("Sayso could not start the new version: {e}. It could not put the old version back: {back}. Install Sayso again from the download page.");
                record.clone().fail(&path, message.clone());
                SwapError::Failed(message)
            }
        });
    }
    Ok(())
}

/// The checks before the exchange: the staged update is the one that the
/// signed manifest names, for this install.
fn check(context: &Context, installer: &dyn Installer, record: &Record) -> Result<(), String> {
    match manifest::verify_for(record.envelope.as_bytes(), &context.keys, &context.url_prefix) {
        Ok(Verified::Manifest(manifest)) if manifest.version == record.to_version => {}
        _ => return Err("The update record does not have a signature of Sayso.".into()),
    }
    if record.install_path != installer.install_path() {
        return Err("Sayso moved to another folder after the update was prepared.".into());
    }
    if record.to_version.cmp_precedence(&context.running).is_le() {
        return Err("The prepared update is not newer than this version.".into());
    }
    if !installer.staging_exists() {
        return Err("The prepared update is gone.".into());
    }
    installer.verify_staged()
}

/// True while the old version waits in the staging folder for [`confirm`].
pub fn awaits_confirm(installer: &dyn Installer) -> bool {
    Record::load(&installer.record_path()).is_some_and(|r| matches!(r.phase, Phase::Swapping | Phase::Swapped))
}

/// The new version runs well: delete the old version and the record, and
/// keep the version for the "Sayso is now X" notice. Safe to call again.
/// Returns false when it must run again later: another Sayso uses the install.
pub fn confirm(context: &Context, installer: &dyn Installer, lock: &InstallLock) -> bool {
    let Some(_exclusive) = lock.try_exclusive() else { return false };
    let path = installer.record_path();
    let Some(record) = Record::load(&path) else { return true };
    let this_update = matches!(record.phase, Phase::Swapping | Phase::Swapped)
        && record.to_version == context.running
        && record.install_path == installer.install_path();
    if !this_update {
        return true;
    }
    installer.remove_staging();
    Record::delete(&path);
    installer.remove_staging();
    let mut state = UpdateState::load(&context.state_path);
    state.notice = Some(record.to_version);
    if let Err(e) = state.save(&context.state_path) {
        log::warn!("update: could not write {}: {e}", context.state_path.display());
    }
    true
}

/// "Try again" after a failure: forget the failed update.
pub fn clear_failure(installer: &dyn Installer, lock: &InstallLock) {
    let Some(_exclusive) = lock.try_exclusive() else { return };
    let path = installer.record_path();
    if Record::load(&path).is_some_and(|r| r.phase == Phase::Failed) {
        installer.remove_staging();
        Record::delete(&path);
        installer.remove_staging();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::manifest::tests::{file, key, payload, public};
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    /// An installer on two marker files: `installed` and `staged` hold a version each.
    pub struct FakeInstaller {
        pub dir: PathBuf,
        pub verify: Mutex<Result<(), String>>,
        pub exchange_fails: AtomicBool,
        pub relaunch_fails: AtomicBool,
        pub relaunches: AtomicU32,
        /// Set to pause `verify_staged`, for a test with two threads.
        pub verify_gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    }

    impl FakeInstaller {
        pub fn new(dir: &Path, installed: &str) -> Self {
            std::fs::write(dir.join("installed"), installed).unwrap();
            Self {
                dir: dir.to_path_buf(),
                verify: Mutex::new(Ok(())),
                exchange_fails: AtomicBool::new(false),
                relaunch_fails: AtomicBool::new(false),
                relaunches: AtomicU32::new(0),
                verify_gate: Mutex::new(None),
            }
        }
        pub fn installed(&self) -> String {
            std::fs::read_to_string(self.dir.join("installed")).unwrap()
        }
        pub fn staged(&self) -> Option<String> {
            std::fs::read_to_string(self.dir.join("staged")).ok()
        }
    }

    impl Installer for FakeInstaller {
        fn install_path(&self) -> &Path {
            &self.dir
        }
        fn lock_path(&self) -> PathBuf {
            self.dir.join("lock")
        }
        fn record_path(&self) -> PathBuf {
            self.dir.join("update.json")
        }
        fn installed_version(&self) -> Option<Version> {
            self.installed().parse().ok()
        }
        fn stage(&self, file: &Path, version: &Version) -> Result<(), String> {
            let bytes = std::fs::read(file).map_err(|e| e.to_string())?;
            if bytes.starts_with(b"bad") {
                return Err("the file is not a release of Sayso".into());
            }
            std::fs::write(self.dir.join("staged"), version.to_string()).map_err(|e| e.to_string())
        }
        fn staging_exists(&self) -> bool {
            self.dir.join("staged").exists()
        }
        fn verify_staged(&self) -> Result<(), String> {
            if let Some(gate) = self.verify_gate.lock().unwrap().as_ref() {
                let _ = gate.recv();
            }
            self.verify.lock().unwrap().clone()
        }
        fn exchange(&self) -> std::io::Result<()> {
            if self.exchange_fails.load(Ordering::SeqCst) {
                return Err(std::io::Error::other("no exchange"));
            }
            let (a, b, t) = (self.dir.join("installed"), self.dir.join("staged"), self.dir.join("tmp"));
            std::fs::rename(&a, &t)?;
            std::fs::rename(&b, &a)?;
            std::fs::rename(&t, &b)
        }
        fn relaunch(&self) -> std::io::Result<()> {
            if self.relaunch_fails.load(Ordering::SeqCst) {
                return Err(std::io::Error::other("no open"));
            }
            self.relaunches.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn remove_staging(&self) {
            let _ = std::fs::remove_file(self.dir.join("staged"));
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        installer: FakeInstaller,
        lock: InstallLock,
    }

    fn context(dir: &Path, running: &str) -> Context {
        Context {
            state_path: dir.join("update-state.json"),
            keys: vec![public("a", &key(1))],
            url_prefix: manifest::URL_PREFIX.into(),
            running: running.parse().unwrap(),
        }
    }

    /// 0.2.0 is installed, and 0.3.0 is staged with a good record.
    fn staged() -> (Fixture, Context) {
        let dir = tempfile::tempdir().unwrap();
        let installer = FakeInstaller::new(dir.path(), "0.2.0");
        std::fs::write(dir.path().join("staged"), "0.3.0").unwrap();
        let context = context(dir.path(), "0.2.0");
        record(&installer, Phase::Staged).save(&installer.record_path()).unwrap();
        let lock = InstallLock::shared(&installer.lock_path()).unwrap().0;
        (Fixture { _dir: dir, installer, lock }, context)
    }

    fn record(installer: &FakeInstaller, phase: Phase) -> Record {
        Record {
            envelope: String::from_utf8(file(&payload("0.3.0"), "a", &key(1))).unwrap(),
            from_version: Version::new(0, 2, 0),
            to_version: Version::new(0, 3, 0),
            install_path: installer.dir.clone(),
            phase,
            attempts: 0,
            error: None,
        }
    }

    fn phase(f: &Fixture) -> Option<Phase> {
        Record::load(&f.installer.record_path()).map(|r| r.phase)
    }

    #[test]
    fn a_staged_update_swaps_at_start_and_the_new_version_confirms() {
        let (f, context) = staged();
        assert_eq!(at_start(&context, &f.installer, &f.lock), AtStart::Relaunching);
        assert_eq!(f.installer.installed(), "0.3.0");
        // The old version stays until the new one confirms.
        assert_eq!(f.installer.staged().as_deref(), Some("0.2.0"));
        assert_eq!(phase(&f), Some(Phase::Swapped));
        assert_eq!(f.installer.relaunches.load(Ordering::SeqCst), 1);

        // The new version starts.
        let new = self::context(&f.installer.dir, "0.3.0");
        assert_eq!(at_start(&new, &f.installer, &f.lock), AtStart::Updated(Version::new(0, 3, 0)));
        assert_eq!(f.installer.staged().as_deref(), Some("0.2.0"));
        assert!(confirm(&new, &f.installer, &f.lock));
        assert_eq!(f.installer.staged(), None);
        assert_eq!(phase(&f), None);
        assert_eq!(UpdateState::load(&new.state_path).notice, Some(Version::new(0, 3, 0)));
        // A second call and a later start do nothing.
        assert!(confirm(&new, &f.installer, &f.lock));
        assert_eq!(at_start(&new, &f.installer, &f.lock), AtStart::Nothing);
    }

    #[test]
    fn a_crash_after_the_phase_write_and_before_the_exchange_fails_safely() {
        let (f, context) = staged();
        let mut r = record(&f.installer, Phase::Swapping);
        r.attempts = 1;
        r.save(&f.installer.record_path()).unwrap();
        assert!(matches!(at_start(&context, &f.installer, &f.lock), AtStart::Failed(_)));
        assert_eq!(f.installer.installed(), "0.2.0");
        assert_eq!(f.installer.staged(), None);
        assert_eq!(phase(&f), Some(Phase::Failed));
        // The failure shows again at the next start, and it does not swap again.
        assert!(matches!(at_start(&context, &f.installer, &f.lock), AtStart::Failed(_)));
        clear_failure(&f.installer, &f.lock);
        assert_eq!(at_start(&context, &f.installer, &f.lock), AtStart::Nothing);
    }

    #[test]
    fn a_crash_after_the_exchange_is_an_update() {
        let (f, _) = staged();
        f.installer.exchange().unwrap();
        let new = context(&f.installer.dir, "0.3.0");
        record(&f.installer, Phase::Swapping).save(&f.installer.record_path()).unwrap();
        assert_eq!(at_start(&new, &f.installer, &f.lock), AtStart::Updated(Version::new(0, 3, 0)));
    }

    #[test]
    fn a_missing_staging_folder_or_a_leftover_one_is_cleaned() {
        let (f, context) = staged();
        f.installer.remove_staging();
        assert_eq!(at_start(&context, &f.installer, &f.lock), AtStart::Nothing);
        assert_eq!(phase(&f), None);
        std::fs::write(f.installer.dir.join("staged"), "junk").unwrap();
        assert_eq!(at_start(&context, &f.installer, &f.lock), AtStart::Nothing);
        assert_eq!(f.installer.staged(), None);
    }

    #[test]
    fn another_sayso_on_the_install_keeps_the_update_ready() {
        let (f, context) = staged();
        let other = InstallLock::shared(&f.installer.lock_path()).unwrap().0;
        assert_eq!(at_start(&context, &f.installer, &f.lock), AtStart::Ready(Version::new(0, 3, 0)));
        assert_eq!(f.installer.installed(), "0.2.0");
        assert_eq!(phase(&f), Some(Phase::Staged));
        drop(other);
        assert_eq!(at_start(&context, &f.installer, &f.lock), AtStart::Relaunching);
    }

    #[test]
    fn a_staged_app_that_does_not_verify_is_deleted() {
        let (f, context) = staged();
        *f.installer.verify.lock().unwrap() = Err("the signature is not the signature of Sayso".into());
        assert!(matches!(swap(&context, &f.installer, &f.lock), Err(SwapError::Failed(m)) if m.contains("signature")));
        assert_eq!(f.installer.installed(), "0.2.0");
        assert_eq!(f.installer.staged(), None);
        assert_eq!(phase(&f), Some(Phase::Failed));
    }

    #[test]
    fn a_record_that_was_changed_does_not_swap() {
        let cases: [fn(&mut Record); 4] = [
            // A manifest that another key signed.
            |r| r.envelope = String::from_utf8(file(&payload("0.3.0"), "a", &key(2))).unwrap(),
            // A good manifest of another version.
            |r| r.envelope = String::from_utf8(file(&payload("0.2.5"), "a", &key(1))).unwrap(),
            |r| r.install_path = PathBuf::from("/Applications/Other.app"),
            // An update to the version that already runs.
            |r| {
                r.to_version = Version::new(0, 2, 0);
                r.envelope = String::from_utf8(file(&payload("0.2.0"), "a", &key(1))).unwrap();
            },
        ];
        for change in cases {
            let (f, context) = staged();
            let mut r = record(&f.installer, Phase::Staged);
            change(&mut r);
            r.save(&f.installer.record_path()).unwrap();
            assert!(matches!(swap(&context, &f.installer, &f.lock), Err(SwapError::Failed(_))));
            assert_eq!(f.installer.installed(), "0.2.0");
            assert_eq!(f.installer.relaunches.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn an_exchange_that_fails_leaves_the_old_version() {
        let (f, context) = staged();
        f.installer.exchange_fails.store(true, Ordering::SeqCst);
        assert!(matches!(swap(&context, &f.installer, &f.lock), Err(SwapError::Failed(_))));
        assert_eq!(f.installer.installed(), "0.2.0");
        assert_eq!(phase(&f), Some(Phase::Failed));
    }

    #[test]
    fn a_start_that_fails_puts_the_old_version_back() {
        let (f, context) = staged();
        f.installer.relaunch_fails.store(true, Ordering::SeqCst);
        assert!(matches!(swap(&context, &f.installer, &f.lock), Err(SwapError::Failed(m)) if m.contains("still have")));
        assert_eq!(f.installer.installed(), "0.2.0");
        assert_eq!(f.installer.staged(), None);
        assert_eq!(phase(&f), Some(Phase::Failed));
        // The lock is free again for a later try.
        assert!(f.lock.try_exclusive().is_some());
    }

    #[test]
    fn the_old_version_after_a_swap_back_by_hand_fails_and_cleans() {
        let (f, context) = staged();
        // The record says swapped, but the old version runs.
        record(&f.installer, Phase::Swapped).save(&f.installer.record_path()).unwrap();
        assert!(matches!(at_start(&context, &f.installer, &f.lock), AtStart::Failed(_)));
        assert_eq!(f.installer.staged(), None);
    }

    #[test]
    fn an_old_process_that_starts_after_the_swap_exits_and_keeps_the_backup() {
        let (f, context) = staged();
        assert_eq!(swap(&context, &f.installer, &f.lock), Ok(()));
        // `context` is still the old version: a process that started before the exchange.
        assert_eq!(at_start(&context, &f.installer, &f.lock), AtStart::Stale);
        assert_eq!(f.installer.staged().as_deref(), Some("0.2.0"));
        assert_eq!(phase(&f), Some(Phase::Swapped));
    }

    #[test]
    fn confirm_waits_while_another_sayso_uses_the_install_and_ignores_other_updates() {
        let (f, context) = staged();
        swap(&context, &f.installer, &f.lock).unwrap();
        let new = self::context(&f.installer.dir, "0.3.0");
        let other = InstallLock::shared(&f.installer.lock_path()).unwrap().0;
        assert!(!confirm(&new, &f.installer, &f.lock));
        assert_eq!(f.installer.staged().as_deref(), Some("0.2.0"));
        drop(other);
        // A version that is not the version of the record does not confirm it.
        let wrong = self::context(&f.installer.dir, "0.4.0");
        assert!(confirm(&wrong, &f.installer, &f.lock));
        assert_eq!(f.installer.staged().as_deref(), Some("0.2.0"));
        assert!(awaits_confirm(&f.installer));
        assert!(confirm(&new, &f.installer, &f.lock));
        assert_eq!(f.installer.staged(), None);
        assert!(!awaits_confirm(&f.installer));
    }

    #[test]
    fn a_record_for_another_path_is_dropped() {
        let (f, context) = staged();
        let mut r = record(&f.installer, Phase::Swapped);
        r.install_path = PathBuf::from("/Applications/Other.app");
        r.save(&f.installer.record_path()).unwrap();
        assert_eq!(at_start(&context, &f.installer, &f.lock), AtStart::Nothing);
        assert_eq!(phase(&f), None);
        assert_eq!(f.installer.installed(), "0.2.0");
    }

    #[test]
    fn a_swap_and_a_second_update_step_do_not_run_at_the_same_time() {
        let (f, context) = staged();
        let (open, gate) = std::sync::mpsc::channel();
        *f.installer.verify_gate.lock().unwrap() = Some(gate);
        std::thread::scope(|s| {
            let swapping = s.spawn(|| swap(&context, &f.installer, &f.lock));
            // The swap holds the lock while it verifies. A staging step cannot start.
            std::thread::sleep(std::time::Duration::from_millis(100));
            assert!(f.lock.try_exclusive().is_none());
            assert_eq!(swap(&context, &f.installer, &f.lock), Err(SwapError::Busy));
            open.send(()).unwrap();
            assert_eq!(swapping.join().unwrap(), Ok(()));
        });
        assert_eq!(f.installer.installed(), "0.3.0");
        assert_eq!(Record::load(&f.installer.record_path()).unwrap().to_version, Version::new(0, 3, 0));
    }
}
