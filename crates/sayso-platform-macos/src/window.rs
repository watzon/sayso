//! AppKit helpers for the UI, built on objc2.
//!
//! Window functions take a raw pointer from the `raw-window-handle` AppKit
//! handle. It may point at an `NSView` (what GPUI gives) or at an `NSWindow`.
//! The pointer must be live and the call must happen on the main thread.
//! A null pointer, a wrong thread, or an object of another class makes the
//! call do nothing.
//!
//! All coordinates are Cocoa screen coordinates: origin at the bottom left of
//! the primary display, y up, in points.

use crate::ffi::*;
use block2::RcBlock;
use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_foundation::NSObjectProtocol;
use objc2_app_kit::NSAccessibility as _;
use objc2::runtime::NSObject;
use objc2_app_kit::{
    NSBox, NSBoxType, NSTitlePosition,
    NSApplication, NSApplicationActivationPolicy, NSColor, NSEvent, NSEventMask, NSPanel, NSScreen,
    NSStatusItem, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowOrderingMode, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::OnceLock;

/// Window level of the menu bar's status items. Above normal and full-screen
/// app windows, below pop-up menus.
pub const OVERLAY_LEVEL: isize = 25;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<NSRect> for Rect {
    fn from(r: NSRect) -> Self {
        Rect { x: r.origin.x, y: r.origin.y, width: r.size.width, height: r.size.height }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenInfo {
    /// `CGDirectDisplayID`. Stable while the display stays connected.
    pub id: u32,
    pub frame: Rect,
    /// The frame minus the menu bar and the Dock.
    pub visible_frame: Rect,
    /// Backing scale (2.0 on Retina displays).
    pub scale: f64,
    /// The camera housing (notch) in Cocoa screen coordinates, on a display
    /// that has one.
    pub notch: Option<Rect>,
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

/// Hide the Dock icon and the app menu. GPUI sets the Regular policy at
/// launch, so call this at the start of the app's run closure.
/// Returns false when it is not on the main thread.
pub fn become_accessory() -> bool {
    set_dock_icon_visible(false)
}

/// Show the Dock icon (Regular policy) or hide it (Accessory policy).
/// Returns false when the policy change was refused or the thread is wrong.
pub fn set_dock_icon_visible(visible: bool) -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    let policy = if visible {
        NSApplicationActivationPolicy::Regular
    } else {
        NSApplicationActivationPolicy::Accessory
    };
    NSApplication::sharedApplication(mtm).setActivationPolicy(policy)
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// The window behind a pointer to an `NSWindow` or an `NSView`.
fn resolve(ptr: *mut c_void) -> Option<Retained<NSWindow>> {
    MainThreadMarker::new()?;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the caller promises a live Objective-C object. `isKindOfClass:`
    // is the check that the class is what we cast to.
    unsafe {
        let object: &AnyObject = &*ptr.cast::<AnyObject>();
        if let Some(window) = object.downcast_ref::<NSWindow>() {
            Some(window.retain())
        } else {
            object.downcast_ref::<NSView>().and_then(NSView::window)
        }
    }
}

/// Make a window behave as a non-activating overlay: no frame, no shadow, clear
/// background, status-bar level, on every Space, over full-screen apps, out of
/// Cmd+Tab and window cycling.
///
/// GPUI's panel class says yes to `canBecomeKeyWindow` and Rust cannot
/// override that. So the window is also set to become key only when a control
/// needs it, and it does not activate the app on click. Show it with
/// [`order_front_without_activating`].
pub fn configure_overlay(window: *mut c_void) {
    let Some(win) = resolve(window) else { return };
    // GPUI makes a window without a title bar a titled window with a
    // transparent title bar. macOS 26 draws a light line along the top edge
    // of a titled window, even a clear one, so the overlay must be borderless.
    win.setStyleMask(win.styleMask() & !NSWindowStyleMask::Titled);
    win.setHasShadow(false);
    win.setOpaque(false);
    win.setBackgroundColor(Some(&NSColor::clearColor()));
    win.setHidesOnDeactivate(false);
    win.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    let any: &AnyObject = &win;
    if let Some(panel) = any.downcast_ref::<NSPanel>() {
        panel.setBecomesKeyOnlyIfNeeded(true);
        panel.setFloatingPanel(true);
        win.setStyleMask(win.styleMask() | NSWindowStyleMask::NonactivatingPanel);
    }
    // After `setFloatingPanel`, which sets the floating level. The notch must
    // be above the menu bar.
    win.setLevel(OVERLAY_LEVEL);
}

/// Let mouse events pass through the window to what is behind it.
pub fn set_ignores_mouse(window: *mut c_void, ignores: bool) {
    if let Some(win) = resolve(window) {
        win.setIgnoresMouseEvents(ignores);
    }
}

/// Move the window's bottom-left corner to a point in Cocoa screen coordinates.
pub fn set_frame_origin(window: *mut c_void, x: f64, y: f64) {
    if let Some(win) = resolve(window) {
        win.setFrameOrigin(NSPoint::new(x, y));
    }
}

/// Move and resize the window. `x` and `y` are its bottom-left corner in
/// Cocoa screen coordinates.
pub fn set_frame(window: *mut c_void, x: f64, y: f64, width: f64, height: f64) {
    if let Some(win) = resolve(window) {
        win.setFrame_display(NSRect::new(NSPoint::new(x, y), NSSize::new(width, height)), true);
    }
}

/// Make a window that draws its own sheet and shadow fully clear: no system
/// shadow and a clear background. GPUI's almost-clear background and the
/// system shadow show as a frame around the window rectangle. Same as
/// [`configure_overlay`] does for the overlay.
pub fn make_clear(window: *mut c_void) {
    if let Some(win) = resolve(window) {
        win.setHasShadow(false);
        win.setOpaque(false);
        win.setBackgroundColor(Some(&NSColor::clearColor()));
    }
}

/// Attach `child` to `parent`, above it. A child window moves with its
/// parent, so a list opened from a control stays next to that control.
pub fn add_child_window(parent: *mut c_void, child: *mut c_void) {
    let (Some(parent), Some(child)) = (resolve(parent), resolve(child)) else { return };
    // SAFETY: both are live windows on the main thread (`resolve` checks both).
    unsafe { parent.addChildWindow_ordered(&child, NSWindowOrderingMode::Above) };
}

/// Show the window on top without making it key. If it still became key
/// (the app was already active), give up key status and deactivate the app,
/// so keystrokes go back to the app the user is typing in.
pub fn order_front_without_activating(window: *mut c_void) {
    let Some(win) = resolve(window) else { return };
    win.orderFrontRegardless();
    if win.isKeyWindow() {
        win.resignKeyWindow();
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).deactivate();
        }
    }
}

/// Hide the window.
pub fn order_out(window: *mut c_void) {
    if let Some(win) = resolve(window) {
        win.orderOut(None);
    }
}

/// Show the window, make it key, and bring the app to the front.
/// For the Hub and the popover, not for the overlay.
pub fn show_and_focus(window: *mut c_void) {
    let Some(win) = resolve(window) else { return };
    win.makeKeyAndOrderFront(None);
    if let Some(mtm) = MainThreadMarker::new() {
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }
}

// ---------------------------------------------------------------------------
// Screens and mouse
// ---------------------------------------------------------------------------

/// All connected displays, primary first. Empty off the main thread.
pub fn screens() -> Vec<ScreenInfo> {
    let Some(mtm) = MainThreadMarker::new() else { return Vec::new() };
    NSScreen::screens(mtm)
        .iter()
        .map(|screen| {
            let number = screen
                .deviceDescription()
                .objectForKey(&NSString::from_str("NSScreenNumber"))
                .and_then(|n| n.downcast::<objc2_foundation::NSNumber>().ok())
                .map_or(0, |n| n.unsignedIntValue());
            ScreenInfo {
                id: number,
                frame: screen.frame().into(),
                visible_frame: screen.visibleFrame().into(),
                scale: screen.backingScaleFactor(),
                notch: notch_of(&screen),
            }
        })
        .collect()
}

/// The camera housing of a display, from the unobscured areas of the menu bar
/// on each side of it. None on a display without a notch.
fn notch_of(screen: &NSScreen) -> Option<Rect> {
    let height = screen.safeAreaInsets().top;
    if height <= 0.0 {
        return None;
    }
    let frame = screen.frame();
    let (left, right) = (screen.auxiliaryTopLeftArea().size.width, screen.auxiliaryTopRightArea().size.width);
    let width = frame.size.width - left - right;
    if width <= 0.0 {
        return None;
    }
    Some(Rect { x: frame.origin.x + left, y: frame.origin.y + frame.size.height - height, width, height })
}

/// The mouse position in Cocoa screen coordinates.
pub fn mouse_location() -> Point {
    let p = NSEvent::mouseLocation();
    Point { x: p.x, y: p.y }
}

/// Screen frame of a status item's button, for anchoring a popover under it.
/// `item` is the pointer of tray-icon's `ns_status_item()`.
/// Read it at click time. Right after creation the frame is still empty.
pub fn status_item_screen_rect(item: *mut c_void) -> Option<Rect> {
    let mtm = MainThreadMarker::new()?;
    if item.is_null() {
        return None;
    }
    // SAFETY: the caller promises a live `NSStatusItem`.
    let item = unsafe { &*item.cast::<NSStatusItem>() };
    let window = item.button(mtm)?.window()?;
    Some(window.frame().into())
}

/// What a press of the menu bar icon by an assistive client does.
pub struct StatusPressState {
    on_press: Box<dyn Fn()>,
}

define_class!(
    // The target of the menu bar button. A mouse click never reaches it,
    // because the tray view on top of the button takes the mouse. VoiceOver,
    // Voice Control, and other assistive clients press the button itself, and
    // that sends this action.
    //
    // SAFETY: `NSObject` has no subclassing requirements, and the ivar is a
    // plain Rust value that is only used on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SaysoStatusPress"]
    #[ivars = StatusPressState]
    pub struct StatusPress;

    impl StatusPress {
        #[unsafe(method(press:))]
        fn press(&self, _sender: Option<&AnyObject>) {
            (self.ivars().on_press)();
        }
    }

    unsafe impl NSObjectProtocol for StatusPress {}
);

/// Give the menu bar icon an accessible name and an action for an assistive
/// client. Keep the returned target alive as long as the icon.
pub fn make_status_item_accessible(item: *mut c_void, label: &str, on_press: impl Fn() + 'static) -> Option<Retained<StatusPress>> {
    let mtm = MainThreadMarker::new()?;
    if item.is_null() {
        return None;
    }
    // SAFETY: the caller promises a live `NSStatusItem`.
    let item = unsafe { &*item.cast::<NSStatusItem>() };
    let button = item.button(mtm)?;
    let target = StatusPress::alloc(mtm).set_ivars(StatusPressState { on_press: Box::new(on_press) });
    // SAFETY: `init` of `NSObject` on a new allocation.
    let target: Retained<StatusPress> = unsafe { msg_send![super(target), init] };
    // SAFETY: the target lives as long as the caller keeps it, and `press:`
    // has the signature of an action.
    unsafe {
        button.setTarget(Some(&target));
        button.setAction(Some(objc2::sel!(press:)));
    }
    button.setAccessibilityLabel(Some(&NSString::from_str(label)));
    Some(target)
}

/// Change the name that an assistive client reads for the menu bar icon.
pub fn set_status_item_label(item: *mut c_void, label: &str) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    if item.is_null() {
        return;
    }
    // SAFETY: the caller promises a live `NSStatusItem`.
    let item = unsafe { &*item.cast::<NSStatusItem>() };
    if let Some(button) = item.button(mtm) {
        button.setAccessibilityLabel(Some(&NSString::from_str(label)));
    }
}

/// A round dot at the top right of the menu bar icon. The icon stays a
/// template image. Dropping the value removes the dot.
pub struct StatusBadge(Retained<NSBox>);

impl StatusBadge {
    /// The size of the dot and of its ring, in points.
    const DOT: f64 = 7.;
    const RING: f64 = 1.5;
    /// The space to the top and to the right of the icon box.
    const INSET: f64 = 2.;

    pub fn add(item: *mut c_void, dot: [u8; 3], ring: [u8; 3]) -> Option<StatusBadge> {
        let mtm = MainThreadMarker::new()?;
        if item.is_null() {
            return None;
        }
        // SAFETY: the caller promises a live `NSStatusItem`.
        let item = unsafe { &*item.cast::<NSStatusItem>() };
        let button = item.button(mtm)?;
        let color = |[r, g, b]: [u8; 3]| NSColor::colorWithSRGBRed_green_blue_alpha(f64::from(r) / 255., f64::from(g) / 255., f64::from(b) / 255., 1.);
        let side = Self::DOT + 2. * Self::RING;
        let bounds = button.bounds();
        let y = if button.isFlipped() { Self::INSET } else { bounds.size.height - side - Self::INSET };
        let frame = NSRect::new(NSPoint::new(bounds.size.width - side - Self::INSET, y), NSSize::new(side, side));
        let badge = NSBox::initWithFrame(NSBox::alloc(mtm), frame);
        badge.setBoxType(NSBoxType::Custom);
        badge.setTitlePosition(NSTitlePosition::NoTitle);
        badge.setFillColor(&color(dot));
        badge.setBorderColor(&color(ring));
        badge.setBorderWidth(Self::RING);
        badge.setCornerRadius(side / 2.);
        // Under the view of the icon that takes the clicks, so a click on the dot still counts.
        button.addSubview_positioned_relativeTo(&badge, NSWindowOrderingMode::Below, None);
        Some(StatusBadge(badge))
    }
}

impl Drop for StatusBadge {
    fn drop(&mut self) {
        self.0.removeFromSuperview();
    }
}

/// Keeps a global mouse monitor alive. Dropping it removes the monitor.
pub struct MonitorToken {
    monitor: Option<Retained<AnyObject>>,
}

impl Drop for MonitorToken {
    fn drop(&mut self) {
        if let Some(monitor) = self.monitor.take() {
            // SAFETY: the object came from `addGlobalMonitor...` and is removed once.
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
    }
}

impl MonitorToken {
    /// False when macOS refused to create the monitor.
    pub fn is_active(&self) -> bool {
        self.monitor.is_some()
    }
}

/// Call `callback` with the screen position of every left or right mouse
/// press in other apps (for example to close a popover). Presses inside our
/// own windows are not reported. Call on the main thread; the callback runs there.
pub fn install_global_click_monitor(callback: impl Fn(Point) + 'static) -> MonitorToken {
    let block = RcBlock::new(move |_event: NonNull<NSEvent>| callback(mouse_location()));
    let monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
        NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown,
        &block,
    );
    if monitor.is_none() {
        log::warn!("macOS refused to create the global click monitor");
    }
    MonitorToken { monitor }
}

// ---------------------------------------------------------------------------
// Full-screen detection
// ---------------------------------------------------------------------------

/// Whether `bounds` covers a whole display (all values in the same
/// coordinate space, within one point).
pub fn covers_a_display(bounds: Rect, displays: &[Rect]) -> bool {
    const TOLERANCE: f64 = 1.0;
    displays.iter().any(|d| {
        (bounds.x - d.x).abs() <= TOLERANCE
            && (bounds.y - d.y).abs() <= TOLERANCE
            && (bounds.width - d.width).abs() <= TOLERANCE
            && (bounds.height - d.height).abs() <= TOLERANCE
    })
}

/// Best effort: the front app's topmost normal window fills a display.
/// Works on any thread. A window that fills the screen without being in
/// native full screen (a game, a video player) counts too.
pub fn frontmost_app_is_fullscreen() -> bool {
    frontmost_window_bounds().is_some_and(|bounds| covers_a_display(bounds, &display_bounds()))
}

/// The frame of the front app's topmost normal window, in Cocoa screen
/// coordinates. Works on any thread.
pub fn frontmost_window_frame() -> Option<Rect> {
    let b = frontmost_window_bounds()?;
    // The primary display has its origin at zero in both coordinate systems.
    let primary_height = display_bounds().iter().find(|d| d.x == 0.0 && d.y == 0.0)?.height;
    Some(Rect { x: b.x, y: primary_height - b.y - b.height, width: b.width, height: b.height })
}

/// The bounds of the front app's topmost normal window, in CoreGraphics
/// coordinates (origin top left).
fn frontmost_window_bounds() -> Option<Rect> {
    let pid = NSWorkspace::sharedWorkspace().frontmostApplication().map(|a| a.processIdentifier())?;
    // SAFETY: "Copy" rule, so we own the array. Entries are CFDictionary.
    let raw = unsafe {
        CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID)
    };
    if raw.is_null() {
        return None;
    }
    let windows: CFArray<CFDictionary<CFString, CFType>> = unsafe { CFArray::wrap_under_create_rule(raw.cast()) };
    let number = |w: &CFDictionary<CFString, CFType>, key: &str| {
        w.find(CFString::new(key)).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i64())
    };
    // The list is ordered front to back. The first layer-0 window of the app is its top window.
    for window in windows.iter() {
        if number(&window, "kCGWindowOwnerPID") != Some(i64::from(pid)) || number(&window, "kCGWindowLayer") != Some(0) {
            continue;
        }
        let dict = window.find(CFString::new("kCGWindowBounds")).and_then(|v| v.downcast::<CFDictionary>())?;
        let mut rect = CGRect::default();
        // SAFETY: `dict` is a bounds dictionary and `rect` a valid out-pointer.
        if !unsafe { CGRectMakeWithDictionaryRepresentation(dict.as_concrete_TypeRef().cast(), &mut rect) } {
            return None;
        }
        return Some(Rect { x: rect.origin.x, y: rect.origin.y, width: rect.size.width, height: rect.size.height });
    }
    None
}

/// Display bounds in CoreGraphics coordinates (origin top left), as used by the window list.
fn display_bounds() -> Vec<Rect> {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    // SAFETY: the buffer holds 16 ids and `count` receives how many were written.
    if unsafe { CGGetActiveDisplayList(16, ids.as_mut_ptr(), &mut count) } != 0 {
        return Vec::new();
    }
    ids[..count as usize]
        .iter()
        .map(|id| {
            // SAFETY: plain query for an id we just got.
            let b = unsafe { CGDisplayBounds(*id) };
            Rect { x: b.origin.x, y: b.origin.y, width: b.size.width, height: b.size.height }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Added for the app shell
// ---------------------------------------------------------------------------

/// True while the left mouse button is down (for dragging the pill).
pub fn mouse_button_down() -> bool {
    NSEvent::pressedMouseButtons() & 1 == 1
}

/// Bring the app to the front, for the Hub and onboarding.
pub fn show_app_and_activate() {
    if let Some(mtm) = MainThreadMarker::new() {
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }
}

/// Give focus back to the app the user was in (after the popover closes).
pub fn deactivate_app() {
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).deactivate();
    }
}

/// Height of the primary display, for converting Cocoa (bottom-left) and
/// GPUI (top-left) coordinates.
pub fn primary_screen_height() -> f64 {
    screens().first().map_or(1080.0, |s| s.frame.height)
}

/// The window frame in Cocoa screen coordinates.
pub fn frame(window: *mut c_void) -> Option<Rect> {
    resolve(window).map(|w| w.frame().into())
}

/// Make one window unable to become key or main, so it can never take
/// keyboard focus from the app the user types in.
///
/// GPUI's panel class answers yes to `canBecomeKeyWindow`. The class gets a
/// hook that answers no for a window with the flag. Other GPUI windows keep
/// the normal behavior.
pub fn make_never_key(window: *mut c_void) {
    use objc2::sel;

    let Some(win) = resolve(window) else { return };
    extern "C-unwind" fn can_become_key(this: *mut AnyObject, cmd: Sel) -> Bool {
        if has_flag(this, &NEVER_KEY) {
            return Bool::NO;
        }
        // SAFETY: the original has the signature of `- (BOOL)canBecomeKeyWindow`.
        unsafe { original::<extern "C-unwind" fn(*mut AnyObject, Sel) -> Bool>(&KEY_ORIGINAL)(this, cmd) }
    }
    extern "C-unwind" fn can_become_main(this: *mut AnyObject, cmd: Sel) -> Bool {
        if has_flag(this, &NEVER_KEY) {
            return Bool::NO;
        }
        // SAFETY: the original has the signature of `- (BOOL)canBecomeMainWindow`.
        unsafe { original::<extern "C-unwind" fn(*mut AnyObject, Sel) -> Bool>(&MAIN_ORIGINAL)(this, cmd) }
    }
    // SAFETY: the hooks have the signatures of the methods they replace.
    let hooked = unsafe {
        hook(&win, sel!(canBecomeKeyWindow), std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel) -> Bool, Imp>(can_become_key), &KEY_ORIGINAL)
            && hook(&win, sel!(canBecomeMainWindow), std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel) -> Bool, Imp>(can_become_main), &MAIN_ORIGINAL)
    };
    if hooked {
        set_flag(&win, &NEVER_KEY);
    }
    win.resignKeyWindow();
}

/// Let one window go over the menu bar, for the notch at the top edge of
/// the display. AppKit moves a window down out of the menu bar each time its
/// frame changes, also a borderless window at the status level. The class
/// gets a hook that keeps the frame as it is for a window with the flag.
pub fn allow_over_menu_bar(window: *mut c_void) {
    use objc2::sel;

    let Some(win) = resolve(window) else { return };
    type Constrain = extern "C-unwind" fn(*mut AnyObject, Sel, NSRect, *mut AnyObject) -> NSRect;
    extern "C-unwind" fn constrain(this: *mut AnyObject, cmd: Sel, frame: NSRect, screen: *mut AnyObject) -> NSRect {
        if has_flag(this, &OVER_MENU_BAR) {
            return frame;
        }
        // SAFETY: the original has the signature of
        // `- (NSRect)constrainFrameRect:(NSRect)frameRect toScreen:(NSScreen *)screen`.
        unsafe { original::<Constrain>(&CONSTRAIN_ORIGINAL)(this, cmd, frame, screen) }
    }
    // SAFETY: the hook has the signature of the method it replaces.
    let hooked = unsafe { hook(&win, sel!(constrainFrameRect:toScreen:), std::mem::transmute::<Constrain, Imp>(constrain), &CONSTRAIN_ORIGINAL) };
    if hooked {
        set_flag(&win, &OVER_MENU_BAR);
    }
}

// The window hooks.
//
// Do not change the class of a live window to change its behavior. AppKit
// observes windows with key-value observing, which gives the window a hidden
// subclass. A new class drops that subclass, and the window throws an
// exception when AppKit removes its observer at dealloc. A hook on the class
// stays in place under the hidden subclass.

/// A hooked method of one class: the class and the implementation it had.
type Original = OnceLock<(usize, Imp)>;

static KEY_ORIGINAL: Original = OnceLock::new();
static MAIN_ORIGINAL: Original = OnceLock::new();
static CONSTRAIN_ORIGINAL: Original = OnceLock::new();

/// Keys of the per-window flags. Only their addresses count.
static NEVER_KEY: u8 = 0;
static OVER_MENU_BAR: u8 = 1;

/// Replace the method `sel` of the window's class with `imp`, one time. The
/// hook calls the method it replaced through `slot`. False when the hook is
/// on a different class already, which Sayso does not need.
///
/// # Safety
///
/// `imp` must have the signature of the method `sel`.
unsafe fn hook(win: &NSWindow, sel: Sel, imp: Imp, slot: &'static Original) -> bool {
    // `-class`, not the real class: under key-value observing the real class
    // is the hidden subclass, and that subclass inherits the hook.
    let class: &AnyClass = win.class();
    let class_ptr = class as *const AnyClass;
    let (hooked_class, _) = *slot.get_or_init(|| {
        // SAFETY: the class and the selector are valid. The method may come
        // from a superclass. `class_addMethod` then overrides it in this class
        // only; otherwise the class has its own method, which gets the hook.
        unsafe {
            let method = objc2::ffi::class_getInstanceMethod(class_ptr, sel);
            let types = objc2::ffi::method_getTypeEncoding(method);
            let before = objc2::ffi::method_getImplementation(method).expect("a method has an implementation");
            if !objc2::ffi::class_addMethod(class_ptr as *mut AnyClass, sel, imp, types).as_bool() {
                objc2::ffi::method_setImplementation(method, imp);
            }
            (class_ptr as usize, before)
        }
    });
    if hooked_class != class_ptr as usize {
        log::warn!("the window hook for {sel:?} is on another class");
        return false;
    }
    true
}

/// The implementation that a hook replaced.
///
/// # Safety
///
/// `F` must be the signature of the hooked method.
unsafe fn original<F: Copy>(slot: &'static Original) -> F {
    let (_, imp) = *slot.get().expect("a hook runs only after it is installed");
    // SAFETY: the caller gives the signature of the hooked method.
    unsafe { std::mem::transmute_copy::<Imp, F>(&imp) }
}

fn set_flag(win: &NSWindow, key: &'static u8) {
    let object = win as *const NSWindow as *mut AnyObject;
    // SAFETY: the window is live. The value is only compared with null and
    // never retained (ASSIGN), so any pointer that is not null works.
    unsafe { objc2::ffi::objc_setAssociatedObject(object, (key as *const u8).cast(), object, objc2::ffi::OBJC_ASSOCIATION_ASSIGN) };
}

fn has_flag(this: *mut AnyObject, key: &'static u8) -> bool {
    // SAFETY: AppKit calls the hook with a live window.
    unsafe { !objc2::ffi::objc_getAssociatedObject(this, (key as *const u8).cast()).is_null() }
}


#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: Rect = Rect { x: 0.0, y: 0.0, width: 5120.0, height: 1440.0 };
    const SECOND: Rect = Rect { x: -1920.0, y: 100.0, width: 1920.0, height: 1080.0 };

    #[test]
    fn a_window_equal_to_a_display_is_full_screen() {
        assert!(covers_a_display(MAIN, &[MAIN, SECOND]));
        assert!(covers_a_display(SECOND, &[MAIN, SECOND]));
    }

    #[test]
    fn a_maximized_window_below_the_menu_bar_is_not_full_screen() {
        let maximized = Rect { x: 0.0, y: 38.0, width: 5120.0, height: 1402.0 };
        assert!(!covers_a_display(maximized, &[MAIN, SECOND]));
    }

    #[test]
    fn a_small_window_is_not_full_screen() {
        let small = Rect { x: 100.0, y: 100.0, width: 800.0, height: 600.0 };
        assert!(!covers_a_display(small, &[MAIN]));
        assert!(!covers_a_display(MAIN, &[]));
    }

    #[test]
    fn rounding_of_one_point_is_tolerated() {
        let almost = Rect { x: 0.5, y: 0.0, width: 5119.5, height: 1440.0 };
        assert!(covers_a_display(almost, &[MAIN]));
    }

    #[test]
    fn calls_off_the_main_thread_or_with_null_do_nothing() {
        // Test threads are not the main thread, so these must return quietly.
        configure_overlay(std::ptr::null_mut());
        order_out(std::ptr::null_mut());
        assert!(screens().is_empty());
        assert_eq!(status_item_screen_rect(std::ptr::null_mut()), None);
        assert!(!become_accessory());
    }
}
