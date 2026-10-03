//! Keep stdout for protocol lines only.
//!
//! whisper.cpp, ggml, and ONNX Runtime are C and C++ libraries that may print.
//! A stray line on stdout would corrupt the protocol stream. Like the Swift
//! engine, take a private duplicate of the real stdout for the protocol, then
//! point stdout at stderr for everything else.

use std::fs::File;
use std::io::Write;

/// The writer for protocol lines. Afterwards stdout (C file descriptor 1, and
/// on Windows also the Win32 standard output handle) goes to stderr. Falls back
/// to the plain stdout when the duplicate fails.
pub fn protocol_writer() -> Box<dyn Write + Send> {
    match redirect() {
        Some(file) => Box::new(file),
        None => Box::new(std::io::stdout()),
    }
}

#[cfg(unix)]
fn redirect() -> Option<File> {
    use std::os::fd::FromRawFd;
    use std::os::raw::c_int;
    unsafe extern "C" {
        fn dup(fd: c_int) -> c_int;
        fn dup2(src: c_int, dst: c_int) -> c_int;
    }
    // SAFETY: plain POSIX calls on the standard descriptors. The duplicate is a
    // new descriptor that only the returned File owns.
    unsafe {
        let protocol = dup(1);
        if protocol < 0 {
            return None;
        }
        // If this fails, stdout stays as it was; the protocol still works.
        dup2(2, 1);
        Some(File::from_raw_fd(protocol))
    }
}

/// Windows has two layers: the C runtime's descriptor 1 (used by `printf`) and
/// the Win32 standard output handle (used by Rust's `println!` and by
/// `WriteFile(GetStdHandle(...))`). Both are pointed at stderr. The C and C++
/// libraries link the same universal C runtime DLL as Rust, so they share
/// descriptor 1.
#[cfg(windows)]
fn redirect() -> Option<File> {
    use std::os::raw::{c_int, c_void};
    use std::os::windows::io::FromRawHandle;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    unsafe extern "C" {
        fn _dup(fd: c_int) -> c_int;
        fn _dup2(src: c_int, dst: c_int) -> c_int;
        fn _get_osfhandle(fd: c_int) -> isize;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn SetStdHandle(which: u32, handle: *mut c_void) -> i32;
    }
    // SAFETY: C runtime and Win32 calls on the standard handles. `_dup` makes a
    // new descriptor with its own duplicated OS handle. The returned File owns
    // that handle; the descriptor is never used or closed again.
    unsafe {
        let protocol = _dup(1);
        if protocol < 0 {
            return None;
        }
        let handle = _get_osfhandle(protocol);
        if handle == -1 || handle == 0 {
            return None;
        }
        let stderr = GetStdHandle(STD_ERROR_HANDLE);
        if !stderr.is_null() && stderr as isize != -1 {
            _dup2(2, 1);
            SetStdHandle(STD_OUTPUT_HANDLE, stderr);
        }
        Some(File::from_raw_handle(handle as *mut c_void))
    }
}

#[cfg(not(any(unix, windows)))]
fn redirect() -> Option<File> {
    None
}
