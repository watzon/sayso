//! X11 helpers for the UI: move, show, hide, and shape windows, read the
//! screens and the mouse.
//!
//! They work when GPUI uses X11 (an X11 session, or XWayland on GNOME). On a
//! Wayland session they reach XWayland only, so the mouse position and the
//! active window are only right over X11 windows.
//!
//! Coordinates follow the convention of the macOS helpers: logical points,
//! origin at the bottom left of the X11 screen, y up. [`set_scale`] gives the
//! factor between logical points and X11 pixels; the UI takes it from GPUI.

use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    self, AtomEnum, ClipOrdering, ConfigureWindowAux, ConnectionExt as _, InputFocus, Rectangle, StackMode,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenInfo {
    /// The RandR monitor name atom. Stable while the monitor stays connected.
    pub id: u32,
    pub frame: Rect,
    /// The frame minus panels and docks (`_NET_WORKAREA`).
    pub visible_frame: Rect,
    pub scale: f64,
    /// Always false on Linux.
    pub has_notch: bool,
}

/// Keeps a click monitor alive. Linux has none: the popover closes when it
/// loses focus.
pub struct MonitorToken;

impl MonitorToken {
    pub fn is_active(&self) -> bool {
        false
    }
}

struct X11 {
    conn: RustConnection,
    root: xproto::Window,
    /// Root size in X11 pixels.
    width: u16,
    height: u16,
}

static X: Mutex<Option<X11>> = Mutex::new(None);
/// Logical-to-pixel factor, as f64 bits. 1.0 until the UI sets it.
static SCALE: AtomicU64 = AtomicU64::new(0x3FF0_0000_0000_0000);

/// Set the factor between logical points and X11 pixels (GPUI's scale factor).
pub fn set_scale(scale: f64) {
    if scale.is_finite() && scale > 0.0 {
        SCALE.store(scale.to_bits(), Ordering::Relaxed);
    }
}

/// The factor between logical points and X11 pixels.
pub fn scale() -> f64 {
    f64::from_bits(SCALE.load(Ordering::Relaxed))
}

/// Run `f` with the shared connection. None when no X server is reachable.
fn with_x<T>(f: impl FnOnce(&X11) -> Option<T>) -> Option<T> {
    let mut guard = X.lock();
    if guard.is_none() {
        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let screen = conn.setup().roots.get(screen_num)?;
        let (root, width, height) = (screen.root, screen.width_in_pixels, screen.height_in_pixels);
        *guard = Some(X11 { conn, root, width, height });
    }
    let x = guard.as_ref()?;
    let result = f(x);
    let _ = x.conn.flush();
    result
}

/// Height of the X11 screen in points, for converting between bottom-left
/// and top-left coordinates.
pub fn primary_screen_height() -> f64 {
    with_x(|x| Some(f64::from(x.height) / scale())).unwrap_or(1080.0)
}

/// A rectangle in X11 pixels (top-left) to points (bottom-left).
fn to_points(x: i32, y: i32, w: u32, h: u32, root_h: u16) -> Rect {
    let s = scale();
    Rect {
        x: f64::from(x) / s,
        y: (f64::from(root_h) - f64::from(y) - f64::from(h)) / s,
        width: f64::from(w) / s,
        height: f64::from(h) / s,
    }
}

/// The intersection of two rectangles, or `a` when they do not meet.
pub fn intersect(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    if x1 <= x0 || y1 <= y0 { a } else { Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 } }
}

/// All monitors, primary first.
pub fn screens() -> Vec<ScreenInfo> {
    with_x(|x| {
        let monitors = x.conn.randr_get_monitors(x.root, true).ok()?.reply().ok()?;
        let work = workarea(x);
        let mut list: Vec<(bool, ScreenInfo)> = monitors
            .monitors
            .iter()
            .map(|m| {
                let frame = to_points(i32::from(m.x), i32::from(m.y), u32::from(m.width), u32::from(m.height), x.height);
                let visible_frame = work.map_or(frame, |w| intersect(frame, w));
                (m.primary, ScreenInfo { id: m.name, frame, visible_frame, scale: scale(), has_notch: false })
            })
            .collect();
        if list.is_empty() {
            let frame = to_points(0, 0, u32::from(x.width), u32::from(x.height), x.height);
            let visible_frame = work.map_or(frame, |w| intersect(frame, w));
            list.push((true, ScreenInfo { id: 0, frame, visible_frame, scale: scale(), has_notch: false }));
        }
        list.sort_by_key(|(primary, _)| !*primary);
        Some(list.into_iter().map(|(_, s)| s).collect())
    })
    .unwrap_or_default()
}

/// The work area of the current desktop, in points.
fn workarea(x: &X11) -> Option<Rect> {
    let atom = intern(x, b"_NET_WORKAREA")?;
    let reply = x.conn.get_property(false, x.root, atom, AtomEnum::CARDINAL, 0, 4).ok()?.reply().ok()?;
    let v: Vec<u32> = reply.value32()?.collect();
    let [wx, wy, ww, wh] = v[..] else { return None };
    Some(to_points(wx as i32, wy as i32, ww, wh, x.height))
}

fn intern(x: &X11, name: &[u8]) -> Option<xproto::Atom> {
    Some(x.conn.intern_atom(false, name).ok()?.reply().ok()?.atom)
}

pub fn mouse_location() -> Point {
    with_x(|x| {
        let p = x.conn.query_pointer(x.root).ok()?.reply().ok()?;
        let s = scale();
        Some(Point { x: f64::from(p.root_x) / s, y: (f64::from(x.height) - f64::from(p.root_y)) / s })
    })
    .unwrap_or(Point { x: 0.0, y: 0.0 })
}

/// True while the left mouse button is down.
pub fn mouse_button_down() -> bool {
    with_x(|x| {
        let p = x.conn.query_pointer(x.root).ok()?.reply().ok()?;
        Some(p.mask.contains(xproto::KeyButMask::BUTTON1))
    })
    .unwrap_or(false)
}

/// The window frame in points.
pub fn frame(window: u32) -> Option<Rect> {
    with_x(|x| {
        let g = x.conn.get_geometry(window).ok()?.reply().ok()?;
        let t = x.conn.translate_coordinates(window, x.root, 0, 0).ok()?.reply().ok()?;
        Some(to_points(i32::from(t.dst_x), i32::from(t.dst_y), u32::from(g.width), u32::from(g.height), x.height))
    })
}

/// Move the window so its bottom-left corner is at (x, y) points.
pub fn set_frame_origin(window: u32, x: f64, y: f64) {
    with_x(|c| {
        let g = c.conn.get_geometry(window).ok()?.reply().ok()?;
        let s = scale();
        let top = f64::from(c.height) - y * s - f64::from(g.height);
        let aux = ConfigureWindowAux::new().x((x * s).round() as i32).y(top.round() as i32);
        c.conn.configure_window(window, &aux).ok()?;
        Some(())
    });
}

/// Move and resize the window. (x, y) is the bottom-left corner in points.
pub fn set_frame(window: u32, x: f64, y: f64, width: f64, height: f64) {
    with_x(|c| {
        let s = scale();
        let top = f64::from(c.height) - (y + height) * s;
        let aux = ConfigureWindowAux::new()
            .x((x * s).round() as i32)
            .y(top.round() as i32)
            .width((width * s).round().max(1.0) as u32)
            .height((height * s).round().max(1.0) as u32);
        c.conn.configure_window(window, &aux).ok()?;
        Some(())
    });
}

/// Limit where the window takes the mouse to `rect` (window-local points,
/// top-left origin). An empty rect takes no input; None resets to the whole
/// window. Without a compositor the window's visible shape follows too, so
/// its clear parts do not draw black.
pub fn set_input_rect(window: u32, rect: Option<Rect>) {
    let compositor = has_compositor();
    with_x(|c| {
        match rect {
            None => {
                c.conn.shape_mask(shape::SO::SET, shape::SK::INPUT, window, 0, 0, x11rb::NONE).ok()?;
            }
            Some(r) => {
                let s = scale();
                let rects = if r.width <= 0.0 || r.height <= 0.0 {
                    vec![]
                } else {
                    vec![Rectangle {
                        x: (r.x * s).floor() as i16,
                        y: (r.y * s).floor() as i16,
                        width: (r.width * s).ceil() as u16,
                        height: (r.height * s).ceil() as u16,
                    }]
                };
                c.conn
                    .shape_rectangles(shape::SO::SET, shape::SK::INPUT, ClipOrdering::UNSORTED, window, 0, 0, &rects)
                    .ok()?;
                if !compositor {
                    c.conn
                        .shape_rectangles(shape::SO::SET, shape::SK::BOUNDING, ClipOrdering::UNSORTED, window, 0, 0, &rects)
                        .ok()?;
                }
            }
        }
        Some(())
    });
}

/// True when a compositing manager runs, so transparent windows blend.
pub fn has_compositor() -> bool {
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut cache = CACHE.lock();
    if let Some((at, value)) = *cache
        && at.elapsed() < Duration::from_secs(10)
    {
        return value;
    }
    let value = with_x(|x| {
        let atom = intern(x, b"_NET_WM_CM_S0")?;
        Some(x.conn.get_selection_owner(atom).ok()?.reply().ok()?.owner != x11rb::NONE)
    })
    .unwrap_or(false);
    *cache = Some((Instant::now(), value));
    value
}

/// Show the window above the others without giving it focus.
pub fn show(window: u32) {
    with_x(|x| {
        x.conn.map_window(window).ok()?;
        x.conn.configure_window(window, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)).ok()?;
        Some(())
    });
}

/// Put the window above the others again (an override-redirect window can
/// end up below a window that was raised later).
pub fn raise(window: u32) {
    with_x(|x| {
        x.conn.configure_window(window, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)).ok()?;
        Some(())
    });
}

pub fn hide(window: u32) {
    with_x(|x| {
        x.conn.unmap_window(window).ok()?;
        Some(())
    });
}

/// Show the window and give it the keyboard. An override-redirect window
/// (GPUI's pop-up) gets no focus from the window manager on its own.
pub fn show_and_focus(window: u32) {
    show(window);
    with_x(|x| {
        // The window must be viewable before it can take focus.
        x.conn.sync().ok()?;
        x.conn.set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME).ok()?;
        Some(())
    });
}

/// Keep `child` above `parent` (a list that opens from a window).
pub fn set_transient_for(child: u32, parent: u32) {
    with_x(|x| {
        x.conn
            .change_property32(xproto::PropMode::REPLACE, child, AtomEnum::WM_TRANSIENT_FOR, AtomEnum::WINDOW, &[parent])
            .ok()?;
        Some(())
    });
}

/// Best effort: the active window is in full screen (`_NET_WM_STATE_FULLSCREEN`).
pub fn active_window_is_fullscreen() -> bool {
    with_x(|x| {
        let active_atom = intern(x, b"_NET_ACTIVE_WINDOW")?;
        let reply = x.conn.get_property(false, x.root, active_atom, AtomEnum::WINDOW, 0, 1).ok()?.reply().ok()?;
        let active = reply.value32()?.next().filter(|w| *w != 0)?;
        let state_atom = intern(x, b"_NET_WM_STATE")?;
        let full = intern(x, b"_NET_WM_STATE_FULLSCREEN")?;
        let states = x.conn.get_property(false, active, state_atom, AtomEnum::ATOM, 0, 32).ok()?.reply().ok()?;
        Some(states.value32()?.any(|a| a == full))
    })
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersect_keeps_the_common_part() {
        let a = Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
        let b = Rect { x: 50.0, y: 20.0, width: 100.0, height: 50.0 };
        assert_eq!(intersect(a, b), Rect { x: 50.0, y: 20.0, width: 50.0, height: 50.0 });
        let far = Rect { x: 500.0, y: 500.0, width: 10.0, height: 10.0 };
        assert_eq!(intersect(a, far), a, "no overlap keeps the first rectangle");
    }

    #[test]
    fn pixels_flip_to_bottom_left_points() {
        set_scale(1.0);
        // A 100x50 window at the top left of a 1000 px high screen.
        assert_eq!(to_points(0, 0, 100, 50, 1000), Rect { x: 0.0, y: 950.0, width: 100.0, height: 50.0 });
    }
}
