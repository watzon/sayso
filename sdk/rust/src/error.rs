//! The error of a request.

/// An error of a request: the same type in both modes. `code` says what to do.
/// Only the SDK gives the codes `host_not_running`, `connection_lost`, and `host_closing`.
pub type Error = crate::protocol::ErrorReply;
