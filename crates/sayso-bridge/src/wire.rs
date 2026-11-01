//! What goes between the bridge and the host in the sandbox. The channel is a
//! pair of sockets with no name. Sayso makes it and gives one end to the bridge
//! when it starts the bridge, so no other program can reach it.

use crate::identity::Who;
use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

/// A new connection of an app. The bridge sends one packet with this message
/// and the socket of the connection, after the first read of the app.
#[derive(Debug, PartialEq, Eq)]
pub struct Attested {
    /// The process that wrote the first read, in the PID namespace of the bridge.
    pub pid: i32,
    /// What the checks say about it. The error says why it has no identity.
    pub who: Result<Who, String>,
}

/// The longest packet on the channel.
pub const MAX_PACKET: usize = 16 * 1024;

const NONE: u8 = 0;
const PATH: u8 = 1;
const FLATPAK: u8 = 2;

impl Attested {
    pub fn encode(&self) -> Vec<u8> {
        let (kind, text): (u8, &[u8]) = match &self.who {
            Err(reason) => (NONE, reason.as_bytes()),
            Ok(Who::Path(path)) => (PATH, path.as_os_str().as_bytes()),
            Ok(Who::Flatpak(id)) => (FLATPAK, id.as_bytes()),
        };
        let text = &text[..text.len().min(MAX_PACKET - 5)];
        let mut packet = Vec::with_capacity(text.len() + 5);
        packet.push(kind);
        packet.extend_from_slice(&self.pid.to_le_bytes());
        packet.extend_from_slice(text);
        packet
    }

    pub fn decode(packet: &[u8]) -> Option<Attested> {
        let (&kind, rest) = packet.split_first()?;
        let (pid, text) = rest.split_first_chunk::<4>()?;
        let pid = i32::from_le_bytes(*pid);
        let who = match kind {
            NONE => Err(String::from_utf8_lossy(text).into_owned()),
            PATH => Ok(Who::Path(std::ffi::OsString::from_vec(text.to_vec()).into())),
            FLATPAK => Ok(Who::Flatpak(String::from_utf8(text.to_vec()).ok().filter(|id| crate::identity::valid_app_id(id))?)),
            _ => return None,
        };
        Some(Attested { pid, who })
    }
}

/// `socketpair` with `kind`. Both ends close at an exec.
pub fn pair(kind: libc::c_int) -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors.
    if unsafe { libc::socketpair(libc::AF_UNIX, kind | libc::SOCK_CLOEXEC, 0, fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call gave these two new descriptors.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Send `bytes` and, with `file`, one descriptor with them. Gives the count
/// of bytes that went out.
pub fn send(socket: BorrowedFd<'_>, bytes: &[u8], file: Option<BorrowedFd<'_>>) -> io::Result<usize> {
    let mut part = libc::iovec { iov_base: bytes.as_ptr().cast_mut().cast(), iov_len: bytes.len() };
    let mut control = [0u64; 4];
    // SAFETY: all fields of `msghdr` can be zero.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &raw mut part;
    message.msg_iovlen = 1;
    if let Some(file) = file {
        message.msg_control = control.as_mut_ptr().cast();
        // SAFETY: only arithmetic on the size.
        message.msg_controllen = unsafe { libc::CMSG_SPACE(size_of::<libc::c_int>() as u32) } as _;
        // SAFETY: the control buffer has room for one header with one descriptor.
        unsafe {
            let header = libc::CMSG_FIRSTHDR(&raw const message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(size_of::<libc::c_int>() as u32) as _;
            std::ptr::write_unaligned(libc::CMSG_DATA(header).cast::<libc::c_int>(), file.as_raw_fd());
        }
    }
    // SAFETY: the socket is open, and `message` points to live buffers.
    let count = unsafe { libc::sendmsg(socket.as_raw_fd(), &raw const message, libc::MSG_NOSIGNAL) };
    if count < 0 { Err(io::Error::last_os_error()) } else { Ok(count as usize) }
}

/// Read into `buffer`, and add each descriptor that came with the bytes to `files`.
pub fn recv(socket: BorrowedFd<'_>, buffer: &mut [u8], files: &mut Vec<OwnedFd>) -> io::Result<usize> {
    let mut part = libc::iovec { iov_base: buffer.as_mut_ptr().cast(), iov_len: buffer.len() };
    let mut control = [0u64; 4];
    // SAFETY: all fields of `msghdr` can be zero.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &raw mut part;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = std::mem::size_of_val(&control) as _;
    // SAFETY: the socket is open, and `message` points to live buffers of the sizes that it says.
    let count = unsafe { libc::recvmsg(socket.as_raw_fd(), &raw mut message, libc::MSG_CMSG_CLOEXEC) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `recvmsg` filled the control data, and the macros stay in its length.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&raw const message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let room = (*header).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                for index in 0..room / size_of::<libc::c_int>() {
                    let fd = std::ptr::read_unaligned(libc::CMSG_DATA(header).cast::<libc::c_int>().add(index));
                    files.push(OwnedFd::from_raw_fd(fd));
                }
            }
            header = libc::CMSG_NXTHDR(&raw const message, header);
        }
    }
    Ok(count as usize)
}

/// The start of the `path` of a `transcribe_file` that the bridge opened. A
/// number follows it. A path with another start never comes from the bridge,
/// and the host opens only what the bridge sent.
pub const FILE_PREFIX: &str = "/sayso-bridge/file/";
/// The `path` of a `transcribe_file` that the bridge could not open.
pub const NO_FILE: &str = "/sayso-bridge/none";

/// The most files that wait for their line on one connection. An older one goes away.
const MAX_WAITING: usize = 64;

/// The files that came with the lines of one connection, by their number:
/// 0 for the first file of the connection, then 1, and so on.
#[derive(Default)]
pub struct WaitingFiles {
    next: u64,
    files: BTreeMap<u64, File>,
}

impl WaitingFiles {
    fn add(&mut self, file: OwnedFd) {
        self.files.insert(self.next, file.into());
        self.next += 1;
        while self.files.len() > MAX_WAITING {
            self.files.pop_first();
        }
    }

    /// The file of this `path` from a line.
    pub fn take(&mut self, path: &str) -> Option<File> {
        let number: u64 = path.strip_prefix(FILE_PREFIX)?.parse().ok()?;
        self.files.remove(&number)
    }
}

/// The host's end of one connection: reads the bytes of the relay, and keeps
/// the files that came with them.
pub struct RelayReader {
    stream: UnixStream,
    files: Arc<Mutex<WaitingFiles>>,
}

impl RelayReader {
    pub fn new(stream: UnixStream) -> (RelayReader, Arc<Mutex<WaitingFiles>>) {
        let files = Arc::new(Mutex::new(WaitingFiles::default()));
        (RelayReader { stream, files: files.clone() }, files)
    }
}

impl io::Read for RelayReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut came = Vec::new();
        let count = recv(self.stream.as_fd(), buffer, &mut came)?;
        let mut files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        for file in came {
            files.add(file);
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, Write};

    #[test]
    fn a_message_comes_back_the_same() {
        for who in [Ok(Who::Path("/opt/My App/é".into())), Ok(Who::Flatpak("dev.example.Notes".into())), Err("no /proc/7/exe".to_string())] {
            let message = Attested { pid: 77, who };
            assert_eq!(Attested::decode(&message.encode()), Some(message));
        }
    }

    #[test]
    fn a_path_that_is_not_utf8_keeps_its_bytes() {
        let path = std::ffi::OsString::from_vec(vec![b'/', b'x', 0xFF]);
        let message = Attested { pid: 1, who: Ok(Who::Path(path.into())) };
        assert_eq!(Attested::decode(&message.encode()), Some(message));
    }

    #[test]
    fn a_broken_message_gives_nothing() {
        assert_eq!(Attested::decode(&[]), None);
        assert_eq!(Attested::decode(&[PATH, 1, 0]), None);
        assert_eq!(Attested::decode(&[9, 1, 0, 0, 0]), None);
        let mut bad_id = vec![FLATPAK, 1, 0, 0, 0];
        bad_id.extend_from_slice(b"../x");
        assert_eq!(Attested::decode(&bad_id), None);
    }

    #[test]
    fn a_descriptor_comes_with_its_bytes() {
        let (one, two) = pair(libc::SOCK_STREAM).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"sound").unwrap();
        file.rewind().unwrap();
        assert_eq!(send(one.as_fd(), b"line\n", Some(file.as_fd())).unwrap(), 5);
        let (mut reader, files) = RelayReader::new(two.into());
        let mut buffer = [0; 16];
        assert_eq!(reader.read(&mut buffer).unwrap(), 5);
        assert_eq!(&buffer[..5], b"line\n");
        let mut text = String::new();
        files.lock().unwrap().take(&format!("{FILE_PREFIX}0")).unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(text, "sound");
    }

    #[test]
    fn a_file_comes_out_only_with_its_number() {
        let mut waiting = WaitingFiles::default();
        for _ in 0..3 {
            waiting.add(tempfile::tempfile().unwrap().into());
        }
        assert!(waiting.take("/etc/passwd").is_none());
        assert!(waiting.take(NO_FILE).is_none());
        assert!(waiting.take(&format!("{FILE_PREFIX}9")).is_none());
        assert!(waiting.take(&format!("{FILE_PREFIX}1")).is_some());
        assert!(waiting.take(&format!("{FILE_PREFIX}1")).is_none(), "once");
        assert!(waiting.take(&format!("{FILE_PREFIX}0")).is_some(), "a refused line does not move the next file");
    }

    #[test]
    fn old_files_go_away() {
        let mut waiting = WaitingFiles::default();
        for _ in 0..MAX_WAITING + 1 {
            waiting.add(tempfile::tempfile().unwrap().into());
        }
        assert!(waiting.take(&format!("{FILE_PREFIX}0")).is_none());
        assert!(waiting.take(&format!("{FILE_PREFIX}1")).is_some());
    }
}
