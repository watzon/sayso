//! The named pipe `\\.\pipe\sayso-host-<user SID>`, with an ACL for the user
//! only.
//!
//! The handles use overlapped I/O, and each call waits for its own result.
//! Windows runs the calls on a handle without overlapped I/O one at a time, so
//! a read that waits for the client would block each write.

use super::Conn;
use parking_lot::Mutex;
use sayso_protocol::socket::{SocketEnv, SystemEnv};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_OPERATION_ABORTED, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE, GetLastError,
    HANDLE, INVALID_HANDLE_VALUE, LocalFree, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, GetOverlappedResultEx, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT};
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE};
use windows_sys::core::BOOL;

/// A client that does not read for this long loses its connection.
const WRITE_TIMEOUT_MS: u32 = 2000;
const BUFFER_BYTES: u32 = 64 * 1024;

/// One instance of the pipe.
pub(crate) struct Pipe {
    handle: HANDLE,
    closed: AtomicBool,
}

// SAFETY: a pipe handle can be used from any thread. Overlapped calls from
// two threads at the same time are allowed.
unsafe impl Send for Pipe {}
unsafe impl Sync for Pipe {}

impl Drop for Pipe {
    fn drop(&mut self) {
        // SAFETY: the handle is open, and this is its last user.
        unsafe { CloseHandle(self.handle) };
    }
}

impl Pipe {
    pub(crate) fn handle(&self) -> HANDLE {
        self.handle
    }

    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        if self.closed.load(Ordering::SeqCst) {
            return Ok(0);
        }
        let len = buf.len().min(u32::MAX as usize) as u32;
        // SAFETY: `buf` lives until `io` has the result of the read.
        match self.io(INFINITE, |overlapped| unsafe { ReadFile(self.handle, buf.as_mut_ptr(), len, std::ptr::null_mut(), overlapped) }) {
            Ok(read) => Ok(read as usize),
            Err(e) if is_end(&e) => Ok(0),
            Err(e) => Err(e),
        }
    }

    fn write(&self, buf: &[u8]) -> io::Result<usize> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let len = buf.len().min(u32::MAX as usize) as u32;
        // SAFETY: `buf` lives until `io` has the result of the write.
        self.io(WRITE_TIMEOUT_MS, |overlapped| unsafe { WriteFile(self.handle, buf.as_ptr(), len, std::ptr::null_mut(), overlapped) }).map(|written| written as usize)
    }

    /// Start an overlapped call and wait up to `timeout_ms` for its result.
    fn io(&self, timeout_ms: u32, start: impl FnOnce(*mut OVERLAPPED) -> BOOL) -> io::Result<u32> {
        // SAFETY: the event and `overlapped` live until the call has ended:
        // each path below has the result of the call, or cancels the call and
        // waits for it.
        unsafe {
            let event = CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
            if event.is_null() {
                return Err(io::Error::last_os_error());
            }
            let _event = Handle(event);
            let mut overlapped: OVERLAPPED = std::mem::zeroed();
            overlapped.hEvent = event;
            if start(&mut overlapped) == 0 {
                let error = GetLastError();
                if error != ERROR_IO_PENDING {
                    return Err(io::Error::from_raw_os_error(error as i32));
                }
                // A shutdown that came before this call started stops it here.
                if self.closed.load(Ordering::SeqCst) {
                    CancelIoEx(self.handle, &overlapped);
                }
            }
            let mut done = 0u32;
            if GetOverlappedResultEx(self.handle, &overlapped, &mut done, timeout_ms, 0) == 0 {
                let error = GetLastError();
                if error == WAIT_TIMEOUT {
                    CancelIoEx(self.handle, &overlapped);
                    GetOverlappedResult(self.handle, &overlapped, &mut done, 1);
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "the other end does not read"));
                }
                return Err(io::Error::from_raw_os_error(error as i32));
            }
            Ok(done)
        }
    }

    /// Stop both directions. A blocked read returns the end.
    fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        // SAFETY: the handle is open while `self` lives.
        unsafe { CancelIoEx(self.handle, std::ptr::null()) };
    }
}

/// The errors that mean the end of the connection.
fn is_end(e: &io::Error) -> bool {
    matches!(e.raw_os_error().map(|code| code as u32), Some(ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_NO_DATA | ERROR_OPERATION_ABORTED))
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle is open, and this is its only owner.
        unsafe { CloseHandle(self.0) };
    }
}

struct PipeIo(Arc<Pipe>);

impl io::Read for PipeIo {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl io::Write for PipeIo {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) struct Listener {
    name: PathBuf,
    wide_name: Vec<u16>,
    /// The ACL: full access for the user, nobody else.
    sddl: Vec<u16>,
    /// The instance that the next client connects to.
    next: Mutex<Option<Arc<Pipe>>>,
}

impl Listener {
    pub fn bind(path: &Path) -> io::Result<Listener> {
        let sid = SystemEnv.user_sid().ok_or_else(|| io::Error::other("cannot find the SID of the user"))?;
        let listener = Listener { name: path.to_path_buf(), wide_name: wide(path.as_os_str()), sddl: wide(format!("D:P(A;;GA;;;{sid})").as_ref()), next: Mutex::new(None) };
        let first = listener.instance(true).map_err(|e| {
            if e.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) {
                io::Error::new(io::ErrorKind::AddrInUse, format!("another Sayso serves extensions at {}", path.display()))
            } else {
                e
            }
        })?;
        *listener.next.lock() = Some(first);
        Ok(listener)
    }

    fn instance(&self, first: bool) -> io::Result<Arc<Pipe>> {
        // SAFETY: the descriptor lives until CreateNamedPipeW has copied it,
        // and the names are NUL-terminated.
        unsafe {
            let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(self.sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut()) == 0 {
                return Err(io::Error::last_os_error());
            }
            let attributes = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: descriptor, bInheritHandle: 0 };
            let mut mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
            if first {
                mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
            }
            let handle = CreateNamedPipeW(
                self.wide_name.as_ptr(),
                mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                BUFFER_BYTES,
                BUFFER_BYTES,
                0,
                &attributes,
            );
            let error = io::Error::last_os_error();
            LocalFree(descriptor);
            if handle == INVALID_HANDLE_VALUE {
                return Err(error);
            }
            Ok(Arc::new(Pipe { handle, closed: AtomicBool::new(false) }))
        }
    }

    pub fn accept(&self) -> io::Result<Conn> {
        let waiting = self.next.lock().take();
        let pipe = match waiting {
            Some(pipe) => pipe,
            None => self.instance(false)?,
        };
        // SAFETY: the pipe lives until `io` has the result.
        match pipe.io(INFINITE, |overlapped| unsafe { ConnectNamedPipe(pipe.handle, overlapped) }) {
            Ok(_) => {}
            // The client came between the instance and this call.
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32) => {}
            Err(e) => return Err(e),
        }
        // So the next client finds an instance at once.
        match self.instance(false) {
            Ok(next) => *self.next.lock() = Some(next),
            Err(e) => log::warn!("host: cannot make the next instance of {}: {e}", self.name.display()),
        }
        let closer = pipe.clone();
        let client = pipe.clone();
        Ok(Conn {
            reader: Box::new(PipeIo(pipe.clone())),
            writer: Box::new(PipeIo(pipe.clone())),
            closer: Arc::new(move || closer.shutdown()),
            peer: Box::new(move || crate::identity::system::client(&client)),
        })
    }

    /// Make a blocked `accept` return. False when that failed.
    pub fn wake(&self) -> bool {
        // SAFETY: the name is NUL-terminated, and the handle closes at once.
        unsafe {
            let handle = CreateFileW(self.wide_name.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_NONE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut());
            if handle == INVALID_HANDLE_VALUE {
                return false;
            }
            CloseHandle(handle);
        }
        true
    }

    /// Close the instance that waits. The pipe goes away with its last handle.
    pub fn remove(&self) {
        self.next.lock().take();
    }
}

fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}
