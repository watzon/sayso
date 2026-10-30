//! Download a model archive (`.tar.bz2`) and extract it, or download one file.
//!
//! The archive streams from the network through the bzip2 decoder into the tar
//! reader, so a download needs no temporary archive file. The files go to a
//! staging folder first. Only a complete extraction (or file) with the full
//! byte count moves into place.

use anyhow::{Context, Result, bail};
use bzip2::read::MultiBzDecoder;
use sayso_core::engine_catalog::descriptors::{Artifact, Source, check_path};
use std::io::Read;
use std::path::{Component, Path};
use std::time::{Duration, Instant};

/// The sherpa-onnx release that holds the speech model archives.
pub const DEFAULT_BASE_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models";

/// The release with the segmentation models of diarization.
pub const SEGMENTATION_RELEASE: &str = "speaker-segmentation-models";

/// The release with the speaker embedding models. The tag is spelled so on GitHub.
pub const EMBEDDING_RELEASE: &str = "speaker-recongition-models";

/// Time between two progress messages.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// Hugging Face, where a `ParakeetCpp` model's GGUF file comes from.
pub const DEFAULT_HF_ENDPOINT: &str = "https://huggingface.co";

pub fn archive_url(base_url: &str, archive: &str) -> String {
    format!("{}/{archive}.tar.bz2", base_url.trim_end_matches('/'))
}

/// A file of a release, such as a VAD model.
pub fn file_url(base_url: &str, file: &str) -> String {
    format!("{}/{file}", base_url.trim_end_matches('/'))
}

/// The release `release` next to the release at `base_url`: the same place, another last part.
pub fn sibling_release_url(base_url: &str, release: &str) -> String {
    match base_url.trim_end_matches('/').rsplit_once('/') {
        Some((parent, _)) => format!("{parent}/{release}"),
        None => release.to_string(),
    }
}

/// The start of the URL of a file of a sherpa-onnx release in a descriptor.
pub const RELEASES_PREFIX: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/";

/// The start of the URL of a file of Hugging Face in a descriptor.
pub const HF_PREFIX: &str = "https://huggingface.co/";

pub fn hf_file_url(hf_endpoint: &str, repo: &str, revision: &str, file: &str) -> String {
    format!("{}/{repo}/resolve/{revision}/{file}", hf_endpoint.trim_end_matches('/'))
}

/// An HTTP agent for archives. It follows redirects (GitHub release files redirect).
pub fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        // A stalled download holds a lock of the model store that other engines
        // wait for. ureq has no limit for the time between two reads, so this is
        // a limit for the whole file, far above what the largest model needs.
        .timeout_recv_body(Some(Duration::from_secs(6 * 60 * 60)))
        .max_redirects(10)
        .build();
    ureq::Agent::new_with_config(config)
}

/// Download the archive at `url` and extract it to `dest`, or with `file`, put
/// the file at `url` in the folder `dest` under that name. `staging` is a
/// temporary folder beside `dest`. `on_bytes` gets the size of each piece
/// that arrives.
pub fn fetch(
    agent: &ureq::Agent,
    url: &str,
    file: Option<&str>,
    staging: &Path,
    dest: &Path,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<()> {
    remove_dir_if_exists(staging)?;
    std::fs::create_dir_all(staging)
        .with_context(|| format!("cannot create {}", staging.display()))?;
    let result = match file {
        Some(name) => download_to_file(agent, url, &staging.join(name), on_bytes),
        None => download_and_extract(agent, url, staging, on_bytes),
    }
    .and_then(|()| move_into_place(staging, dest));
    if result.is_err() {
        let _ = std::fs::remove_dir_all(staging);
    }
    result
}

/// Put the files of an artifact of the model store at `files`. `url_of` gives
/// the URL to use for a URL of a descriptor. `work` is an empty folder for the
/// time of the download.
pub fn fetch_artifact(
    agent: &ureq::Agent,
    artifact: &Artifact,
    url_of: &dyn Fn(&str) -> String,
    work: &Path,
    files: &Path,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<()> {
    if let Source::Archive { url } = &artifact.source {
        return fetch(agent, &url_of(url), None, &work.join("extract"), files, on_bytes);
    }
    for file in &artifact.files {
        check_path(&file.path)?;
        let url = artifact.source.file_url(&file.path).context("a source without a file URL")?;
        let path = files.join(&file.path);
        let folder = path.parent().context("a file has a folder")?;
        std::fs::create_dir_all(folder).with_context(|| format!("cannot create {}", folder.display()))?;
        download_to_file(agent, &url_of(&url), &path, on_bytes)?;
    }
    Ok(())
}

fn download_to_file(
    agent: &ureq::Agent,
    url: &str,
    path: &Path,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<()> {
    let (mut body, expected) = get(agent, url, on_bytes)?;
    let mut file = std::fs::File::create(path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    std::io::copy(&mut body, &mut file).with_context(|| format!("cannot download {url}"))?;
    file.sync_all()
        .with_context(|| format!("cannot write {}", path.display()))?;
    check_size(url, body.count, expected)
}

/// The response body, counted, and the size the server announced.
fn get<'a>(
    agent: &ureq::Agent,
    url: &str,
    on_bytes: &'a mut dyn FnMut(u64),
) -> Result<(Counting<'a, impl Read + use<'a>>, Option<u64>)> {
    let response = agent
        .get(url)
        .call()
        .with_context(|| format!("cannot download {url}"))?;
    let expected: Option<u64> = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let body = Counting {
        inner: response.into_body().into_reader(),
        count: 0,
        on_bytes,
    };
    Ok((body, expected))
}

fn check_size(url: &str, count: u64, expected: Option<u64>) -> Result<()> {
    if let Some(expected) = expected
        && count != expected
    {
        bail!("download of {url} is incomplete: {count} of {expected} bytes");
    }
    Ok(())
}

fn download_and_extract(
    agent: &ureq::Agent,
    url: &str,
    staging: &Path,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<()> {
    let (mut body, expected) = get(agent, url, on_bytes)?;
    {
        let mut archive = tar::Archive::new(MultiBzDecoder::new(&mut body));
        archive.set_preserve_permissions(false);
        for entry in archive.entries().with_context(|| format!("cannot extract {url}"))? {
            let mut entry = entry.with_context(|| format!("cannot extract {url}"))?;
            let kind = entry.header().entry_type();
            if kind.is_pax_global_extensions() || kind.is_pax_local_extensions() {
                continue;
            }
            let path = entry.path().with_context(|| format!("cannot extract {url}"))?.into_owned();
            let plain = path.components().all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
            if !plain || !(kind.is_file() || kind.is_dir() || kind.is_gnu_sparse()) {
                bail!("the archive {url} has an entry that is not a plain file or folder: {}", path.display());
            }
            entry.unpack_in(staging).with_context(|| format!("cannot extract {url}"))?;
        }
    }
    // Read to the end, so that the size check counts every byte.
    std::io::copy(&mut body, &mut std::io::sink())
        .with_context(|| format!("cannot download {url}"))?;
    check_size(url, body.count, expected)
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

    /// Serve `body` once and return the URL.
    fn serve_once(body: Vec<u8>) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            if let Ok(request) = server.recv() {
                let _ = request.respond(tiny_http::Response::from_data(body));
            }
        });
        format!("http://127.0.0.1:{port}/a.tar.bz2")
    }

    fn compressed(tar: Vec<u8>) -> Vec<u8> {
        use std::io::Write;
        let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        encoder.write_all(&tar).unwrap();
        encoder.finish().unwrap()
    }

    /// Fetch an archive with one entry and return the result.
    fn fetch_entry(path: &str, kind: tar::EntryType, link: Option<&str>) -> Result<()> {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(kind);
        header.set_mode(0o644);
        header.set_size(if kind.is_file() { 1 } else { 0 });
        // `set_path` refuses `..`, so the path goes straight into the header.
        let name = &mut header.as_old_mut().name;
        name[..path.len()].copy_from_slice(path.as_bytes());
        if let Some(link) = link {
            header.set_link_name(link).unwrap();
        }
        header.set_cksum();
        let mut builder = tar::Builder::new(Vec::new());
        let body: &[u8] = if kind.is_file() { b"x" } else { b"" };
        builder.append(&header, body).unwrap();
        let url = serve_once(compressed(builder.into_inner().unwrap()));
        let dir = tempfile::tempdir().unwrap();
        fetch(&agent(), &url, None, &dir.path().join("work"), &dir.path().join("dest"), &mut |_| {})
    }

    #[test]
    fn an_archive_with_a_link_or_an_unsafe_path_is_refused() {
        fetch_entry("a/model.onnx", tar::EntryType::Regular, None).unwrap();
        for (path, kind, link) in [
            ("a/link", tar::EntryType::Symlink, Some("/etc/passwd")),
            ("a/hard", tar::EntryType::Link, Some("a/model.onnx")),
            ("../outside", tar::EntryType::Regular, None),
            ("/absolute", tar::EntryType::Regular, None),
        ] {
            let error = fetch_entry(path, kind, link).unwrap_err();
            assert!(format!("{error:#}").contains("not a plain file or folder"), "{path}: {error:#}");
        }
    }

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
        assert_eq!(hf_file_url("http://h/", "o/r", "c0ffee", "m.gguf"), "http://h/o/r/resolve/c0ffee/m.gguf");
        assert_eq!(file_url("http://h/x/", "silero_vad.onnx"), "http://h/x/silero_vad.onnx");
        assert_eq!(
            sibling_release_url(DEFAULT_BASE_URL, EMBEDDING_RELEASE),
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models"
        );
        assert_eq!(sibling_release_url("http://h/redirect/", "r"), "http://h/r");
    }
}
