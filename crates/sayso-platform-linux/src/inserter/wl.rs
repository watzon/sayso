//! A Wayland connection to the session's compositor.

use crate::session::Session;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use wayland_client::Connection;

/// Connect to the compositor of the session. This uses
/// [`Session::wayland_display`], because Sayso clears `WAYLAND_DISPLAY` when
/// its UI runs on XWayland.
pub fn connect(session: &Session) -> Result<Connection, String> {
    let name = session.wayland_display.as_ref().ok_or("the session has no Wayland display")?;
    let mut path = PathBuf::from(name);
    if path.is_relative() {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?;
        path = PathBuf::from(runtime).join(path);
    }
    let stream = UnixStream::connect(&path).map_err(|e| format!("cannot connect to {}: {e}", path.display()))?;
    Connection::from_socket(stream).map_err(|e| e.to_string())
}
