//! Download a model archive (`.tar.bz2`) and extract it.
//!
//! The archive streams from the network through the bzip2 decoder into the tar
//! reader, so a download needs no temporary archive file. The files go to a
//! staging folder first. Only a complete extraction with the full byte count
//! moves into place.

use anyhow::{Context, Result, bail};
use bzip2::read::MultiBzDecoder;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

/// The sherpa-onnx release that holds the speech model archives.
pub const DEFAULT_BASE_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models";

/// Time between two progress messages.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

pub fn archive_url(base_url: &str, archive: &str) -> String {
    format!("{}/{archive}.tar.bz2", base_url.trim_end_matches('/'))
}

/// An HTTP agent for archives. It follows redirects (GitHub release files redirect).
pub fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .max_redirects(10)
        .build();
    ureq::Agent::new_with_config(config)
}

/// Download the archive at `url` and extract it to `dest`. `staging` is a
/// temporary folder beside `dest`. `on_bytes` gets the size of each piece
/// that arrives.
pub fn fetch_archive(
    agent: &ureq::Agent,
    url: &str,
    staging: &Path,
    dest: &Path,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<()> {
    remove_dir_if_exists(staging)?;
    std::fs::create_dir_all(staging)
        .with_context(|| format!("cannot create {}", staging.display()))?;
    let result = download_and_extract(agent, url, staging, on_bytes)
        .and_then(|()| move_into_place(staging, dest));
    if result.is_err() {
        let _ = std::fs::remove_dir_all(staging);
    }
    result
}

fn download_and_extract(
    agent: &ureq::Agent,
    url: &str,
    staging: &Path,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<()> {
    let response = agent
        .get(url)
        .call()
        .with_context(|| format!("cannot download {url}"))?;
    let expected: Option<u64> = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let mut body = Counting {
        inner: response.into_body().into_reader(),
        count: 0,
        on_bytes,
    };
    {
        let mut archive = tar::Archive::new(MultiBzDecoder::new(&mut body));
        archive.set_preserve_permissions(false);
        archive
            .unpack(staging)
            .with_context(|| format!("cannot extract {url}"))?;
    }
    // Read to the end, so that the size check counts every byte.
    std::io::copy(&mut body, &mut std::io::sink())
        .with_context(|| format!("cannot download {url}"))?;
    if let Some(expected) = expected
        && body.count != expected
    {
        bail!(
            "download of {url} is incomplete: {} of {expected} bytes",
            body.count
        );
    }
    Ok(())
}

/// Move the extracted files to `dest`. An archive usually holds one top
/// folder. That folder becomes `dest`.
fn move_into_place(staging: &Path, dest: &Path) -> Result<()> {
    let entries: Vec<_> = std::fs::read_dir(staging)?.collect::<std::io::Result<_>>()?;
    remove_dir_if_exists(dest)?;
    match entries.as_slice() {
        [only] if only.file_type()?.is_dir() => {
            std::fs::rename(only.path(), dest)
                .with_context(|| format!("cannot move files to {}", dest.display()))?;
            std::fs::remove_dir(staging)
                .with_context(|| format!("cannot remove {}", staging.display()))?;
        }
        [] => bail!("the archive is empty"),
        _ => std::fs::rename(staging, dest)
            .with_context(|| format!("cannot move files to {}", dest.display()))?,
    }
    Ok(())
}

fn remove_dir_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("cannot remove {}", path.display()))
        }
        _ => Ok(()),
    }
}

/// A reader that counts the bytes that pass through it.
struct Counting<'a, R> {
    inner: R,
    count: u64,
    on_bytes: &'a mut dyn FnMut(u64),
}

impl<R: Read> Read for Counting<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count += n as u64;
        if n > 0 {
            (self.on_bytes)(n as u64);
        }
        Ok(n)
    }
}

/// Download progress over all archives of a model. The fraction never goes
/// backwards and stays below 1 until the download is complete.
pub struct Progress {
    total: u64,
    done: u64,
    highest: f64,
    last_emit: Option<Instant>,
}

impl Progress {
    /// `total` is the expected size in bytes (the catalog size). Zero means unknown.
    pub fn new(total: u64) -> Self {
        Progress {
            total,
            done: 0,
            highest: 0.0,
            last_emit: None,
        }
    }

    pub fn add(&mut self, bytes: u64) {
        self.done += bytes;
        if self.total > 0 {
            let fraction = (self.done as f64 / self.total as f64).min(0.99);
            self.highest = self.highest.max(fraction);
        }
    }

    pub fn fraction(&self) -> f64 {
        self.highest
    }

    pub fn bytes_done(&self) -> u64 {
        self.done
    }

    /// The expected size, or the bytes so far when the download is larger.
    pub fn bytes_total(&self) -> u64 {
        self.total.max(self.done)
    }

    /// True when a progress message is due at `now`. It then counts as sent.
    pub fn due(&mut self, now: Instant) -> bool {
        let due = self
            .last_emit
            .is_none_or(|last| now.duration_since(last) >= PROGRESS_INTERVAL);
        if due {
            self.last_emit = Some(now);
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_monotonic_capped_and_throttled() {
        let mut progress = Progress::new(1000);
        let start = Instant::now();
        assert!(progress.due(start));
        progress.add(500);
        assert_eq!(progress.fraction(), 0.5);
        assert!(!progress.due(start + Duration::from_millis(100)));
        assert!(progress.due(start + Duration::from_millis(300)));
        progress.add(700);
        assert_eq!(progress.fraction(), 0.99, "below 1 until done");
        assert_eq!(progress.bytes_total(), 1200);

        let mut unknown = Progress::new(0);
        unknown.add(10);
        assert_eq!(unknown.fraction(), 0.0);
        assert_eq!(unknown.bytes_total(), 10);
    }

    #[test]
    fn urls_join_base_and_archive() {
        assert_eq!(archive_url("http://h/x/", "a-b"), "http://h/x/a-b.tar.bz2");
        assert!(archive_url(DEFAULT_BASE_URL, "a").starts_with("https://github.com/"));
    }
}
