//! The socket of the host: a Unix socket on macOS and Linux, a named pipe on
//! Windows. Blocking I/O: one thread reads and one thread writes each
//! connection.

use std::io::{Read, Write};
use std::sync::Arc;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::Listener;

#[cfg(windows)]
pub(crate) mod windows;
#[cfg(windows)]
pub(crate) use windows::Listener;

/// One accepted connection.
pub(crate) struct Conn {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    /// Stops both directions: a blocked read returns, and the client sees the
    /// end of the connection.
    pub closer: Arc<dyn Fn() + Send + Sync>,
    pub peer: crate::Peer,
}
