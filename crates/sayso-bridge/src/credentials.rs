//! The writer of each read on Linux. `SO_PEERCRED` says only who connected.

use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

/// On the listener, so a line that a client writes before the accept
/// also has its credentials. A connection gets the option with the accept.
pub fn pass_credentials(listener: &UnixListener) -> io::Result<()> {
    let on: libc::c_int = 1;
    // SAFETY: the socket is open, and the size is the size of `on`.
    let result = unsafe { libc::setsockopt(listener.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PASSCRED, (&raw const on).cast(), std::mem::size_of::<libc::c_int>() as libc::socklen_t) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Who wrote what the reader has so far.
#[derive(Default)]
struct Writers {
    first: Option<libc::pid_t>,
    /// A second process, or a part without a pid in this PID namespace.
    other: bool,
}

impl Writers {
    fn add(&mut self, pid: Option<libc::pid_t>) {
        match (pid.filter(|pid| *pid > 0), self.first) {
            (Some(pid), None) => self.first = Some(pid),
            (Some(pid), Some(first)) if pid == first => {}
            _ => self.other = true,
        }
    }

    fn pid(&self) -> Result<libc::pid_t, String> {
        match self.first {
            Some(pid) if !self.other => Ok(pid),
            _ => Err("more than one process wrote to this connection".into()),
        }
    }
}

/// Gives the pid of the one process that wrote what the reader has so far, or
/// an error when there is no such process.
pub type WriterPid = Box<dyn Fn() -> Result<libc::pid_t, String> + Send>;

/// The reader of a connection, and what gives the writer. The reader takes
/// a part ahead of the line, so a second writer closes the connection at
/// the line before its own at the latest.
pub fn reader(stream: UnixStream) -> (Box<dyn Read + Send>, WriterPid) {
    let writers = Arc::new(Mutex::new(Writers::default()));
    let peer = writers.clone();
    (Box::new(CredentialReader { stream, writers }), Box::new(move || peer.lock().unwrap_or_else(|e| e.into_inner()).pid()))
}

struct CredentialReader {
    stream: UnixStream,
    writers: Arc<Mutex<Writers>>,
}

impl Read for CredentialReader {
    /// One `recvmsg`. The kernel does not join the bytes of two writers.
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut part = libc::iovec { iov_base: buffer.as_mut_ptr().cast(), iov_len: buffer.len() };
        // Room for one `ucred`, in the alignment of `cmsghdr`. So file
        // descriptors that a client sends do not come into the host.
        let mut control = [0u64; 4];
        // SAFETY: all fields of `msghdr` can be zero.
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &raw mut part;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = std::mem::size_of_val(&control) as _;
        // SAFETY: the socket is open, and `message` points to live buffers of the sizes that it says.
        let count = unsafe { libc::recvmsg(self.stream.as_raw_fd(), &raw mut message, libc::MSG_CMSG_CLOEXEC) };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if count > 0 {
            self.writers.lock().unwrap_or_else(|e| e.into_inner()).add(sender(&message));
        }
        Ok(count as usize)
    }
}

fn sender(message: &libc::msghdr) -> Option<libc::pid_t> {
    // SAFETY: `recvmsg` filled the control data of `message`, and the
    // macros stay in its length. The data of `SCM_CREDENTIALS` is a `ucred`.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_CREDENTIALS {
                return Some(std::ptr::read_unaligned(libc::CMSG_DATA(header).cast::<libc::ucred>()).pid);
            }
            header = libc::CMSG_NXTHDR(message, header);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_writer_or_no_pid_gives_no_peer() {
        let mut writers = Writers::default();
        assert!(writers.pid().is_err(), "nothing was written");
        writers.add(Some(7));
        writers.add(Some(7));
        assert_eq!(writers.pid().unwrap(), 7);
        let mut second = Writers { first: Some(7), other: false };
        second.add(Some(8));
        assert!(second.pid().is_err());
        for no_pid in [None, Some(0)] {
            let mut writers = Writers::default();
            writers.add(no_pid);
            assert!(writers.pid().is_err());
        }
    }
}
