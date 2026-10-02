//! objc2 glue for things GPUI does not expose.
use gpui_kit::Window;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSColor, NSView, NSWindow, NSWorkspace,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// The NSWindow (an NSPanel for `WindowKind::PopUp`) behind a GPUI window.
pub fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
    let RawWindowHandle::AppKit(h) = HasWindowHandle::window_handle(window).ok()?.as_raw() else {
        return None;
    };
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    view.window()
}

pub fn frontmost_bundle_id() -> String {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .and_then(|app| app.bundleIdentifier())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "<none>".into())
}

/// GPUI sets the Regular policy in `applicationDidFinishLaunching`; undo it.
/// Returns the policy read back (0 = Regular, 1 = Accessory, 2 = Prohibited).
pub fn become_accessory(mtm: MainThreadMarker) -> isize {
    let app = NSApplication::sharedApplication(mtm);
    let ok = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    println!("setActivationPolicy(accessory) returned {ok}");
    app.activationPolicy().0
}

/// Candidate fix for the transparent-PopUp border (zed #61508).
/// `shadow_only` isolates which call matters.
pub fn clear_window_chrome(win: &NSWindow, shadow_only: bool) {
    win.setHasShadow(false);
    if !shadow_only {
        win.setOpaque(false);
        win.setBackgroundColor(Some(&NSColor::clearColor()));
    }
}

/// `NSApp.isActive` (is our process the active app?).
pub fn app_is_active(mtm: MainThreadMarker) -> bool {
    NSApplication::sharedApplication(mtm).isActive()
}

/// Give up "active app" status (macOS 14+ `NSApp.deactivate`). Does not touch the frontmost app.
pub fn deactivate_app(mtm: MainThreadMarker) {
    NSApplication::sharedApplication(mtm).deactivate();
}

/// Current activation policy (0 = Regular, 1 = Accessory, 2 = Prohibited).
pub fn policy(mtm: MainThreadMarker) -> isize {
    NSApplication::sharedApplication(mtm).activationPolicy().0
}
