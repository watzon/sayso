//! The updater: one thread that checks for a new version, downloads and
//! stages it, and reports each [`Status`].

use crate::check::{self, CheckError};
use crate::download::{self, DownloadError};
use crate::install::{self, Installer, Manual};
use crate::lock::InstallLock;
use crate::manifest::{self, Offer, PublicKey, Release, Verified};
use crate::record::{Phase, Record};
use crate::state::UpdateState;
use crate::swap::{self, AtStart, Context, SwapError};
use crossbeam_channel::{RecvTimeoutError, Sender};
use semver::Version;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What the UI shows about updates.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// No newer version is known. `last_check` is the time of the last check
    /// that succeeded, in seconds since 1970.
    Idle { last_check: Option<u64> },
    /// A check that the user started is in progress.
    Checking,
    /// A newer version exists for this system. With a `manual` reason, this
    /// install cannot update itself, and the user gets the download page.
    Available { version: Version, notes_url: String, manual: Option<Manual> },
    Downloading { version: Version, received: u64, total: u64 },
    /// The download is complete. Sayso unpacks and verifies the new version.
    Preparing { version: Version },
    /// The new version is staged. It takes the place of this one at the next
    /// start, or when the user selects "Restart to update".
    Ready { version: Version },
    /// The newest release has update information that this build cannot
    /// read: a newer key or a newer schema. The user must update by hand.
    Unreadable,
    /// A check or an update did not succeed.
    Failed { message: String },
}

pub struct Options {
    pub url: String,
    /// False only for a local test server.
    pub https_only: bool,
    pub platform: String,
    /// Check without a request from the user: after `first_delay`, then each `interval`.
    pub auto_check: bool,
    /// Download and stage a newer version without a request from the user.
    pub automatic: bool,
    pub first_delay: Duration,
    pub interval: Duration,
    /// Where a release file goes during the download.
    pub downloads_dir: PathBuf,
    pub context: Context,
    /// The installer of this install, or why it has none.
    pub installer: Result<Arc<dyn Installer>, Manual>,
    /// The shared lock of this install. None when the install has no installer.
    pub lock: Option<InstallLock>,
}

impl Options {
    /// The options of this build, or None when this build does not look for
    /// updates: it is not an official build, or it has no public key.
    ///
    /// A debug build also looks for updates when `SAYSO_UPDATE_URL` and
    /// `SAYSO_UPDATE_KEY` (`<id>:<public key in base64>`) are set, so a
    /// complete run works against a local server. A release build ignores both.
    ///
    /// This takes the shared lock of the install. Call it one time, at start.
    pub fn for_this_build(data_dir: &Path, cache_dir: &Path, auto_check: bool, automatic: bool) -> Option<Self> {
        let installer = install::detect();
        let lock = match &installer {
            Ok(installer) => InstallLock::shared(&installer.lock_path()).inspect_err(|e| log::warn!("update: no install lock: {e}")).ok(),
            Err(_) => None,
        };
        let running = Version::parse(crate::VERSION).ok()?;
        // The lock had to wait, so a swap can have run. When the installed app
        // is now another version, this process is the old one. The Sayso that
        // did the swap starts the new version, so this process only exits.
        if let (Ok(installer), Some((_, true))) = (&installer, &lock)
            && installer.installed_version().is_some_and(|installed| installed != running)
        {
            std::process::exit(0);
        }
        let mut options = Self {
            url: crate::MANIFEST_URL.to_string(),
            https_only: true,
            platform: crate::platform(),
            auto_check,
            automatic,
            first_delay: Duration::from_secs(30),
            interval: Duration::from_secs(24 * 60 * 60),
            downloads_dir: cache_dir.join("updates"),
            context: Context {
                state_path: data_dir.join("update-state.json"),
                keys: PublicKey::release_keys(),
                url_prefix: manifest::URL_PREFIX.to_string(),
                running,
            },
            // Without the lock, a swap is not safe.
            installer: if lock.is_some() { installer } else { installer.and(Err(Manual::NoPermission)) },
            lock: lock.map(|(lock, _)| lock),
        };
        #[cfg(debug_assertions)]
        if let (Ok(url), Ok(key)) = (std::env::var("SAYSO_UPDATE_URL"), std::env::var("SAYSO_UPDATE_KEY")) {
            let (id, key) = key.split_once(':')?;
            options.https_only = url.starts_with("https://");
            options.context.url_prefix = url.rsplit_once('/').map_or(url.clone(), |(base, _)| format!("{base}/"));
            options.url = url;
            options.context.keys = vec![PublicKey::parse(id, key).ok()?];
            options.first_delay = Duration::from_secs(2);
            return Some(options);
        }
        (crate::official_build() && !options.context.keys.is_empty()).then_some(options)
    }

    /// Run "Recovery at start": do a staged swap, or find what the last
    /// update left. Call it before the engine starts. After
    /// [`AtStart::Relaunching`] and [`AtStart::Stale`], exit the process at once.
    pub fn at_start(&self) -> AtStart {
        match (&self.installer, &self.lock) {
            (Ok(installer), Some(lock)) => swap::at_start(&self.context, installer.as_ref(), lock),
            _ => AtStart::Nothing,
        }
    }
}

enum Command {
    Check,
    Install,
    SetAutoCheck(bool),
    SetAutomatic(bool),
}

/// What the thread and the handle share.
struct Shared {
    context: Context,
    installer: Result<Arc<dyn Installer>, Manual>,
    lock: Option<InstallLock>,
    cancel: AtomicBool,
}

/// A handle to the updater thread. The thread stops when the handle drops.
pub struct Updater {
    commands: Sender<Command>,
    shared: Arc<Shared>,
}

impl Updater {
    /// Start the thread. `on_status` runs on that thread for each new status.
    pub fn spawn(options: Options, on_status: impl Fn(Status) + Send + 'static) -> Self {
        let (commands, rx) = crossbeam_channel::unbounded();
        let shared = Arc::new(Shared { context: options.context, installer: options.installer, lock: options.lock, cancel: AtomicBool::new(false) });
        let worker = Worker {
            url: options.url,
            https_only: options.https_only,
            platform: options.platform,
            downloads_dir: options.downloads_dir,
            automatic: options.automatic,
            shared: shared.clone(),
            offer: None,
            on_status: Box::new(on_status),
        };
        let (first_delay, interval, auto_check) = (options.first_delay, options.interval, options.auto_check);
        let spawned = std::thread::Builder::new().name("sayso-update".into()).spawn(move || {
            let mut worker = worker;
            let mut auto = auto_check;
            let mut next = auto.then(|| Instant::now() + first_delay);
            loop {
                let command = match next {
                    Some(at) => rx.recv_deadline(at),
                    None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                };
                match command {
                    Ok(Command::SetAutoCheck(on)) => {
                        if on != auto {
                            auto = on;
                            next = on.then(|| Instant::now() + first_delay);
                        }
                    }
                    Ok(Command::SetAutomatic(on)) => worker.automatic = on,
                    Ok(Command::Install) => worker.install(true),
                    Ok(Command::Check) => {
                        worker.check(true);
                        next = auto.then(|| Instant::now() + interval);
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        worker.check(false);
                        next = auto.then(|| Instant::now() + interval);
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        if let Err(e) = spawned {
            log::error!("update: could not start the updater thread: {e}");
        }
        Self { commands, shared }
    }

    /// Check now, because the user asked. The answer is `Checking`, then one more status.
    pub fn check_now(&self) {
        let _ = self.commands.send(Command::Check);
    }

    /// "Update now": download and stage the version of the last check.
    pub fn install(&self) {
        let _ = self.commands.send(Command::Install);
    }

    /// Stop the download in progress. The status goes back to `Available`.
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::SeqCst);
    }

    /// Turn the checks that the user did not start on or off.
    pub fn set_auto_check(&self, on: bool) {
        let _ = self.commands.send(Command::SetAutoCheck(on));
    }

    pub fn set_automatic(&self, on: bool) {
        let _ = self.commands.send(Command::SetAutomatic(on));
    }

    /// "Restart to update": put the staged version in place and start it.
    /// After `Ok`, quit the app at once.
    pub fn swap(&self) -> Result<(), SwapError> {
        match (&self.shared.installer, &self.shared.lock) {
            (Ok(installer), Some(lock)) => swap::swap(&self.shared.context, installer.as_ref(), lock),
            _ => Err(SwapError::Failed("This install cannot update itself.".into())),
        }
    }

    /// The new version runs well: delete the old version. False when it
    /// must run again later, because another Sayso uses the install.
    pub fn confirm(&self) -> bool {
        match (&self.shared.installer, &self.shared.lock) {
            (Ok(installer), Some(lock)) => swap::confirm(&self.shared.context, installer.as_ref(), lock),
            _ => true,
        }
    }

    /// The version for the "Sayso is now X" notice, until [`Self::dismiss_notice`].
    pub fn notice(&self) -> Option<Version> {
        UpdateState::load(&self.shared.context.state_path).notice
    }

    pub fn dismiss_notice(&self) {
        let path = &self.shared.context.state_path;
        let mut state = UpdateState::load(path);
        if state.notice.take().is_some() {
            let _ = state.save(path);
        }
    }
}

struct Worker {
    url: String,
    https_only: bool,
    platform: String,
    downloads_dir: PathBuf,
    automatic: bool,
    shared: Arc<Shared>,
    /// The newer version of the last check, with the bytes of its manifest.
    offer: Option<(String, Release)>,
    on_status: Box<dyn Fn(Status) + Send>,
}

impl Worker {
    fn emit(&self, status: Status) {
        (self.on_status)(status);
    }

    fn available(&self, release: &Release) -> Status {
        Status::Available { version: release.version.clone(), notes_url: release.notes_url.clone(), manual: self.shared.installer.as_ref().err().copied() }
    }

    /// One check: get the manifest, verify it, and compare the versions.
    fn check(&mut self, by_user: bool) {
        if by_user {
            self.emit(Status::Checking);
            // "Try again" after a failed update starts from zero.
            if let (Ok(installer), Some(lock)) = (&self.shared.installer, &self.shared.lock) {
                swap::clear_failure(installer.as_ref(), lock);
            }
        }
        match self.fetch() {
            Ok(Some(release)) => match self.record_phase() {
                Some((Phase::Staged, version)) if version == release.version => self.emit(Status::Ready { version }),
                // The last update is not confirmed, or it failed. Sayso does not
                // start a new one by itself: the old version in the staging
                // folder is the way back, and a failure must not repeat at
                // each check. The status of the start stays.
                Some((Phase::Swapping | Phase::Swapped | Phase::Failed, _)) if !by_user => {}
                _ => {
                    self.emit(self.available(&release));
                    if self.automatic && self.shared.installer.is_ok() {
                        self.install(false);
                    }
                }
            },
            Ok(None) => {}
            // A check that the user did not start fails without a message.
            Err(message) if by_user => self.emit(Status::Failed { message }),
            Err(message) => log::warn!("update: the check failed: {message}"),
        }
    }

    /// The phase and the version of the update record of this install. A
    /// record `staged` counts only when the staged app is there.
    fn record_phase(&self) -> Option<(Phase, Version)> {
        let installer = self.shared.installer.as_ref().ok()?;
        let record = Record::load(&installer.record_path())?;
        (record.phase != Phase::Staged || installer.staging_exists()).then_some((record.phase, record.to_version))
    }

    /// Returns the newer release, or None after it sent the status itself.
    fn fetch(&mut self) -> Result<Option<Release>, String> {
        let context = &self.shared.context;
        let bytes = check::fetch(&self.url, self.https_only).map_err(|e| match e {
            CheckError::Network(_) => format!("Sayso could not reach the update server. Check your connection, then try again. ({e})"),
            _ => format!("Sayso could not get the update information: {e}. Try again later."),
        })?;
        let verified = manifest::verify_for(&bytes, &context.keys, &context.url_prefix).map_err(|e| format!("Sayso did not accept the update information: {e}."))?;
        let manifest = match verified {
            Verified::Manifest(manifest) => manifest,
            Verified::UnknownKey | Verified::UnknownSchema => {
                self.emit(Status::Unreadable);
                return Ok(None);
            }
        };
        let mut state = UpdateState::load(&context.state_path);
        let offer = manifest::offer(&manifest, &context.running, state.highest_seen.as_ref(), &self.platform);
        if offer == Offer::Replayed {
            return Err(format!(
                "The update server offered version {}, which is older than a version that Sayso saw before. Try again later.",
                manifest.version
            ));
        }
        state.saw(&manifest.version, SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()));
        if let Err(e) = state.save(&context.state_path) {
            log::warn!("update: could not write {}: {e}", context.state_path.display());
        }
        match offer {
            Offer::Newer(release) => {
                self.offer = Some((String::from_utf8_lossy(&bytes).into_owned(), release.clone()));
                Ok(Some(release))
            }
            _ => {
                self.offer = None;
                self.emit(Status::Idle { last_check: state.last_check });
                Ok(None)
            }
        }
    }

    /// Download the release file, stage it, and write the record.
    fn install(&mut self, by_user: bool) {
        let (Some((envelope, release)), Ok(installer)) = (self.offer.clone(), self.shared.installer.clone()) else { return };
        match self.record_phase() {
            Some((Phase::Staged, version)) if version == release.version => return self.emit(Status::Ready { version }),
            Some((Phase::Swapping | Phase::Swapped | Phase::Failed, _)) if !by_user => return,
            _ => {}
        }
        match self.download_and_stage(&envelope, &release, installer.as_ref()) {
            Ok(()) => self.emit(Status::Ready { version: release.version }),
            Err(None) => self.emit(self.available(&release)),
            Err(Some(message)) if by_user => self.emit(Status::Failed { message }),
            Err(Some(message)) => {
                log::warn!("update: the automatic update failed: {message}");
                self.emit(self.available(&release));
            }
        }
    }

    /// `Err(None)` is a download that the user cancelled.
    fn download_and_stage(&self, envelope: &str, release: &Release, installer: &dyn Installer) -> Result<(), Option<String>> {
        let version = &release.version;
        let total = release.asset.size;
        self.shared.cancel.store(false, Ordering::SeqCst);
        self.emit(Status::Downloading { version: version.clone(), received: 0, total });
        let mut last = Instant::now();
        let mut progress = |received: u64| {
            if last.elapsed() >= Duration::from_millis(120) || received == total {
                last = Instant::now();
                self.emit(Status::Downloading { version: version.clone(), received, total });
            }
        };
        let file = download::download(&release.asset, &self.downloads_dir, self.https_only, &self.shared.cancel, &mut progress).map_err(|e| match e {
            DownloadError::Cancelled => None,
            DownloadError::Mismatch => Some("The file that Sayso downloaded is not the file of the release. Try again later.".to_string()),
            DownloadError::Disk(_) => Some(format!("{e}. Make sure that the disk has free space, then try again.")),
            _ => Some(format!("Sayso could not download the update: {e}. Check your connection, then try again.")),
        })?;
        self.emit(Status::Preparing { version: version.clone() });
        let context = &self.shared.context;
        // The exclusive lock stays until the record is written, so a swap
        // never sees new staged files with an old record.
        let Some(_exclusive) = self.shared.lock.as_ref().and_then(|lock| lock.try_exclusive()) else {
            let _ = std::fs::remove_file(&file);
            return Err(Some("Another Sayso runs from this app. Quit it, then try again.".into()));
        };
        let record_path = installer.record_path();
        // The user asked for this update, so the version that runs now is
        // accepted: the old version from the last update goes.
        Record::delete(&record_path);
        let staged = installer.stage(&file, version);
        let _ = std::fs::remove_file(&file);
        staged.map_err(|e| Some(format!("Sayso could not prepare the update: {e}.")))?;
        let record = Record {
            envelope: envelope.to_string(),
            from_version: context.running.clone(),
            to_version: version.clone(),
            install_path: installer.install_path().to_path_buf(),
            phase: Phase::Staged,
            attempts: 0,
            error: None,
        };
        record.save(&record_path).map_err(|e| {
            installer.remove_staging();
            Some(format!("Sayso could not write the update record: {e}."))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::tests::{Server, serve};
    use crate::manifest::tests::{file, key, payload, public};
    use crate::swap::tests::FakeInstaller;

    const WAIT: Duration = Duration::from_secs(10);

    fn options(server: &Server, dir: &tempfile::TempDir, running: &str, auto_check: bool) -> Options {
        Options {
            url: server.url.clone(),
            https_only: false,
            platform: "macos-aarch64".into(),
            auto_check,
            automatic: false,
            first_delay: Duration::from_millis(10),
            interval: Duration::from_secs(3600),
            downloads_dir: dir.path().join("downloads"),
            context: Context {
                state_path: dir.path().join("update-state.json"),
                keys: vec![public("a", &key(1))],
                url_prefix: manifest::URL_PREFIX.into(),
                running: Version::parse(running).unwrap(),
            },
            installer: Err(Manual::Unsupported),
            lock: None,
        }
    }

    fn start(options: Options) -> (Updater, crossbeam_channel::Receiver<Status>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (Updater::spawn(options, move |status| { let _ = tx.send(status); }), rx)
    }

    fn signed(version: &str) -> (u16, Vec<u8>) {
        (200, file(&payload(version), "a", &key(1)))
    }

    fn state(dir: &tempfile::TempDir) -> UpdateState {
        UpdateState::load(&dir.path().join("update-state.json"))
    }

    #[test]
    fn the_first_check_runs_by_itself_and_finds_a_newer_version() {
        let dir = tempfile::tempdir().unwrap();
        let server = serve(vec![signed("0.3.0")]);
        let (_updater, statuses) = start(options(&server, &dir, "0.2.0", true));
        let status = statuses.recv_timeout(WAIT).unwrap();
        assert!(matches!(&status, Status::Available { version, notes_url, manual: Some(Manual::Unsupported) }
            if *version == Version::new(0, 3, 0) && notes_url.ends_with("/tag/v0.3.0")), "{status:?}");
        assert_eq!(state(&dir).highest_seen, Some(Version::new(0, 3, 0)));
        assert!(state(&dir).last_check.is_some());
    }

    #[test]
    fn the_same_version_is_idle_with_the_time_of_the_check() {
        let dir = tempfile::tempdir().unwrap();
        let server = serve(vec![signed("0.3.0")]);
        let (_updater, statuses) = start(options(&server, &dir, "0.3.0", true));
        assert!(matches!(statuses.recv_timeout(WAIT).unwrap(), Status::Idle { last_check: Some(_) }));
    }

    #[test]
    fn a_check_by_itself_fails_without_a_status_and_a_check_by_the_user_shows_it() {
        let dir = tempfile::tempdir().unwrap();
        let server = serve(vec![(404, vec![])]);
        let (updater, statuses) = start(options(&server, &dir, "0.2.0", true));
        while server.requests.load(Ordering::SeqCst) == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        updater.check_now();
        assert_eq!(statuses.recv_timeout(WAIT).unwrap(), Status::Checking);
        assert!(matches!(statuses.recv_timeout(WAIT).unwrap(), Status::Failed { .. }));
        assert_eq!(server.requests.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn no_check_runs_by_itself_when_auto_check_is_off() {
        let dir = tempfile::tempdir().unwrap();
        let server = serve(vec![signed("0.3.0")]);
        let (updater, statuses) = start(options(&server, &dir, "0.2.0", false));
        assert!(statuses.recv_timeout(Duration::from_millis(200)).is_err());
        assert_eq!(server.requests.load(Ordering::SeqCst), 0);
        // The user can still check.
        updater.check_now();
        assert_eq!(statuses.recv_timeout(WAIT).unwrap(), Status::Checking);
        assert!(matches!(statuses.recv_timeout(WAIT).unwrap(), Status::Available { .. }));
        // And turn the checks on.
        updater.set_auto_check(true);
        assert!(matches!(statuses.recv_timeout(WAIT).unwrap(), Status::Available { .. }));
        assert_eq!(server.requests.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_manifest_of_a_newer_key_or_schema_is_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let other_key = serve(vec![(200, file(&payload("0.3.0"), "b", &key(2)))]);
        let (_u1, statuses) = start(options(&other_key, &dir, "0.2.0", true));
        assert_eq!(statuses.recv_timeout(WAIT).unwrap(), Status::Unreadable);
        let other_schema = serve(vec![(200, file(r#"{"schema":2}"#, "a", &key(1)))]);
        let (_u2, statuses) = start(options(&other_schema, &dir, "0.2.0", true));
        assert_eq!(statuses.recv_timeout(WAIT).unwrap(), Status::Unreadable);
        // Neither one counts as a version that Sayso saw.
        assert_eq!(state(&dir), UpdateState::default());
    }

    #[test]
    fn a_bad_signature_and_a_replay_fail() {
        let dir = tempfile::tempdir().unwrap();
        let forged = serve(vec![(200, file(&payload("0.9.0"), "a", &key(2)))]);
        let (updater, statuses) = start(options(&forged, &dir, "0.2.0", false));
        updater.check_now();
        assert_eq!(statuses.recv_timeout(WAIT).unwrap(), Status::Checking);
        assert!(matches!(statuses.recv_timeout(WAIT).unwrap(), Status::Failed { .. }));

        // The server offers 0.4.0, then goes back to 0.3.0.
        let replay = serve(vec![signed("0.4.0"), signed("0.3.0")]);
        let (updater, statuses) = start(options(&replay, &dir, "0.2.0", false));
        updater.check_now();
        assert_eq!(statuses.recv_timeout(WAIT).unwrap(), Status::Checking);
        assert!(matches!(statuses.recv_timeout(WAIT).unwrap(), Status::Available { version, .. } if version == Version::new(0, 4, 0)));
        updater.check_now();
        assert_eq!(statuses.recv_timeout(WAIT).unwrap(), Status::Checking);
        assert!(matches!(statuses.recv_timeout(WAIT).unwrap(), Status::Failed { message } if message.contains("0.3.0")));
        assert_eq!(state(&dir).highest_seen, Some(Version::new(0, 4, 0)));
    }

    /// A server with the manifest of 0.3.0 and its release file, and options
    /// with an installer on marker files.
    struct Install {
        dir: tempfile::TempDir,
        installer: Arc<FakeInstaller>,
        options: Options,
        _server: Server,
    }

    fn install_fixture(body: &[u8], automatic: bool) -> Install {
        let dir = tempfile::tempdir().unwrap();
        // The manifest names a file on the same test server, so the URL prefix is the server.
        let file_server = serve(vec![(200, body.to_vec())]);
        let prefix = file_server.url.replace("latest.json", "");
        let asset = crate::download::tests::asset_for(&format!("{prefix}Sayso-0.3.0-macos-arm64.dmg"), body);
        let payload = serde_json::json!({
            "schema": 1, "version": "0.3.0", "published": "2026-10-10T12:00:00Z",
            "notes_url": format!("{prefix}tag/v0.3.0"),
            "assets": { "macos-aarch64": asset },
        })
        .to_string();
        let server = serve(vec![(200, file(&payload, "a", &key(1)))]);
        let install_dir = dir.path().join("install");
        std::fs::create_dir(&install_dir).unwrap();
        let installer = Arc::new(FakeInstaller::new(&install_dir, "0.2.0"));
        let mut options = options(&server, &dir, "0.2.0", true);
        options.automatic = automatic;
        options.context.url_prefix = prefix;
        options.lock = Some(InstallLock::shared(&installer.lock_path()).unwrap().0);
        options.installer = Ok(installer.clone());
        // Keep the file server alive with the fixture.
        std::mem::forget(file_server);
        Install { dir, installer, options, _server: server }
    }

    fn until_settled(statuses: &crossbeam_channel::Receiver<Status>) -> Vec<Status> {
        let mut seen = Vec::new();
        loop {
            let status = statuses.recv_timeout(WAIT).unwrap();
            let done = matches!(status, Status::Ready { .. } | Status::Failed { .. });
            seen.push(status);
            if done {
                return seen;
            }
        }
    }

    #[test]
    fn update_now_downloads_stages_and_is_ready() {
        let Install { dir, installer, options, _server } = install_fixture(b"the release file", false);
        let record_path = installer.record_path();
        let (updater, statuses) = start(options);
        assert!(matches!(statuses.recv_timeout(WAIT).unwrap(), Status::Available { manual: None, .. }));
        // Nothing is downloaded until the user asks.
        assert!(statuses.recv_timeout(Duration::from_millis(150)).is_err());
        updater.install();
        let seen = until_settled(&statuses);
        assert!(matches!(seen.first(), Some(Status::Downloading { received: 0, total: 16, .. })), "{seen:?}");
        assert!(seen.iter().any(|s| matches!(s, Status::Preparing { .. })));
        assert_eq!(seen.last(), Some(&Status::Ready { version: Version::new(0, 3, 0) }));
        assert_eq!(installer.staged().as_deref(), Some("0.3.0"));
        assert_eq!(installer.installed(), "0.2.0");
        let record = Record::load(&record_path).unwrap();
        assert_eq!((record.phase, record.attempts), (Phase::Staged, 0));
        // The release file does not stay in the cache.
        assert_eq!(std::fs::read_dir(dir.path().join("downloads")).unwrap().count(), 0);

        // A later check knows that this version is staged.
        updater.check_now();
        assert_eq!(statuses.recv_timeout(WAIT).unwrap(), Status::Checking);
        assert_eq!(statuses.recv_timeout(WAIT).unwrap(), Status::Ready { version: Version::new(0, 3, 0) });

        // "Restart to update" puts it in place.
        assert_eq!(updater.swap(), Ok(()));
        assert_eq!(installer.installed(), "0.3.0");
    }

    #[test]
    fn automatic_mode_stages_without_a_request() {
        let Install { dir: _dir, installer, options, _server } = install_fixture(b"the release file", true);
        let (_updater, statuses) = start(options);
        let seen = until_settled(&statuses);
        assert!(matches!(seen.first(), Some(Status::Available { .. })));
        assert_eq!(seen.last(), Some(&Status::Ready { version: Version::new(0, 3, 0) }));
        assert_eq!(installer.staged().as_deref(), Some("0.3.0"));
    }

    #[test]
    fn a_release_file_that_does_not_stage_fails_and_leaves_nothing() {
        let Install { dir, installer, options, _server } = install_fixture(b"bad file", false);
        let (updater, statuses) = start(options);
        assert!(matches!(statuses.recv_timeout(WAIT).unwrap(), Status::Available { .. }));
        updater.install();
        let seen = until_settled(&statuses);
        assert!(matches!(seen.last(), Some(Status::Failed { message }) if message.contains("not a release of Sayso")), "{seen:?}");
        assert_eq!(installer.staged(), None);
        assert!(Record::load(&installer.record_path()).is_none());
        drop(dir);
        assert_eq!(updater.swap(), Err(SwapError::Failed("No update is ready to install.".into())));
    }

    #[test]
    fn a_failed_automatic_update_goes_back_to_available() {
        let Install { dir: _dir, installer: _installer, options, _server } = install_fixture(b"bad file", true);
        let (_updater, statuses) = start(options);
        let mut seen = Vec::new();
        while !matches!(seen.last(), Some(Status::Preparing { .. })) {
            seen.push(statuses.recv_timeout(WAIT).unwrap());
        }
        assert!(matches!(statuses.recv_timeout(WAIT).unwrap(), Status::Available { .. }));
    }

    /// A record of the last update, in the given phase, with the old version in the staging folder.
    fn leave_record(installer: &FakeInstaller, phase: Phase) {
        std::fs::write(installer.dir.join("staged"), "0.1.0").unwrap();
        Record {
            envelope: String::new(),
            from_version: Version::new(0, 1, 0),
            to_version: Version::new(0, 2, 0),
            install_path: installer.dir.clone(),
            phase,
            attempts: 1,
            error: Some("the exchange failed".into()),
        }
        .save(&installer.record_path())
        .unwrap();
    }

    #[test]
    fn automatic_mode_does_not_replace_an_update_that_is_not_confirmed_or_that_failed() {
        for phase in [Phase::Swapped, Phase::Failed] {
            let Install { dir: _dir, installer, options, _server } = install_fixture(b"the release file", true);
            leave_record(&installer, phase);
            let (updater, statuses) = start(options);
            // The check runs, and nothing follows: no status and no download.
            assert!(statuses.recv_timeout(Duration::from_millis(400)).is_err(), "{phase:?}");
            assert_eq!(installer.staged().as_deref(), Some("0.1.0"), "{phase:?}");
            assert_eq!(Record::load(&installer.record_path()).unwrap().phase, phase);
            // A check by the user forgets a failure, and the update runs again.
            // An update that is not confirmed needs "Update now".
            updater.check_now();
            assert_eq!(statuses.recv_timeout(WAIT).unwrap(), Status::Checking);
            assert!(matches!(statuses.recv_timeout(WAIT).unwrap(), Status::Available { .. }));
            if phase == Phase::Swapped {
                assert!(statuses.recv_timeout(Duration::from_millis(400)).is_err());
                assert_eq!(installer.staged().as_deref(), Some("0.1.0"));
                updater.install();
            }
            assert_eq!(until_settled(&statuses).last(), Some(&Status::Ready { version: Version::new(0, 3, 0) }));
            assert_eq!(installer.staged().as_deref(), Some("0.3.0"));
        }
    }
}
