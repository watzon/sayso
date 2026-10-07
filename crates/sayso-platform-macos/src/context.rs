//! [`ContextProvider`]: frontmost app, running apps, and app icons.

use crate::ffi::{
    AXUIElementCopyAttributeValue, AXUIElementCreateApplication, AXUIElementSetMessagingTimeout,
    CFTypeRef, kAXErrorSuccess,
};
use core_foundation::base::{CFType, TCFType};
use core_foundation::string::CFString;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSBitmapImageRep, NSBitmapImageFileType, NSDeviceRGBColorSpace,
    NSGraphicsContext, NSCompositingOperation, NSImage, NSRunningApplication, NSWorkspace,
};
use objc2_foundation::{NSBundle, NSDictionary, NSFileManager, NSPoint, NSRect, NSSize, NSString};
use sayso_platform::{AppInfo, ContextProvider};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct MacContext;

/// Convert a running app to our plain type. Apps without a bundle id get an empty id.
fn app_info(app: &NSRunningApplication) -> AppInfo {
    AppInfo {
        bundle_id: app.bundleIdentifier().map(|s| s.to_string()).unwrap_or_default(),
        name: app.localizedName().map(|s| s.to_string()).unwrap_or_default(),
        pid: app.processIdentifier(),
    }
}

/// Localized name of the process with this PID.
pub fn app_name_for_pid(pid: i32) -> Option<String> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .and_then(|app| app.localizedName())
        .map(|s| s.to_string())
}

/// Running apps that can have a UI or a menu bar item (regular and accessory apps).
/// Raycast and Alfred are accessory apps, so they must be included.
pub fn running_apps() -> Vec<AppInfo> {
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter(|app| app.activationPolicy() != NSApplicationActivationPolicy::Prohibited)
        .map(|app| app_info(&app))
        .collect()
}

pub fn frontmost_app() -> Option<AppInfo> {
    NSWorkspace::sharedWorkspace().frontmostApplication().map(|app| app_info(&app))
}

/// The icon of an app, by bundle id. Running apps first, then Launch Services.
fn icon_for_bundle_id(bundle_id: &str) -> Option<Retained<NSImage>> {
    let workspace = NSWorkspace::sharedWorkspace();
    let id = NSString::from_str(bundle_id);
    let running = NSRunningApplication::runningApplicationsWithBundleIdentifier(&id);
    if let Some(icon) = running.iter().find_map(|app| app.icon()) {
        return Some(icon);
    }
    let url = workspace.URLForApplicationWithBundleIdentifier(&id)?;
    let path = url.path()?;
    Some(workspace.iconForFile(&path))
}

/// Draw `image` into a `size` by `size` pixel bitmap and encode it as PNG.
fn render_png(image: &NSImage, size: u32) -> Option<Vec<u8>> {
    let px = isize::try_from(size).ok().filter(|s| *s > 0)?;
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            px,
            px,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    let dest = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(size as f64, size as f64));
    image.drawInRect_fromRect_operation_fraction(
        dest,
        NSRect::ZERO,
        NSCompositingOperation::SourceOver,
        1.0,
    );
    NSGraphicsContext::restoreGraphicsState_class();
    let data = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(data.to_vec())
}

/// How long one Accessibility call may wait for the app, in seconds.
const AX_TIMEOUT_SECS: f32 = 0.25;

/// Copy one Accessibility attribute. The result is owned and released on drop.
///
/// # Safety
/// `element` must be a valid `AXUIElementRef`.
unsafe fn copy_attribute(element: CFTypeRef, name: &str) -> Option<CFType> {
    let attribute = CFString::new(name);
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: the caller gives a valid element. On success `value` is a retained object.
    let status = unsafe {
        AXUIElementCopyAttributeValue(element, attribute.as_concrete_TypeRef().cast(), &mut value)
    };
    if status != kAXErrorSuccess || value.is_null() {
        return None;
    }
    // SAFETY: "Copy" gives a +1 reference, so the wrapper owns it.
    Some(unsafe { CFType::wrap_under_create_rule(value) })
}

/// The `AXDocument` of the focused window of the app with this PID.
/// It reads one attribute on the app and one on the window. It never walks the tree.
fn focused_window_document(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    // SAFETY: the app element is created here and released by the wrapper.
    // Every other call gets an element that stays alive in this scope.
    unsafe {
        let raw = AXUIElementCreateApplication(pid);
        if raw.is_null() {
            return None;
        }
        let app = CFType::wrap_under_create_rule(raw);
        AXUIElementSetMessagingTimeout(app.as_concrete_TypeRef().cast(), AX_TIMEOUT_SECS);
        let window = copy_attribute(app.as_concrete_TypeRef().cast(), "AXFocusedWindow")?;
        let document = copy_attribute(window.as_concrete_TypeRef().cast(), "AXDocument")?;
        let document = document.downcast_into::<CFString>()?.to_string();
        Some(document).filter(|s| !s.is_empty())
    }
}

/// Folders that hold apps, and whether to look one level into their sub-folders.
fn app_roots() -> Vec<(PathBuf, bool)> {
    let mut roots = vec![
        (PathBuf::from("/Applications"), true),
        (PathBuf::from("/Applications/Utilities"), false),
        (PathBuf::from("/System/Applications"), false),
        (PathBuf::from("/System/Applications/Utilities"), false),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push((Path::new(&home).join("Applications"), true));
    }
    roots
}

fn is_app_bundle(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "app") && path.is_dir()
}

/// The `*.app` folders in each root. With the flag set, also the ones one
/// level inside a sub-folder that is not an app (for example `Setapp`).
/// A root that cannot be read is skipped.
fn collect_app_paths(roots: &[(PathBuf, bool)]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for (root, descend) in roots {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if is_app_bundle(&path) {
                found.push(path);
            } else if *descend && path.is_dir() {
                let Ok(inner) = std::fs::read_dir(&path) else { continue };
                let mut inner: Vec<PathBuf> =
                    inner.flatten().map(|e| e.path()).filter(|p| is_app_bundle(p)).collect();
                inner.sort();
                found.extend(inner);
            }
        }
    }
    found
}

/// Bundle id and display name of the app at `path`. Bundles with no id give `None`.
fn app_at_path(path: &Path) -> Option<AppInfo> {
    let path_str = NSString::from_str(path.to_str()?);
    let bundle_id = NSBundle::bundleWithPath(&path_str)?.bundleIdentifier()?.to_string();
    if bundle_id.is_empty() {
        return None;
    }
    let display = NSFileManager::defaultManager().displayNameAtPath(&path_str).to_string();
    let name = display.strip_suffix(".app").unwrap_or(&display).to_string();
    Some(AppInfo { bundle_id, name, pid: 0 })
}

/// Remove duplicate bundle ids (the first one wins) and sort by lowercase name.
/// Apps with an empty bundle id are dropped.
fn dedup_and_sort(apps: Vec<AppInfo>) -> Vec<AppInfo> {
    let mut seen = HashSet::new();
    let mut apps: Vec<AppInfo> = apps
        .into_iter()
        .filter(|a| !a.bundle_id.is_empty() && seen.insert(a.bundle_id.clone()))
        .collect();
    apps.sort_by_cached_key(|a| a.name.to_lowercase());
    apps
}

/// Apps installed in the usual folders, plus the apps that run now.
pub fn installed_apps() -> Vec<AppInfo> {
    let mut apps: Vec<AppInfo> =
        collect_app_paths(&app_roots()).iter().filter_map(|p| app_at_path(p)).collect();
    // An app in an unusual folder shows up when it runs. Installed ones come first and win.
    // Only apps with a Dock icon: the helpers of other apps are not a place to dictate into.
    apps.extend(
        NSWorkspace::sharedWorkspace()
            .runningApplications()
            .iter()
            .filter(|app| app.activationPolicy() == NSApplicationActivationPolicy::Regular)
            .map(|app| app_info(&app)),
    );
    dedup_and_sort(apps)
}

impl ContextProvider for MacContext {
    fn frontmost_app(&self) -> Option<AppInfo> {
        frontmost_app()
    }

    fn running_apps(&self) -> Vec<AppInfo> {
        running_apps()
    }

    fn app_icon_png(&self, bundle_id: &str, size: u32) -> Option<Vec<u8>> {
        let image = icon_for_bundle_id(bundle_id)?;
        render_png(&image, size)
    }

    fn installed_apps(&self) -> Vec<AppInfo> {
        installed_apps()
    }

    fn reads_page_url(&self) -> bool {
        true
    }

    fn page_url(&self, app: &AppInfo) -> Option<String> {
        focused_window_document(app.pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(bundle_id: &str, name: &str) -> AppInfo {
        AppInfo { bundle_id: bundle_id.into(), name: name.into(), pid: 0 }
    }

    #[test]
    fn dedup_keeps_first_drops_empty_and_sorts_by_lowercase_name() {
        let apps = vec![
            info("b", "banana"),
            info("a", "Zed"),
            info("b", "Other banana"),
            info("", "No id"),
            info("c", "apple"),
        ];
        let out = dedup_and_sort(apps);
        let names: Vec<_> = out.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["apple", "banana", "Zed"]);
    }

    #[test]
    fn collect_app_paths_reads_roots_and_one_level_of_subfolders() {
        let base = std::env::temp_dir().join(format!("sayso-apps-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("root");
        let flat = base.join("flat");
        for dir in [
            root.join("A.app"),
            root.join("Setapp/B.app"),
            root.join("Setapp/deeper/C.app"),
            root.join("notes"),
            flat.join("D.app"),
            flat.join("sub/E.app"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(root.join("file.app"), b"not a folder").unwrap();

        let found = collect_app_paths(&[(root.clone(), true), (flat.clone(), false), (base.join("missing"), true)]);
        std::fs::remove_dir_all(&base).unwrap();

        assert_eq!(found, [root.join("A.app"), root.join("Setapp/B.app"), flat.join("D.app")]);
    }
}
