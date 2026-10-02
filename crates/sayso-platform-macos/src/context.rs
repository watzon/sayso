//! [`ContextProvider`]: frontmost app, running apps, and app icons.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSBitmapImageRep, NSBitmapImageFileType, NSDeviceRGBColorSpace,
    NSGraphicsContext, NSCompositingOperation, NSImage, NSRunningApplication, NSWorkspace,
};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};
use sayso_platform::{AppInfo, ContextProvider};

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
}
