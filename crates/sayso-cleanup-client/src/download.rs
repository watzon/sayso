//! Download of the model files. See `docs/cleanup-models.md`, "Download".
//!
//! A model lives in `<models dir>/cleanup/<id>/`. The model file streams into
//! `<file>.partial` and resumes with an HTTP `Range` request. After the last
//! byte the size and the SHA-256 hash are checked. Then the file gets its
//! final name and `ready.json` is written. A folder without a valid
//! `ready.json` is not ready, also when the model file is there.

use sayso_core::cleanup::CleanupModelInfo;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// The default server of the model files.
pub const DEFAULT_ENDPOINT: &str = "https://huggingface.co";

/// Attempts in a row without progress before a download fails.
const ATTEMPTS: u32 = 3;
/// Bytes per read. The cancel flag is checked between reads.
const CHUNK: usize = 64 * 1024;
/// Time that one request may take to read its body. A slow download takes
/// several requests, each resuming where the last one stopped. This also ends
/// a stalled connection.
const BODY_TIMEOUT: Duration = Duration::from_secs(60);
/// The shortest time between two progress calls.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(125);
/// The pause after a failed attempt, times the number of failures.
const RETRY_PAUSE: Duration = Duration::from_millis(500);
const READY: &str = "ready.json";

/// How far a download is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("The download was cancelled.")]
    Cancelled,
    #[error("This model is already downloading or being deleted.")]
    Busy,
    #[error("The server answered with status {status}.")]
    Http { status: u16 },
    #[error("The network failed: {0}")]
    Network(String),
    #[error("The file is not the expected file. Its hash is {got}, but it must be {expected}.")]
    WrongHash { expected: String, got: String },
    #[error("The file has {got} bytes, but it must have {expected}.")]
    WrongSize { expected: u64, got: u64 },
    #[error("Cannot use the model folder: {0}")]
    Io(#[from] std::io::Error),
}

/// `<models dir>/cleanup/<id>`.
pub fn model_dir(models_dir: &Path, model: &CleanupModelInfo) -> PathBuf {
    debug_assert!(names_are_plain(model), "the catalog names a path that is not a plain name");
    models_dir.join("cleanup").join(model.id)
}

/// True when `ready.json` in the model folder names the catalog revision and SHA-256 and the model file is there.
pub fn is_ready(models_dir: &Path, model: &CleanupModelInfo) -> bool {
    let dir = model_dir(models_dir, model);
    let Ok(bytes) = fs::read(dir.join(READY)) else {
        return false;
    };
    let Ok(ready) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    ready["revision"].as_str() == Some(model.revision)
        && ready["sha256"].as_str() == Some(model.sha256)
        && dir.join(model.file).is_file()
}

/// The model file when ready.
pub fn model_path(models_dir: &Path, model: &CleanupModelInfo) -> Option<PathBuf> {
    is_ready(models_dir, model).then(|| model_dir(models_dir, model).join(model.file))
}

/// Download the model file and its license files from the pinned revision.
///
/// `progress` gets about eight calls per second and one at the end. The
/// fraction never goes backwards. A cancelled download keeps its partial file
/// for a later resume.
pub fn download(
    models_dir: &Path,
    model: &CleanupModelInfo,
    endpoint: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf, DownloadError> {
    check_names(model)?;
    let dir = model_dir(models_dir, model);
    let _guard = Guard::acquire(&dir)?;
    let size = model.size_bytes;
    let path = dir.join(model.file);
    if is_ready(models_dir, model) {
        progress(Progress { bytes_done: size, bytes_total: size });
        return Ok(path);
    }
    fs::create_dir_all(&dir)?;

    let agent = agent();
    let url = |name: &str| {
        format!("{}/{}/resolve/{}/{name}", endpoint.trim_end_matches('/'), model.repo, model.revision)
    };
    for name in model.license_files {
        fetch_small(&agent, &url(name), &dir.join(name), cancel)?;
    }

    let partial = suffixed(&path, ".partial");
    let partial_info = suffixed(&path, ".partial.json");
    prepare_partial(&partial, &partial_info, model)?;
    let mut reporter = Reporter::new(size);
    fetch_model(&agent, &url(model.file), &partial, model, cancel, &mut reporter, progress)?;
    verify(&partial, &partial_info, model, cancel)?;
    mark_ready(&partial, &partial_info, &path, model)?;
    reporter.report(size, true, progress);
    Ok(path)
}

/// Delete the model folder. A model that is not on disk is Ok. Fails with Busy while a download of it runs.
pub fn delete(models_dir: &Path, model: &CleanupModelInfo) -> Result<(), DownloadError> {
    check_names(model)?;
    let dir = model_dir(models_dir, model);
    let _guard = Guard::acquire(&dir)?;
    // Windows can refuse for a moment while a file is still open (a model
    // being dropped, a virus scanner), so it tries again for about two seconds.
    let mut attempt = 0;
    loop {
        match fs::remove_dir_all(&dir) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) if cfg!(windows) && attempt < 20 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

// The model folders that a download or a delete works on right now.
static ACTIVE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Holds a model folder for one download or delete. Dropping it frees the folder.
struct Guard(PathBuf);

impl Guard {
    fn acquire(dir: &Path) -> Result<Guard, DownloadError> {
        let mut active = ACTIVE.lock();
        if active.iter().any(|d| d == dir) {
            return Err(DownloadError::Busy);
        }
        active.push(dir.to_path_buf());
        Ok(Guard(dir.to_path_buf()))
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        ACTIVE.lock().retain(|d| *d != self.0);
    }
}

/// True for a name of letters, digits, `.`, `_`, and `-` that is not `.` or `..`.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn names_are_plain(model: &CleanupModelInfo) -> bool {
    is_plain_name(model.id) && is_plain_name(model.file) && model.license_files.iter().all(|n| is_plain_name(n))
}

/// No name from the catalog may lead out of the model folder.
fn check_names(model: &CleanupModelInfo) -> Result<(), DownloadError> {
    if names_are_plain(model) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("the catalog names a path that is not a plain name ({})", model.id),
        )
        .into())
    }
}

/// `<path><suffix>` in the same folder.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .user_agent(concat!("Sayso/", env!("CARGO_PKG_VERSION")))
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .build();
    ureq::Agent::new_with_config(config)
}

/// The result of one attempt. A hard failure is an `Err` instead.
enum Attempt {
    Complete,
    /// Worth another attempt: a network error, a 5xx, a short body.
    Retry(DownloadError),
}

fn get(agent: &ureq::Agent, url: &str, from: u64) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    // The bytes of a Range request must be the bytes of the file.
    let mut request = agent
        .get(url)
        .config()
        .timeout_recv_body(Some(BODY_TIMEOUT))
        .build()
        .header("Accept-Encoding", "identity");
    if from > 0 {
        request = request.header("Range", format!("bytes={from}-"));
    }
    request.call()
}

/// Sleep, and end early with `Cancelled` when the flag is set.
fn pause(cancel: &AtomicBool, time: Duration) -> Result<(), DownloadError> {
    let end = Instant::now() + time;
    while Instant::now() < end {
        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn remove_file(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// A small file (a license file). No resume: a stale `.partial` is replaced.
fn fetch_small(agent: &ureq::Agent, url: &str, dest: &Path, cancel: &AtomicBool) -> Result<(), DownloadError> {
    let partial = suffixed(dest, ".partial");
    let mut failures = 0;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        match small_attempt(agent, url, &partial)? {
            Attempt::Complete => {
                fs::rename(&partial, dest)?;
                return Ok(());
            }
            Attempt::Retry(error) => {
                failures += 1;
                if failures >= ATTEMPTS {
                    remove_file(&partial)?;
                    return Err(error);
                }
                log::warn!("download of {url} stopped ({error}), trying again");
                pause(cancel, RETRY_PAUSE * failures)?;
            }
        }
    }
}

fn small_attempt(agent: &ureq::Agent, url: &str, partial: &Path) -> Result<Attempt, DownloadError> {
    let mut response = match get(agent, url, 0) {
        Ok(response) => response,
        Err(e) => return Ok(Attempt::Retry(DownloadError::Network(e.to_string()))),
    };
    match response.status().as_u16() {
        200 => {}
        status @ 500..=599 => return Ok(Attempt::Retry(DownloadError::Http { status })),
        status => return Err(DownloadError::Http { status }),
    }
    let mut file = File::create(partial)?;
    let mut reader = response.body_mut().as_reader();
    let mut buffer = vec![0u8; CHUNK];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => file.write_all(&buffer[..n])?,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Ok(Attempt::Retry(DownloadError::Network(e.to_string()))),
        }
    }
    file.sync_all()?;
    Ok(Attempt::Complete)
}

/// Delete a partial file that belongs to another revision or hash. Then write
/// the info file for this model.
fn prepare_partial(partial: &Path, info: &Path, model: &CleanupModelInfo) -> Result<(), DownloadError> {
    let wanted = json!({ "revision": model.revision, "sha256": model.sha256 });
    let current = fs::read(info).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    if current.as_ref() != Some(&wanted) {
        remove_file(partial)?;
        fs::write(info, serde_json::to_vec(&wanted).unwrap_or_default())?;
    }
    Ok(())
}

/// At most one progress call per interval, and the progress never goes
/// backwards. `force` skips the interval.
struct Reporter {
    total: u64,
    highest: u64,
    last: Option<Instant>,
}

impl Reporter {
    fn new(total: u64) -> Reporter {
        Reporter { total, highest: 0, last: None }
    }

    fn report(&mut self, done: u64, force: bool, progress: &mut dyn FnMut(Progress)) {
        self.highest = self.highest.max(done.min(self.total));
        let due = self.last.is_none_or(|last| last.elapsed() >= PROGRESS_INTERVAL);
        if force || due {
            self.last = Some(Instant::now());
            progress(Progress { bytes_done: self.highest, bytes_total: self.total });
        }
    }
}

/// Fetch the model file into `partial` until it is complete, or the attempts
/// are used up. Each attempt resumes the partial file.
fn fetch_model(
    agent: &ureq::Agent,
    url: &str,
    partial: &Path,
    model: &CleanupModelInfo,
    cancel: &AtomicBool,
    reporter: &mut Reporter,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), DownloadError> {
    // An attempt that brings the partial file past its longest length so far
    // is progress. A file that restarts and stops at the same place is not,
    // so a server with a short file cannot keep this loop alive.
    let mut best = file_len(partial);
    let mut failures = 0;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        match model_attempt(agent, url, partial, model.size_bytes, cancel, reporter, progress)? {
            Attempt::Complete => return Ok(()),
            Attempt::Retry(error) => {
                let now = file_len(partial);
                if now > best {
                    best = now;
                    failures = 0;
                } else {
                    failures += 1;
                }
                if failures >= ATTEMPTS {
                    return Err(error);
                }
                log::warn!("download of {} stopped ({error}), resuming", model.id);
                pause(cancel, RETRY_PAUSE * failures.max(1))?;
            }
        }
    }
}

fn model_attempt(
    agent: &ureq::Agent,
    url: &str,
    partial: &Path,
    size: u64,
    cancel: &AtomicBool,
    reporter: &mut Reporter,
    progress: &mut dyn FnMut(Progress),
) -> Result<Attempt, DownloadError> {
    let mut have = file_len(partial);
    if have > size {
        // More bytes than the file has: this partial file is never right.
        remove_file(partial)?;
        have = 0;
    }
    let mut response = match get(agent, url, have) {
        Ok(response) => response,
        Err(e) => return Ok(Attempt::Retry(DownloadError::Network(e.to_string()))),
    };
    let status = response.status().as_u16();
    let mut options = OpenOptions::new();
    options.create(true);
    match status {
        206 => {
            let range = response.headers().get("content-range").and_then(|v| v.to_str().ok()).unwrap_or("");
            if have == 0 || content_range_start(range) != Some(have) {
                // Start again without a Range header.
                remove_file(partial)?;
                return Ok(Attempt::Retry(DownloadError::Network(format!(
                    "the server resumed at the wrong place ({range:?})"
                ))));
            }
            options.append(true);
        }
        200 => {
            // The server sends the whole file again.
            have = 0;
            options.write(true).truncate(true);
        }
        416 if have == size && have > 0 => return Ok(Attempt::Complete),
        416 => {
            remove_file(partial)?;
            return Ok(Attempt::Retry(DownloadError::Http { status }));
        }
        500..=599 => return Ok(Attempt::Retry(DownloadError::Http { status })),
        _ => return Err(DownloadError::Http { status }),
    }
    let mut file = options.open(partial)?;
    reporter.report(have, false, progress);

    let mut reader = response.body_mut().as_reader();
    let mut buffer = vec![0u8; CHUNK];
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        let n = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Ok(Attempt::Retry(DownloadError::Network(e.to_string()))),
        };
        file.write_all(&buffer[..n])?;
        have += n as u64;
        if have > size {
            // Too long. The size check after this reports it.
            break;
        }
        reporter.report(have, false, progress);
    }
    file.sync_all()?;
    if have < size {
        return Ok(Attempt::Retry(DownloadError::Network(format!(
            "the connection closed after {have} of {size} bytes"
        ))));
    }
    Ok(Attempt::Complete)
}

/// `bytes 100-199/200` to `Some(100)`.
fn content_range_start(value: &str) -> Option<u64> {
    let range = value.trim().strip_prefix("bytes ")?;
    let (start, _) = range.split_once('-')?;
    start.trim().parse().ok()
}

/// Check the size and the hash of the partial file. A wrong file is deleted.
fn verify(partial: &Path, info: &Path, model: &CleanupModelInfo, cancel: &AtomicBool) -> Result<(), DownloadError> {
    let got = file_len(partial);
    if got != model.size_bytes {
        remove_file(partial)?;
        remove_file(info)?;
        return Err(DownloadError::WrongSize { expected: model.size_bytes, got });
    }
    let got = sha256_file(partial, cancel)?;
    if !got.eq_ignore_ascii_case(model.sha256) {
        remove_file(partial)?;
        remove_file(info)?;
        return Err(DownloadError::WrongHash { expected: model.sha256.to_string(), got });
    }
    Ok(())
}

fn sha256_file(path: &Path, cancel: &AtomicBool) -> Result<String, DownloadError> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        let n = file.read(&mut buffer)?;
        if n == 0 {
            return Ok(hex(&hash.finalize()));
        }
        hash.update(&buffer[..n]);
    }
}

/// Ready is one step in the eyes of `is_ready`: the file gets its name, then
/// `ready.json` appears through a rename.
fn mark_ready(partial: &Path, info: &Path, path: &Path, model: &CleanupModelInfo) -> Result<(), DownloadError> {
    fs::rename(partial, path)?;
    remove_file(info)?;
    let ready = json!({ "revision": model.revision, "sha256": model.sha256, "file": model.file });
    let ready_path = path.with_file_name(READY);
    let temp = suffixed(&ready_path, ".tmp");
    let mut file = File::create(&temp)?;
    file.write_all(&serde_json::to_vec(&ready).unwrap_or_default())?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp, &ready_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sayso_core::cleanup::PromptFormat;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    const FILE_PATH: &str = "/test/repo/resolve/rev1/test.gguf";

    fn body(len: usize) -> Vec<u8> {
        (0..len).map(|i| ((i * 31 + i / 7) % 251) as u8).collect()
    }

    fn model_for(id: &'static str, body: &[u8], license_files: &'static [&'static str]) -> CleanupModelInfo {
        let sha256: &'static str = Box::leak(hex(&Sha256::digest(body)).into_boxed_str());
        CleanupModelInfo {
            id,
            name: "Test",
            publisher: "Test",
            repo: "test/repo",
            revision: "rev1",
            file: "test.gguf",
            size_bytes: body.len() as u64,
            sha256,
            license_files,
            languages: &["en"],
            tones: false,
            layouts: &[],
            formats: &[],
            context: false,
            prompt: PromptFormat::KeyValue,
        }
    }

    /// The server holds the body at `offset` until the gate opens.
    type Stop = (usize, Arc<AtomicBool>);

    /// A body that waits at its stops.
    struct Gated {
        data: Vec<u8>,
        pos: usize,
        stops: Vec<Stop>,
    }

    impl Read for Gated {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut end = self.data.len();
            for (offset, gate) in &self.stops {
                if self.pos == *offset {
                    let start = Instant::now();
                    while !gate.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(10) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                } else if *offset > self.pos && !gate.load(Ordering::SeqCst) {
                    end = end.min(*offset);
                }
            }
            let n = buf.len().min(end - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[derive(Debug, Clone)]
    struct Seen {
        path: String,
        range: Option<String>,
        sent: usize,
    }

    #[derive(Default)]
    struct Config {
        files: Mutex<HashMap<String, Vec<u8>>>,
        honor_range: AtomicBool,
        /// Statuses to answer with before the first good answer.
        fail_with: Mutex<Vec<u16>>,
        /// Gates for the first answer of the model file.
        stops: Mutex<Vec<Stop>>,
        seen: Mutex<Vec<Seen>>,
        count: AtomicUsize,
    }

    struct Server {
        url: String,
        config: Arc<Config>,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Server {
        fn start(files: Vec<(String, Vec<u8>)>) -> Server {
            let config = Arc::new(Config { honor_range: AtomicBool::new(true), ..Config::default() });
            config.files.lock().extend(files);
            let http = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let port = http.server_addr().to_ip().unwrap().port();
            let stop = Arc::new(AtomicBool::new(false));
            let thread = {
                let (config, stop) = (config.clone(), stop.clone());
                std::thread::spawn(move || {
                    while !stop.load(Ordering::SeqCst) {
                        if let Ok(Some(request)) = http.recv_timeout(Duration::from_millis(20)) {
                            answer(&config, request);
                        }
                    }
                })
            };
            Server { url: format!("http://127.0.0.1:{port}"), config, stop, thread: Some(thread) }
        }

        fn seen(&self) -> Vec<Seen> {
            self.config.seen.lock().clone()
        }

        fn wait_for_requests(&self, count: usize) {
            let start = Instant::now();
            while self.config.count.load(Ordering::SeqCst) < count {
                assert!(start.elapsed() < Duration::from_secs(10), "the server got no request");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn header(name: &str, value: &str) -> tiny_http::Header {
        tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
    }

    fn answer(config: &Config, request: tiny_http::Request) {
        let path = request.url().to_string();
        let range = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Range"))
            .map(|h| h.value.as_str().to_string());
        let mut seen = Seen { path: path.clone(), range: range.clone(), sent: 0 };
        let status_only = |status: u16| tiny_http::Response::from_data(Vec::new()).with_status_code(status);

        let fail = {
            let mut fail = config.fail_with.lock();
            (!fail.is_empty()).then(|| fail.remove(0))
        };
        let file = config.files.lock().get(&path).cloned();
        let result = match (fail, file) {
            (Some(status), _) => {
                config.seen.lock().push(seen);
                config.count.fetch_add(1, Ordering::SeqCst);
                request.respond(status_only(status))
            }
            (None, None) => {
                config.seen.lock().push(seen);
                config.count.fetch_add(1, Ordering::SeqCst);
                request.respond(status_only(404))
            }
            (None, Some(file)) => {
                let from = range
                    .as_deref()
                    .and_then(|r| r.strip_prefix("bytes="))
                    .and_then(|r| r.strip_suffix('-'))
                    .and_then(|r| r.parse::<usize>().ok())
                    .filter(|_| config.honor_range.load(Ordering::SeqCst));
                let stops = if path == FILE_PATH { std::mem::take(&mut *config.stops.lock()) } else { Vec::new() };
                let (status, data, mut headers) = match from {
                    Some(from) if from >= file.len() => {
                        config.seen.lock().push(seen);
                        config.count.fetch_add(1, Ordering::SeqCst);
                        let _ = request.respond(status_only(416));
                        return;
                    }
                    Some(from) => {
                        let value = format!("bytes {from}-{}/{}", file.len() - 1, file.len());
                        (206, file[from..].to_vec(), vec![header("Content-Range", &value)])
                    }
                    None => (200, file, Vec::new()),
                };
                headers.push(header("Content-Type", "application/octet-stream"));
                seen.sent = data.len();
                config.seen.lock().push(seen);
                config.count.fetch_add(1, Ordering::SeqCst);
                let len = data.len();
                let reader = Gated { data, pos: 0, stops };
                request.respond(tiny_http::Response::new(
                    tiny_http::StatusCode(status),
                    headers,
                    reader,
                    Some(len),
                    None,
                ))
            }
        };
        let _ = result;
    }

    fn serve_model(data: &[u8]) -> Server {
        Server::start(vec![(FILE_PATH.to_string(), data.to_vec())])
    }

    fn run(
        dir: &Path,
        model: &CleanupModelInfo,
        server: &Server,
        cancel: &AtomicBool,
    ) -> (Result<PathBuf, DownloadError>, Vec<Progress>) {
        let mut calls = Vec::new();
        let result = download(dir, model, &server.url, cancel, &mut |p| calls.push(p));
        (result, calls)
    }

    fn no_cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn write_partial(dir: &Path, model: &CleanupModelInfo, bytes: &[u8], revision: &str) {
        let folder = model_dir(dir, model);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("test.gguf.partial"), bytes).unwrap();
        let info = json!({ "revision": revision, "sha256": model.sha256 });
        fs::write(folder.join("test.gguf.partial.json"), info.to_string()).unwrap();
    }

    #[test]
    fn fresh_download_is_ready() {
        let data = body(200_000);
        let model = model_for("fresh", &data, &[]);
        let server = serve_model(&data);
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_ready(dir.path(), &model));
        assert!(model_path(dir.path(), &model).is_none());

        let (result, calls) = run(dir.path(), &model, &server, &no_cancel());
        let path = result.unwrap();
        assert_eq!(path, model_dir(dir.path(), &model).join("test.gguf"));
        assert_eq!(fs::read(&path).unwrap(), data);
        assert!(is_ready(dir.path(), &model));
        assert_eq!(model_path(dir.path(), &model), Some(path));
        let folder = model_dir(dir.path(), &model);
        assert!(!folder.join("test.gguf.partial").exists());
        assert!(!folder.join("test.gguf.partial.json").exists());
        assert!(!folder.join("ready.json.tmp").exists());

        let ready: Value = serde_json::from_slice(&fs::read(folder.join("ready.json")).unwrap()).unwrap();
        assert_eq!(ready["revision"], "rev1");
        assert_eq!(ready["sha256"], model.sha256);
        assert_eq!(ready["file"], "test.gguf");

        // The fraction goes up only, ends at 100 percent, and is not flooded.
        let total = data.len() as u64;
        assert!(calls.iter().all(|p| p.bytes_total == total));
        assert!(calls.windows(2).all(|w| w[0].bytes_done <= w[1].bytes_done));
        assert_eq!(calls.last(), Some(&Progress { bytes_done: total, bytes_total: total }));
        assert!(calls.len() <= 4, "{} progress calls for a fast download", calls.len());
    }

    #[test]
    fn a_ready_model_returns_at_once() {
        let data = body(5_000);
        let model = model_for("again", &data, &[]);
        let server = serve_model(&data);
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &model, &server, &no_cancel()).0.unwrap();
        let before = server.seen().len();

        let (result, calls) = run(dir.path(), &model, &server, &no_cancel());
        result.unwrap();
        assert_eq!(server.seen().len(), before);
        assert_eq!(calls, vec![Progress { bytes_done: 5_000, bytes_total: 5_000 }]);
    }

    #[test]
    fn a_wrong_hash_deletes_the_file() {
        let data = body(10_000);
        let mut model = model_for("hash", &body(10_001), &[]);
        model.size_bytes = data.len() as u64;
        let server = serve_model(&data);
        let dir = tempfile::tempdir().unwrap();

        let (result, _) = run(dir.path(), &model, &server, &no_cancel());
        match result {
            Err(DownloadError::WrongHash { expected, got }) => {
                assert_eq!(expected, model.sha256);
                assert_eq!(got, hex(&Sha256::digest(&data)));
            }
            other => panic!("expected WrongHash, got {other:?}"),
        }
        let folder = model_dir(dir.path(), &model);
        assert!(!folder.join("test.gguf").exists());
        assert!(!folder.join("test.gguf.partial").exists());
        assert!(!folder.join("test.gguf.partial.json").exists());
        assert!(!is_ready(dir.path(), &model));
    }

    #[test]
    fn a_wrong_size_deletes_the_file() {
        let data = body(200_000);
        let mut model = model_for("size", &data, &[]);
        model.size_bytes = 150_000;
        let server = serve_model(&data);
        let dir = tempfile::tempdir().unwrap();

        let (result, _) = run(dir.path(), &model, &server, &no_cancel());
        match result {
            Err(DownloadError::WrongSize { expected, got }) => {
                assert_eq!(expected, 150_000);
                assert!(got > 150_000);
            }
            other => panic!("expected WrongSize, got {other:?}"),
        }
        let folder = model_dir(dir.path(), &model);
        assert!(!folder.join("test.gguf").exists());
        assert!(!folder.join("test.gguf.partial").exists());
        assert!(!is_ready(dir.path(), &model));
    }

    #[test]
    fn a_partial_file_resumes_with_a_range_request() {
        let data = body(200_000);
        let model = model_for("resume", &data, &[]);
        let server = serve_model(&data);
        let dir = tempfile::tempdir().unwrap();
        write_partial(dir.path(), &model, &data[..100_000], "rev1");

        let (result, _) = run(dir.path(), &model, &server, &no_cancel());
        assert_eq!(fs::read(result.unwrap()).unwrap(), data);
        let seen = server.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].range.as_deref(), Some("bytes=100000-"));
        assert_eq!(seen[0].sent, 100_000, "only the rest is sent");
    }

    #[test]
    fn a_server_that_ignores_range_starts_again() {
        let data = body(200_000);
        let model = model_for("norange", &data, &[]);
        let server = serve_model(&data);
        server.config.honor_range.store(false, Ordering::SeqCst);
        let dir = tempfile::tempdir().unwrap();
        write_partial(dir.path(), &model, &data[..100_000], "rev1");

        let (result, _) = run(dir.path(), &model, &server, &no_cancel());
        assert_eq!(fs::read(result.unwrap()).unwrap(), data);
        let seen = server.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].range.as_deref(), Some("bytes=100000-"));
        assert_eq!(seen[0].sent, data.len());
        assert!(is_ready(dir.path(), &model));
    }

    #[test]
    fn a_complete_partial_file_needs_no_bytes() {
        let data = body(20_000);
        let model = model_for("complete", &data, &[]);
        let server = serve_model(&data);
        let dir = tempfile::tempdir().unwrap();
        write_partial(dir.path(), &model, &data, "rev1");

        let (result, _) = run(dir.path(), &model, &server, &no_cancel());
        assert_eq!(fs::read(result.unwrap()).unwrap(), data);
        assert_eq!(server.seen()[0].sent, 0);
    }

    #[test]
    fn a_partial_file_of_another_revision_is_deleted() {
        let data = body(50_000);
        let model = model_for("stale", &data, &[]);
        let server = serve_model(&data);
        let dir = tempfile::tempdir().unwrap();
        write_partial(dir.path(), &model, &vec![7u8; 20_000], "rev0");

        let (result, _) = run(dir.path(), &model, &server, &no_cancel());
        assert_eq!(fs::read(result.unwrap()).unwrap(), data);
        let seen = server.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].range, None);
    }

    #[test]
    fn a_partial_file_without_info_is_deleted() {
        let data = body(50_000);
        let model = model_for("noinfo", &data, &[]);
        let server = serve_model(&data);
        let dir = tempfile::tempdir().unwrap();
        let folder = model_dir(dir.path(), &model);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("test.gguf.partial"), vec![7u8; 20_000]).unwrap();

        let (result, _) = run(dir.path(), &model, &server, &no_cancel());
        assert_eq!(fs::read(result.unwrap()).unwrap(), data);
        assert_eq!(server.seen()[0].range, None);
    }

    #[test]
    fn cancel_keeps_the_partial_file_and_the_next_call_resumes() {
        let data = body(200_000);
        let model = model_for("cancel", &data, &[]);
        let server = serve_model(&data);
        let first = Arc::new(AtomicBool::new(false));
        let second = Arc::new(AtomicBool::new(false));
        *server.config.stops.lock() = vec![(50_000, first.clone()), (100_000, second.clone())];
        let dir = tempfile::tempdir().unwrap();
        let cancel = no_cancel();
        let partial = model_dir(dir.path(), &model).join("test.gguf.partial");

        let (result, _) = std::thread::scope(|scope| {
            let handle = scope.spawn(|| run(dir.path(), &model, &server, &cancel));
            let start = Instant::now();
            // The server buffers the last bytes before a stop, so wait for less.
            while file_len(&partial) < 30_000 {
                assert!(start.elapsed() < Duration::from_secs(10), "the download did not start");
                std::thread::sleep(Duration::from_millis(5));
            }
            cancel.store(true, Ordering::SeqCst);
            first.store(true, Ordering::SeqCst);
            let result = handle.join().unwrap();
            second.store(true, Ordering::SeqCst);
            result
        });
        assert!(matches!(result, Err(DownloadError::Cancelled)), "{result:?}");
        let kept = file_len(&partial);
        assert!((30_000..100_000).contains(&kept), "kept {kept} bytes");
        assert!(!is_ready(dir.path(), &model));

        let (result, _) = run(dir.path(), &model, &server, &no_cancel());
        assert_eq!(fs::read(result.unwrap()).unwrap(), data);
        let seen = server.seen();
        let last = seen.last().unwrap();
        assert_eq!(last.range, Some(format!("bytes={kept}-")));
        assert_eq!(last.sent, data.len() - kept as usize);
    }

    #[test]
    fn a_second_download_of_the_same_model_is_busy() {
        let data = body(100_000);
        let model = model_for("busy", &data, &[]);
        let server = serve_model(&data);
        let gate = Arc::new(AtomicBool::new(false));
        *server.config.stops.lock() = vec![(0, gate.clone())];
        let dir = tempfile::tempdir().unwrap();
        let cancel = no_cancel();

        std::thread::scope(|scope| {
            let handle = scope.spawn(|| run(dir.path(), &model, &server, &cancel));
            server.wait_for_requests(1);
            let (second, calls) = run(dir.path(), &model, &server, &no_cancel());
            assert!(matches!(second, Err(DownloadError::Busy)), "{second:?}");
            assert!(calls.is_empty());
            assert!(matches!(delete(dir.path(), &model), Err(DownloadError::Busy)));
            gate.store(true, Ordering::SeqCst);
            handle.join().unwrap().0.unwrap();
        });
        // The guard is gone after the download.
        delete(dir.path(), &model).unwrap();
    }

    #[test]
    fn a_missing_file_fails_at_once() {
        let data = body(1_000);
        let model = model_for("notfound", &data, &[]);
        let server = Server::start(Vec::new());
        let dir = tempfile::tempdir().unwrap();

        let (result, _) = run(dir.path(), &model, &server, &no_cancel());
        assert!(matches!(result, Err(DownloadError::Http { status: 404 })), "{result:?}");
        assert_eq!(server.seen().len(), 1);
        assert!(!is_ready(dir.path(), &model));
    }

    #[test]
    fn a_server_error_is_tried_again() {
        let data = body(30_000);
        let model = model_for("retry", &data, &[]);
        let server = serve_model(&data);
        *server.config.fail_with.lock() = vec![500];
        let dir = tempfile::tempdir().unwrap();

        let (result, _) = run(dir.path(), &model, &server, &no_cancel());
        assert_eq!(fs::read(result.unwrap()).unwrap(), data);
        assert_eq!(server.seen().len(), 2);
    }

    #[test]
    fn three_attempts_without_progress_fail() {
        let data = body(1_000);
        let model = model_for("giveup", &data, &[]);
        let server = serve_model(&data);
        *server.config.fail_with.lock() = vec![503; 10];
        let dir = tempfile::tempdir().unwrap();

        let (result, _) = run(dir.path(), &model, &server, &no_cancel());
        assert!(matches!(result, Err(DownloadError::Http { status: 503 })), "{result:?}");
        assert_eq!(server.seen().len(), 3);
    }

    #[test]
    fn delete_removes_the_folder_and_a_missing_folder_is_ok() {
        let data = body(5_000);
        let model = model_for("delete", &data, &[]);
        let server = serve_model(&data);
        let dir = tempfile::tempdir().unwrap();
        delete(dir.path(), &model).unwrap();

        run(dir.path(), &model, &server, &no_cancel()).0.unwrap();
        assert!(is_ready(dir.path(), &model));
        delete(dir.path(), &model).unwrap();
        assert!(!model_dir(dir.path(), &model).exists());
        assert!(!is_ready(dir.path(), &model));
        delete(dir.path(), &model).unwrap();
    }

    #[test]
    fn without_ready_json_the_model_is_not_ready() {
        let data = body(5_000);
        let model = model_for("noready", &data, &[]);
        let server = serve_model(&data);
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &model, &server, &no_cancel()).0.unwrap();
        let folder = model_dir(dir.path(), &model);

        fs::remove_file(folder.join("ready.json")).unwrap();
        assert!(folder.join("test.gguf").is_file());
        assert!(!is_ready(dir.path(), &model));
        assert!(model_path(dir.path(), &model).is_none());

        // Another revision or hash is not ready either.
        let other = json!({ "revision": "rev0", "sha256": model.sha256, "file": "test.gguf" });
        fs::write(folder.join("ready.json"), other.to_string()).unwrap();
        assert!(!is_ready(dir.path(), &model));
        fs::write(folder.join("ready.json"), "not json").unwrap();
        assert!(!is_ready(dir.path(), &model));

        // A new download makes the model ready again.
        run(dir.path(), &model, &server, &no_cancel()).0.unwrap();
        assert!(is_ready(dir.path(), &model));
    }

    #[test]
    fn license_files_are_downloaded_first() {
        let data = body(5_000);
        let model = model_for("license", &data, &["LICENSE", "NOTICE"]);
        let server = Server::start(vec![
            (FILE_PATH.to_string(), data.clone()),
            ("/test/repo/resolve/rev1/LICENSE".to_string(), b"license text".to_vec()),
            ("/test/repo/resolve/rev1/NOTICE".to_string(), b"notice text".to_vec()),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let folder = model_dir(dir.path(), &model);
        // A stale partial license file is replaced.
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("LICENSE.partial"), "old").unwrap();

        run(dir.path(), &model, &server, &no_cancel()).0.unwrap();
        assert_eq!(fs::read_to_string(folder.join("LICENSE")).unwrap(), "license text");
        assert_eq!(fs::read_to_string(folder.join("NOTICE")).unwrap(), "notice text");
        assert!(!folder.join("LICENSE.partial").exists());
        let paths: Vec<String> = server.seen().into_iter().map(|s| s.path).collect();
        assert_eq!(
            paths,
            ["/test/repo/resolve/rev1/LICENSE", "/test/repo/resolve/rev1/NOTICE", FILE_PATH]
        );
    }

    #[test]
    fn a_missing_license_file_stops_before_the_model_file() {
        let data = body(5_000);
        let model = model_for("nolicense", &data, &["LICENSE"]);
        let server = serve_model(&data);
        let dir = tempfile::tempdir().unwrap();

        let (result, _) = run(dir.path(), &model, &server, &no_cancel());
        assert!(matches!(result, Err(DownloadError::Http { status: 404 })), "{result:?}");
        assert!(!is_ready(dir.path(), &model));
        assert!(server.seen().iter().all(|s| s.path != FILE_PATH));
    }

    #[test]
    fn a_path_that_leaves_the_folder_is_refused() {
        let data = body(100);
        let mut model = model_for("escape", &data, &[]);
        model.file = "../escape.gguf";
        let dir = tempfile::tempdir().unwrap();
        let server = Server::start(Vec::new());
        let (result, _) = run(dir.path(), &model, &server, &no_cancel());
        assert!(matches!(result, Err(DownloadError::Io(_))), "{result:?}");
        assert!(matches!(delete(dir.path(), &model), Err(DownloadError::Io(_))));
        assert!(server.seen().is_empty());
    }

    #[test]
    fn plain_names() {
        assert!(is_plain_name("s1-mini"));
        assert!(is_plain_name("cleanup-1-q8_0.gguf"));
        assert!(!is_plain_name(""));
        assert!(!is_plain_name(".."));
        assert!(!is_plain_name("a/b"));
        assert!(!is_plain_name("a\\b"));
    }

    #[test]
    fn content_range_start_parses() {
        assert_eq!(content_range_start("bytes 100-199/200"), Some(100));
        assert_eq!(content_range_start("bytes 5-9/*"), Some(5));
        assert_eq!(content_range_start("nonsense"), None);
        assert_eq!(content_range_start(""), None);
    }
}
