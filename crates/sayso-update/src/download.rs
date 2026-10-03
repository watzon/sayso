//! The download of one release file, with its size and hash from the manifest.

use crate::manifest::Asset;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DownloadError {
    #[error("the download was cancelled")]
    Cancelled,
    #[error("the file is not the file that the update information names")]
    Mismatch,
    #[error("the server answered with HTTP {0}")]
    Status(u16),
    #[error("{0}")]
    Network(String),
    #[error("Sayso could not write the file: {0}")]
    Disk(String),
}

/// The name of the file in `dir`: the last part of the URL, with only safe characters.
pub fn file_name(asset: &Asset) -> String {
    let last = asset.url.rsplit('/').next().unwrap_or_default();
    let name: String = last.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')).collect();
    if name.trim_matches('.').is_empty() { "update".into() } else { name }
}

/// Download `asset` to `dir` and return the path. Blocks.
///
/// The file is complete only when its size and SHA-256 hash are the values in
/// the manifest. Until then it has the suffix `.part`. A download does not
/// resume: each call starts from zero. `on_progress` gets the bytes received.
pub fn download(
    asset: &Asset,
    dir: &Path,
    https_only: bool,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(u64),
) -> Result<PathBuf, DownloadError> {
    let disk = |e: std::io::Error| DownloadError::Disk(e.to_string());
    std::fs::create_dir_all(dir).map_err(disk)?;
    let dest = dir.join(file_name(asset));
    let part = dir.join(format!("{}.part", file_name(asset)));
    let result = fetch(asset, &part, https_only, cancel, on_progress);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    std::fs::rename(&part, &dest).map_err(disk)?;
    Ok(dest)
}

fn fetch(asset: &Asset, part: &Path, https_only: bool, cancel: &AtomicBool, on_progress: &mut dyn FnMut(u64)) -> Result<(), DownloadError> {
    let network = |e: ureq::Error| DownloadError::Network(e.to_string());
    let disk = |e: std::io::Error| DownloadError::Disk(e.to_string());
    // No timeout for the whole call: a release file is large. GitHub
    // redirects a release file to its storage host.
    let config = ureq::Agent::config_builder()
        .user_agent(crate::check::user_agent())
        .https_only(https_only)
        .max_redirects(10)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_recv_body(Some(Duration::from_secs(20 * 60)))
        .http_status_as_error(false)
        .build();
    let mut response = ureq::Agent::new_with_config(config).get(&asset.url).call().map_err(network)?;
    if response.status().as_u16() != 200 {
        return Err(DownloadError::Status(response.status().as_u16()));
    }
    let mut reader = response.body_mut().as_reader();
    let mut file = std::fs::File::create(part).map_err(disk)?;
    let mut hash = Sha256::new();
    let mut received = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        // A read that fails after the user cancelled is a cancel. A read that
        // waits for a silent server cannot be stopped before its timeout.
        let n = reader.read(&mut buffer).map_err(|e| match cancel.load(Ordering::SeqCst) {
            true => DownloadError::Cancelled,
            false => DownloadError::Network(e.to_string()),
        })?;
        if n == 0 {
            break;
        }
        received += n as u64;
        // A file that is larger than the manifest says is never the right file.
        if received > asset.size {
            return Err(DownloadError::Mismatch);
        }
        hash.update(&buffer[..n]);
        file.write_all(&buffer[..n]).map_err(disk)?;
        on_progress(received);
    }
    file.sync_all().map_err(disk)?;
    let hex: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if received != asset.size || hex != asset.sha256 {
        return Err(DownloadError::Mismatch);
    }
    Ok(())
}

/// The SHA-256 hash of a file, in lower-case hex.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::check::tests::serve;

    pub fn asset_for(url: &str, body: &[u8]) -> Asset {
        let hex: String = Sha256::digest(body).iter().map(|b| format!("{b:02x}")).collect();
        Asset { url: url.to_string(), sha256: hex, size: body.len() as u64 }
    }

    fn run(asset: &Asset, dir: &Path) -> Result<PathBuf, DownloadError> {
        download(asset, dir, false, &AtomicBool::new(false), &mut |_| {})
    }

    #[test]
    fn a_good_file_arrives_with_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let body = vec![7u8; 200_000];
        let server = serve(vec![(200, body.clone())]);
        let asset = asset_for(&server.url.replace("latest.json", "Sayso-0.3.0-macos-arm64.dmg"), &body);
        let mut last = 0;
        let path = download(&asset, dir.path(), false, &AtomicBool::new(false), &mut |n| last = n).unwrap();
        assert_eq!(path.file_name().unwrap(), "Sayso-0.3.0-macos-arm64.dmg");
        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert_eq!(last, body.len() as u64);
        assert_eq!(sha256_file(&path).unwrap(), asset.sha256);
    }

    #[test]
    fn a_wrong_hash_or_size_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let server = serve(vec![(200, b"the real file".to_vec())]);
        let mut wrong_hash = asset_for(&server.url, b"the real file");
        wrong_hash.sha256 = "0".repeat(64);
        assert_eq!(run(&wrong_hash, dir.path()), Err(DownloadError::Mismatch));
        let mut too_small = asset_for(&server.url, b"the real file");
        too_small.size -= 1;
        assert_eq!(run(&too_small, dir.path()), Err(DownloadError::Mismatch));
        let mut too_large = asset_for(&server.url, b"the real file");
        too_large.size += 1;
        assert_eq!(run(&too_large, dir.path()), Err(DownloadError::Mismatch));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_cancelled_download_and_a_missing_file_fail() {
        let dir = tempfile::tempdir().unwrap();
        let server = serve(vec![(200, vec![1u8; 1000])]);
        let asset = asset_for(&server.url, &[1u8; 1000]);
        let cancelled = download(&asset, dir.path(), false, &AtomicBool::new(true), &mut |_| {});
        assert_eq!(cancelled, Err(DownloadError::Cancelled));
        let gone = serve(vec![(404, vec![])]);
        assert_eq!(run(&asset_for(&gone.url, b"x"), dir.path()), Err(DownloadError::Status(404)));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn the_file_name_has_only_safe_characters() {
        let name = |url: &str| file_name(&Asset { url: url.into(), sha256: String::new(), size: 1 });
        assert_eq!(name("https://x/releases/download/v1/Sayso-1.0.0-macos-arm64.dmg"), "Sayso-1.0.0-macos-arm64.dmg");
        assert_eq!(name("https://x/a/..%2F..%2Fetc"), "..2F..2Fetc");
        assert_eq!(name("https://x/a/"), "update");
        assert_eq!(name("https://x/a/.."), "update");
    }
}
