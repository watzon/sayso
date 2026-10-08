//! App icons for badges, by bundle id.
//!
//! The first request renders the icon from the system (running app, then
//! Launch Services) to a PNG in `<cache>/icons`. Later requests read the file,
//! then memory. A file older than [`MAX_AGE`] is rendered again, so an app
//! update shows its new icon. Apps without an icon fall back to the letter
//! badge; that result is kept in memory only, so a later install is found.

use gpui_kit::{Image, ImageFormat};
use sayso_platform::ContextProvider;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Pixel size of the cached PNG. Badges are 16 to 28 points, so 64 px is sharp at 2x.
const ICON_PX: u32 = 64;
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

pub struct AppIcons {
    dir: PathBuf,
    context: Arc<dyn ContextProvider>,
    /// Main thread only: icons are read while views render.
    memory: RefCell<HashMap<String, Option<Arc<Image>>>>,
}

impl AppIcons {
    pub fn new(dir: PathBuf, context: Arc<dyn ContextProvider>) -> Self {
        Self { dir, context, memory: RefCell::default() }
    }

    /// The icon for `bundle_id`, or None when the system has none.
    pub fn get(&self, bundle_id: &str) -> Option<Arc<Image>> {
        if bundle_id.is_empty() {
            return None;
        }
        if let Some(hit) = self.memory.borrow().get(bundle_id) {
            return hit.clone();
        }
        let icon = self.load(bundle_id).map(|png| Arc::new(Image::from_bytes(ImageFormat::Png, png)));
        self.memory.borrow_mut().insert(bundle_id.to_string(), icon.clone());
        icon
    }

    fn load(&self, bundle_id: &str) -> Option<Vec<u8>> {
        let file = cache_file(&self.dir, bundle_id);
        if is_fresh(&file, MAX_AGE)
            && let Ok(png) = std::fs::read(&file)
        {
            return Some(png);
        }
        match self.context.app_icon_png(bundle_id, ICON_PX) {
            Some(png) => {
                let _ = std::fs::create_dir_all(&self.dir);
                if let Err(e) = std::fs::write(&file, &png) {
                    log::debug!("could not cache the icon of {bundle_id}: {e}");
                }
                Some(png)
            }
            // The app may be gone. A stale file is better than no icon.
            None => std::fs::read(&file).ok(),
        }
    }
}

/// `<dir>/<id>.png`, with anything outside `[A-Za-z0-9.-]` replaced.
pub(crate) fn cache_file(dir: &Path, bundle_id: &str) -> PathBuf {
    let safe: String =
        bundle_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
    dir.join(format!("{safe}.png"))
}

pub(crate) fn is_fresh(file: &Path, max_age: Duration) -> bool {
    std::fs::metadata(file)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().is_ok_and(|age| age < max_age))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sayso_platform::AppInfo;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Gives a fixed PNG for one bundle id and counts the renders.
    struct FakeContext {
        renders: AtomicUsize,
    }

    impl ContextProvider for FakeContext {
        fn frontmost_app(&self) -> Option<AppInfo> {
            None
        }
        fn running_apps(&self) -> Vec<AppInfo> {
            Vec::new()
        }
        fn app_icon_png(&self, bundle_id: &str, _size: u32) -> Option<Vec<u8>> {
            self.renders.fetch_add(1, Ordering::SeqCst);
            (bundle_id == "com.example.app").then(|| b"png".to_vec())
        }
    }

    fn icons(dir: &Path) -> (AppIcons, Arc<FakeContext>) {
        let context = Arc::new(FakeContext { renders: AtomicUsize::new(0) });
        (AppIcons::new(dir.to_path_buf(), context.clone()), context)
    }

    #[test]
    fn renders_once_then_reads_memory_and_disk() {
        let dir = tempfile::tempdir().unwrap();
        let (a, context) = icons(dir.path());
        assert!(a.get("com.example.app").is_some());
        assert!(a.get("com.example.app").is_some());
        assert_eq!(context.renders.load(Ordering::SeqCst), 1, "memory hit");
        assert_eq!(std::fs::read(dir.path().join("com.example.app.png")).unwrap(), b"png");

        // A new run reads the file and does not render.
        let (b, context) = icons(dir.path());
        assert!(b.get("com.example.app").is_some());
        assert_eq!(context.renders.load(Ordering::SeqCst), 0, "disk hit");
    }

    #[test]
    fn missing_icons_fall_back_and_are_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let (a, context) = icons(dir.path());
        assert!(a.get("com.example.gone").is_none());
        assert!(a.get("com.example.gone").is_none());
        assert_eq!(context.renders.load(Ordering::SeqCst), 1);
        assert!(!dir.path().join("com.example.gone.png").exists());
        assert!(a.get("").is_none());
    }

    #[test]
    fn cache_file_names_are_safe() {
        assert_eq!(cache_file(Path::new("/c"), "com.a-b.App"), Path::new("/c/com.a-b.App.png"));
        assert_eq!(cache_file(Path::new("/c"), "../x y"), Path::new("/c/.._x_y.png"));
    }
}
