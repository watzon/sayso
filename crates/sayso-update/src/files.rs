//! File writes that survive a crash.

use std::io::Write as _;
use std::path::Path;

/// Write `bytes` to a temporary file beside `path`, sync it, and rename it.
/// A crash leaves the old file or the new file, and never half of one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)?;
    sync_dir(path.parent().unwrap_or(Path::new(".")));
    Ok(())
}

/// Make a rename in `dir` durable. Windows has no such call for a folder.
pub fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(dir) = std::fs::File::open(dir) {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}
