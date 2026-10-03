//! Model downloads over HTTPS (ureq, rustls).
//!
//! Each file streams into `<file>.partial` and is renamed when its size is
//! right. A partial file from an earlier run resumes with an HTTP `Range`
//! request. A file that already has its final name is complete and is skipped
//! without a request.

use crate::protocol::Result;
use crate::store::{PARTIAL_SUFFIX, RemoteFile};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Failed attempts in a row, without progress, before a file fails. Each
/// retry resumes the partial file.
const ATTEMPTS: u32 = 3;

/// Progress of a whole model download.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    pub fraction: f64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// At most one progress message per `min_interval`, and the fraction never
/// goes backwards. `force` skips the interval (first and last message).
#[derive(Debug)]
pub struct Throttle {
    min_interval: Duration,
    last: Option<Instant>,
    highest: f64,
}

impl Throttle {
    /// Eight messages per second, the protocol limit.
    pub fn protocol() -> Throttle {
        Throttle::new(Duration::from_millis(125))
    }

    pub fn new(min_interval: Duration) -> Throttle {
        Throttle {
            min_interval,
            last: None,
            highest: 0.0,
        }
    }

    pub fn check(&mut self, done: u64, total: u64, now: Instant, force: bool) -> Option<Progress> {
        let fraction = if total == 0 {
            0.0
        } else {
            (done as f64 / total as f64).clamp(0.0, 1.0)
        };
        self.highest = self.highest.max(fraction);
        let due = self
            .last
            .is_none_or(|last| now.duration_since(last) >= self.min_interval);
        if !force && !due {
            return None;
        }
        self.last = Some(now);
        Some(Progress {
            fraction: self.highest,
            bytes_done: done,
            bytes_total: total,
        })
    }
}

enum FetchError {
    /// Worth another attempt: network errors, 5xx, a short body.
    Retry(String),
    Fatal(String),
}

pub struct Downloader {
    agent: ureq::Agent,
    /// Longest time one request reads its body. A slow download takes several
    /// requests, each resuming where the last one stopped.
    request_time: Duration,
}

impl Default for Downloader {
    fn default() -> Self {
        Downloader::new()
    }
}

impl Downloader {
    pub fn new() -> Downloader {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .user_agent(format!("SaysoEngine/{}", crate::protocol::ENGINE_VERSION))
            .build();
        Downloader {
            agent: ureq::Agent::new_with_config(config),
            request_time: Duration::from_secs(120),
        }
    }

    /// A shorter request time, for tests.
    pub fn with_request_time(mut self, request_time: Duration) -> Downloader {
        self.request_time = request_time;
        self
    }

    /// Fetch every missing file of a model into `folder`. `progress` gets
    /// `(bytes_done, bytes_total)` for the whole model, often; throttle it.
    /// `bytes_total` is the sum of the `Content-Length` of every file, or
    /// `size_hint` while a length is unknown.
    pub fn fetch_all(
        &self,
        files: &[RemoteFile],
        folder: &Path,
        size_hint: Option<u64>,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<()> {
        fs::create_dir_all(folder)
            .map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
        // Sizes first, so the total is right from the first message.
        let mut sizes: Vec<Option<u64>> = Vec::with_capacity(files.len());
        let mut complete: Vec<bool> = Vec::with_capacity(files.len());
        for file in files {
            let path = folder.join(&file.name);
            if let Some(meta) = fs::metadata(&path).ok().filter(|m| m.is_file()) {
                sizes.push(Some(meta.len()));
                complete.push(true);
            } else {
                sizes.push(self.remote_size(&file.url));
                complete.push(false);
            }
        }
        let total = |sizes: &[Option<u64>]| {
            let known: u64 = sizes.iter().flatten().sum();
            if sizes.iter().all(Option::is_some) {
                known
            } else {
                known.max(size_hint.unwrap_or(0))
            }
        };

        let mut done: u64 = sizes
            .iter()
            .zip(&complete)
            .filter(|(_, c)| **c)
            .filter_map(|(s, _)| *s)
            .sum();
        progress(done, total(&sizes));
        for (index, file) in files.iter().enumerate() {
            if complete[index] {
                continue;
            }
            let path = folder.join(&file.name);
            let partial = partial_path(&path);
            let mut expected = sizes[index];
            let base = done;
            // A failed attempt that moved the partial file forward does not
            // count, so a slow but working connection always gets there.
            let mut failures = 0;
            let mut best = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
            let size = loop {
                let result =
                    self.fetch_one(&file.url, &path, &mut expected, &mut |bytes, expected| {
                        let mut current = sizes.clone();
                        current[index] = current[index].or(expected);
                        progress(base + bytes, total(&current).max(base + bytes));
                    });
                if sizes[index].is_none() {
                    sizes[index] = expected;
                }
                match result {
                    Ok(size) => break size,
                    Err(FetchError::Retry(message)) => {
                        let now = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
                        if now > best {
                            best = now;
                            failures = 0;
                        } else {
                            failures += 1;
                        }
                        if failures >= ATTEMPTS {
                            return Err(format!("download of {} failed: {message}", file.name));
                        }
                        log::warn!("download of {} stopped ({message}), resuming", file.name);
                        std::thread::sleep(Duration::from_millis(500 * u64::from(failures)));
                    }
                    Err(FetchError::Fatal(message)) => {
                        return Err(format!("download of {} failed: {message}", file.name));
                    }
                }
            };
            sizes[index] = Some(size);
            done = base + size;
            progress(done, total(&sizes));
        }
        Ok(())
    }

    /// `Content-Length` from a HEAD request, following redirects. None on any error.
    fn remote_size(&self, url: &str) -> Option<u64> {
        let response = self.agent.head(url).call().ok()?;
        if !response.status().is_success() {
            return None;
        }
        header_u64(response.headers(), "content-length")
    }

    /// One attempt at one file. `expected` is the full size when known; a
    /// response can fill it in. Returns the final size.
    fn fetch_one(
        &self,
        url: &str,
        path: &Path,
        expected: &mut Option<u64>,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> std::result::Result<u64, FetchError> {
        let partial = partial_path(path);
        let mut have = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
        if let Some(size) = *expected {
            if have == size && have > 0 {
                return finish(&partial, path, have);
            }
            if have > size {
                remove(&partial);
                have = 0;
            }
        }

        // Each request reads for at most `request_time`, then the next one
        // resumes. This also ends a stalled connection.
        let mut request = self
            .agent
            .get(url)
            .config()
            .timeout_recv_body(Some(self.request_time))
            .build();
        if have > 0 {
            request = request.header("Range", format!("bytes={have}-"));
        }
        let mut response = request
            .call()
            .map_err(|e| FetchError::Retry(e.to_string()))?;
        let status = response.status().as_u16();
        let mut file = match status {
            206 => {
                let range = response
                    .headers()
                    .get("content-range")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                let (start, full) = parse_content_range(range);
                if start != Some(have) {
                    remove(&partial);
                    return Err(FetchError::Retry(format!(
                        "the server resumed at the wrong place ({range:?})"
                    )));
                }
                if expected.is_none() {
                    *expected = full;
                }
                open(&partial, true)?
            }
            200 => {
                have = 0;
                if expected.is_none() {
                    *expected = response.body().content_length();
                }
                open(&partial, false)?
            }
            416 => {
                remove(&partial);
                return Err(FetchError::Retry("the server refused to resume".into()));
            }
            500..=599 | 429 => return Err(FetchError::Retry(format!("HTTP {status}"))),
            _ => return Err(FetchError::Fatal(format!("HTTP {status} for {url}"))),
        };

        progress(have, *expected);
        let mut reader = response.body_mut().as_reader();
        let mut buffer = vec![0u8; 256 * 1024];
        loop {
            let n = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(FetchError::Retry(e.to_string())),
            };
            file.write_all(&buffer[..n]).map_err(|e| {
                FetchError::Fatal(format!("cannot write {}: {e}", partial.display()))
            })?;
            have += n as u64;
            progress(have, *expected);
        }
        file.flush()
            .and_then(|()| file.sync_all())
            .map_err(|e| FetchError::Fatal(format!("cannot write {}: {e}", partial.display())))?;
        drop(file);

        match *expected {
            Some(size) if have < size => Err(FetchError::Retry(format!(
                "the connection closed after {have} of {size} bytes"
            ))),
            Some(size) if have > size => {
                remove(&partial);
                Err(FetchError::Retry(format!(
                    "got {have} bytes, expected {size}"
                )))
            }
            _ => finish(&partial, path, have),
        }
    }
}

/// `<file>.partial` beside `<file>`.
pub fn partial_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(PARTIAL_SUFFIX);
    path.with_file_name(name)
}

fn finish(partial: &Path, path: &Path, size: u64) -> std::result::Result<u64, FetchError> {
    fs::rename(partial, path)
        .map(|()| size)
        .map_err(|e| FetchError::Fatal(format!("cannot rename {}: {e}", partial.display())))
}

fn open(path: &Path, append: bool) -> std::result::Result<File, FetchError> {
    let mut options = OpenOptions::new();
    options.create(true);
    if append {
        options.append(true);
    } else {
        options.write(true).truncate(true);
    }
    options
        .open(path)
        .map_err(|e| FetchError::Fatal(format!("cannot write {}: {e}", path.display())))
}

fn remove(path: &Path) {
    let _ = fs::remove_file(path);
}

fn header_u64(headers: &ureq::http::HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// `bytes 100-199/200` to `(Some(100), Some(200))`.
fn parse_content_range(value: &str) -> (Option<u64>, Option<u64>) {
    let Some(rest) = value.trim().strip_prefix("bytes ") else {
        return (None, None);
    };
    let (range, full) = rest.split_once('/').unwrap_or((rest, "*"));
    let start = range
        .split_once('-')
        .and_then(|(s, _)| s.trim().parse().ok());
    (start, full.trim().parse().ok())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn throttle_allows_eight_per_second_and_never_goes_back() {
        let mut throttle = Throttle::protocol();
        let start = Instant::now();
        let mut sent = 0;
        // 1000 updates over one simulated second.
        for i in 0..1000u64 {
            let now = start + Duration::from_millis(i);
            if throttle.check(i, 1000, now, false).is_some() {
                sent += 1;
            }
        }
        assert!(sent <= 8, "{sent} messages in one second");
        assert!(sent >= 7);
        let forced = throttle
            .check(1000, 1000, start + Duration::from_millis(1000), true)
            .unwrap();
        assert_eq!(forced.fraction, 1.0);
        let later = throttle
            .check(10, 1000, start + Duration::from_secs(5), true)
            .unwrap();
        assert_eq!(later.fraction, 1.0, "the fraction never goes backwards");
        assert_eq!(throttle.check(0, 0, start, true).unwrap().fraction, 1.0);
    }

    #[test]
    fn content_range() {
        assert_eq!(
            parse_content_range("bytes 100-199/200"),
            (Some(100), Some(200))
        );
        assert_eq!(parse_content_range("bytes 5-9/*"), (Some(5), None));
        assert_eq!(parse_content_range("nonsense"), (None, None));
    }

    #[test]
    fn partial_name() {
        assert_eq!(
            partial_path(Path::new("a/b/vocab.txt")),
            Path::new("a/b/vocab.txt.partial")
        );
    }

    /// A small file server with HEAD and Range support. Paths under `/fail/`
    /// answer 500, paths under `/short/` send half the body and close.
    pub(crate) struct Server {
        pub url: String,
        pub requests: Arc<Mutex<Vec<String>>>,
        _thread: std::thread::JoinHandle<()>,
    }

    pub(crate) fn serve(files: Vec<(&'static str, Vec<u8>)>) -> Server {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}", server.server_addr().to_ip().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        let thread = std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let path = request.url().to_string();
                let range = request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Range"))
                    .map(|h| h.value.as_str().to_string());
                log.lock().unwrap().push(format!(
                    "{} {path}{}",
                    request.method(),
                    range.as_ref().map(|r| format!(" {r}")).unwrap_or_default()
                ));
                let name = path.rsplit('/').next().unwrap_or_default();
                let Some((_, body)) = files.iter().find(|(n, _)| *n == name) else {
                    let _ = request.respond(tiny_http::Response::empty(404));
                    continue;
                };
                if path.contains("/fail/") {
                    let _ = request.respond(tiny_http::Response::empty(500));
                    continue;
                }
                let start = range
                    .as_deref()
                    .and_then(|r| r.strip_prefix("bytes="))
                    .and_then(|r| r.trim_end_matches('-').parse::<usize>().ok())
                    .unwrap_or(0);
                if *request.method() == tiny_http::Method::Head {
                    // tiny_http sends the length and no body for HEAD.
                    let _ = request.respond(
                        tiny_http::Response::from_data(body.clone())
                            .with_chunked_threshold(usize::MAX),
                    );
                    continue;
                }
                if path.contains("/short/") {
                    // Promise the full length, send half, close.
                    let mut writer = request.into_writer();
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = writer.write_all(head.as_bytes());
                    let _ = writer.write_all(&body[..body.len() / 2]);
                    let _ = writer.flush();
                    continue;
                }
                let part = body[start.min(body.len())..].to_vec();
                let mut response =
                    tiny_http::Response::from_data(part).with_chunked_threshold(usize::MAX);
                if start > 0 {
                    response = response.with_status_code(206).with_header(
                        tiny_http::Header::from_bytes(
                            "Content-Range",
                            format!("bytes {start}-{}/{}", body.len() - 1, body.len()),
                        )
                        .unwrap(),
                    );
                }
                let _ = request.respond(response);
            }
        });
        Server {
            url,
            requests,
            _thread: thread,
        }
    }

    fn remote(server: &Server, prefix: &str, name: &str) -> RemoteFile {
        RemoteFile {
            name: name.into(),
            url: format!("{}/{prefix}/{name}", server.url),
        }
    }

    #[test]
    fn fetches_every_file_and_reports_the_content_length_total() {
        let a = vec![1u8; 300_000];
        let b = vec![2u8; 1000];
        let server = serve(vec![("a.bin", a.clone()), ("b.txt", b.clone())]);
        let dir = tempfile::tempdir().unwrap();
        let mut reports = Vec::new();
        Downloader::new()
            .fetch_all(
                &[
                    remote(&server, "ok", "a.bin"),
                    remote(&server, "ok", "b.txt"),
                ],
                dir.path(),
                Some(5),
                &mut |d, t| reports.push((d, t)),
            )
            .unwrap();
        assert_eq!(fs::read(dir.path().join("a.bin")).unwrap(), a);
        assert_eq!(fs::read(dir.path().join("b.txt")).unwrap(), b);
        assert!(!dir.path().join("a.bin.partial").exists());
        assert!(reports.iter().all(|(_, t)| *t == 301_000), "{reports:?}");
        assert_eq!(reports.last(), Some(&(301_000, 301_000)));
        assert!(reports.windows(2).all(|w| w[0].0 <= w[1].0));
    }

    #[test]
    fn a_failed_file_stays_partial_and_complete_files_are_skipped_next_time() {
        let server = serve(vec![("a.bin", vec![1u8; 1000]), ("b.bin", vec![2u8; 1000])]);
        let dir = tempfile::tempdir().unwrap();
        let error = Downloader::new()
            .fetch_all(
                &[
                    remote(&server, "ok", "a.bin"),
                    remote(&server, "fail", "b.bin"),
                ],
                dir.path(),
                None,
                &mut |_, _| {},
            )
            .unwrap_err();
        assert!(error.contains("b.bin") && error.contains("500"), "{error}");
        assert!(dir.path().join("a.bin").exists());
        assert!(!dir.path().join("b.bin").exists());

        server.requests.lock().unwrap().clear();
        Downloader::new()
            .fetch_all(
                &[
                    remote(&server, "ok", "a.bin"),
                    remote(&server, "ok", "b.bin"),
                ],
                dir.path(),
                None,
                &mut |_, _| {},
            )
            .unwrap();
        let requests = server.requests.lock().unwrap().clone();
        assert!(
            requests.iter().all(|r| !r.contains("a.bin")),
            "a.bin is complete: {requests:?}"
        );
        assert_eq!(fs::read(dir.path().join("b.bin")).unwrap(), vec![2u8; 1000]);
    }

    #[test]
    fn resumes_a_partial_file_with_a_range_request() {
        let body: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let server = serve(vec![("model.onnx", body.clone())]);
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("model.onnx.partial"), &body[..4000]).unwrap();
        let mut first = None;
        Downloader::new()
            .fetch_all(
                &[remote(&server, "ok", "model.onnx")],
                dir.path(),
                None,
                &mut |d, _| {
                    first.get_or_insert(d);
                },
            )
            .unwrap();
        assert_eq!(fs::read(dir.path().join("model.onnx")).unwrap(), body);
        let requests = server.requests.lock().unwrap().clone();
        assert!(
            requests
                .iter()
                .any(|r| r.starts_with("GET") && r.ends_with("bytes=4000-")),
            "{requests:?}"
        );
        assert_eq!(first, Some(0));
    }

    #[test]
    fn a_short_body_is_retried_and_then_fails_without_a_final_file() {
        let server = serve(vec![("big.bin", vec![7u8; 100_000])]);
        let dir = tempfile::tempdir().unwrap();
        let error = Downloader::new()
            .with_request_time(Duration::from_millis(500))
            .fetch_all(
                &[remote(&server, "short", "big.bin")],
                dir.path(),
                None,
                &mut |_, _| {},
            )
            .unwrap_err();
        assert!(error.contains("big.bin"), "{error}");
        assert!(!dir.path().join("big.bin").exists());
        let gets = server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.starts_with("GET"))
            .count();
        // The first attempt moved the file forward, so it does not count.
        assert_eq!(gets, ATTEMPTS as usize + 1);
    }

    #[test]
    fn missing_files_fail_at_once() {
        let server = serve(vec![]);
        let dir = tempfile::tempdir().unwrap();
        let error = Downloader::new()
            .fetch_all(
                &[remote(&server, "ok", "nope.bin")],
                dir.path(),
                Some(10),
                &mut |_, _| {},
            )
            .unwrap_err();
        assert!(error.contains("404"), "{error}");
    }
}
