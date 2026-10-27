//! The client end of the socket: a Unix socket on macOS and Linux, a named
//! pipe on Windows.

use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Arc;

pub(crate) struct Stream {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    /// Stops both directions. A blocked read returns.
    pub closer: Arc<dyn Fn() + Send + Sync>,
}

#[cfg(unix)]
pub(crate) fn connect(path: &Path) -> io::Result<Stream> {
    let stream = std::os::unix::net::UnixStream::connect(path)?;
    let reader = stream.try_clone()?;
    let closer = stream.try_clone()?;
    Ok(Stream {
        reader: Box::new(reader),
        writer: Box::new(stream),
        closer: Arc::new(move || {
            let _ = closer.shutdown(std::net::Shutdown::Both);
        }),
    })
}

#[cfg(windows)]
pub(crate) fn connect(path: &Path) -> io::Result<Stream> {
    let pipe = Arc::new(pipe::Pipe::open(path)?);
    let closer = pipe.clone();
    Ok(Stream { reader: Box::new(pipe::PipeIo(pipe.clone())), writer: Box::new(pipe::PipeIo(pipe)), closer: Arc::new(move || closer.shutdown()) })
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn connect(_path: &Path) -> io::Result<Stream> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "Sayso extensions do not run on this system"))
}

/// The handle uses overlapped I/O, and each call waits for its own result.
/// Windows runs the calls on a handle without overlapped I/O one at a time,
/// so a read that waits for the host would block each request.
#[cfg(windows)]
mod pipe {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY, ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING, ReadFile, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile};
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
    use windows_sys::Win32::System::Threading::CreateEventW;
    use windows_sys::core::BOOL;

    pub(crate) struct Pipe {
        handle: HANDLE,
        closed: AtomicBool,
    }

    // SAFETY: a pipe handle can be used from any thread. Overlapped calls
    // from two threads at the same time are allowed.
    unsafe impl Send for Pipe {}
    unsafe impl Sync for Pipe {}

    impl Drop for Pipe {
        fn drop(&mut self) {
            // SAFETY: the handle is open, and this is its last user.
            unsafe { CloseHandle(self.handle) };
        }
    }

    impl Pipe {
        pub fn open(path: &Path) -> io::Result<Pipe> {
            let name: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
            // The host can only identify this process, not act as it.
            let flags = FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION;
            for _ in 0..5 {
                // SAFETY: the name is NUL-terminated.
                let handle = unsafe { CreateFileW(name.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_NONE, std::ptr::null(), OPEN_EXISTING, flags, std::ptr::null_mut()) };
                if handle != INVALID_HANDLE_VALUE {
                    return Ok(Pipe { handle, closed: AtomicBool::new(false) });
                }
                // SAFETY: no other call in between.
                let error = unsafe { GetLastError() };
                if error != ERROR_PIPE_BUSY {
                    return Err(io::Error::from_raw_os_error(error as i32));
                }
                // Each instance of the pipe has a client. Wait for the next one.
                // SAFETY: the name is NUL-terminated.
                unsafe { WaitNamedPipeW(name.as_ptr(), 2000) };
            }
            Err(io::Error::new(io::ErrorKind::TimedOut, "the pipe of Sayso stays busy"))
        }

        fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
            if self.closed.load(Ordering::SeqCst) {
                return Ok(0);
            }
            let len = buf.len().min(u32::MAX as usize) as u32;
            // SAFETY: `buf` lives until `io` has the result of the read.
            match self.io(|overlapped| unsafe { ReadFile(self.handle, buf.as_mut_ptr(), len, std::ptr::null_mut(), overlapped) }) {
                Ok(read) => Ok(read as usize),
                Err(e) if matches!(e.raw_os_error().map(|code| code as u32), Some(ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_NO_DATA | ERROR_OPERATION_ABORTED)) => Ok(0),
                Err(e) => Err(e),
            }
        }

        fn write(&self, buf: &[u8]) -> io::Result<usize> {
            if self.closed.load(Ordering::SeqCst) {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            let len = buf.len().min(u32::MAX as usize) as u32;
            // SAFETY: `buf` lives until `io` has the result of the write.
            match self.io(|overlapped| unsafe { WriteFile(self.handle, buf.as_ptr(), len, std::ptr::null_mut(), overlapped) }) {
                Ok(written) => Ok(written as usize),
                // The host closed its end of the pipe.
                Err(e) if matches!(e.raw_os_error().map(|code| code as u32), Some(ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_NO_DATA)) => Err(io::ErrorKind::BrokenPipe.into()),
                Err(e) => Err(e),
            }
        }

        /// Start an overlapped call and wait for its result.
        fn io(&self, start: impl FnOnce(*mut OVERLAPPED) -> BOOL) -> io::Result<u32> {
            // SAFETY: the event and `overlapped` live until the call has ended,
            // because GetOverlappedResult waits for it.
            unsafe {
                let event = CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
                if event.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let mut overlapped: OVERLAPPED = std::mem::zeroed();
                overlapped.hEvent = event;
                if start(&mut overlapped) == 0 {
                    let error = GetLastError();
                    if error != ERROR_IO_PENDING {
                        CloseHandle(event);
                        return Err(io::Error::from_raw_os_error(error as i32));
                    }
                    // A shutdown that came before this call started stops it here.
                    if self.closed.load(Ordering::SeqCst) {
                        CancelIoEx(self.handle, &overlapped);
                    }
                }
                let mut done = 0u32;
                let ok = GetOverlappedResult(self.handle, &overlapped, &mut done, 1);
                let error = io::Error::last_os_error();
                CloseHandle(event);
                if ok == 0 { Err(error) } else { Ok(done) }
            }
        }

        pub fn shutdown(&self) {
            self.closed.store(true, Ordering::SeqCst);
            // SAFETY: the handle is open while `self` lives.
            unsafe { CancelIoEx(self.handle, std::ptr::null()) };
        }
    }

    pub(crate) struct PipeIo(pub Arc<Pipe>);

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
}
