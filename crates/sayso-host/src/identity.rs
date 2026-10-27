//! Who is calling (`docs/extensions.md`, "Who is calling"). The host knows a
//! client by its executable, not by a token:
//!
//! - Linux: `SO_PEERCRED` gives the pid, then `/proc/<pid>/exe`.
//! - macOS: `LOCAL_PEERTOKEN` gives the audit token, then `proc_pidpath`. For
//!   a record with `team_id`, the code signature of the peer must have that
//!   team and the bundle id.
//! - Windows: `GetNamedPipeClientProcessId`, then `QueryFullProcessImageNameW`.
//!
//! The executable must be in the `path` of a record of `state.toml`.

use crate::state::{Record, State};
use std::io;
use std::path::{Path, PathBuf};

/// The other end of a connection. Only the host makes one.
pub struct Peer {
    #[cfg(unix)]
    pub(crate) stream: std::os::unix::net::UnixStream,
    #[cfg(windows)]
    pub(crate) pipe: std::sync::Arc<crate::transport::windows::Pipe>,
}

/// Finds the process at the other end of a connection. A trait, so tests can
/// give the host another identity.
pub trait Identify: Send + Sync {
    /// The executable of the peer.
    fn executable(&self, peer: &Peer) -> io::Result<PathBuf>;
    /// True when the code signature of the peer is valid and has the team id,
    /// and the bundle id when one is given.
    fn signed_by(&self, peer: &Peer, team_id: &str, bundle_id: Option<&str>) -> bool;
}

/// The checks of the system.
pub struct SystemIdentify;

impl Identify for SystemIdentify {
    fn executable(&self, peer: &Peer) -> io::Result<PathBuf> {
        system::executable(peer)?.canonicalize()
    }

    fn signed_by(&self, peer: &Peer, team_id: &str, bundle_id: Option<&str>) -> bool {
        system::signed_by(peer, team_id, bundle_id)
    }
}

/// The record of the extension whose folder has `executable`. When folders
/// are inside each other, the deepest folder wins. A record with `team_id`
/// needs that signature too. An executable that is not on disk matches
/// nothing: its path could have `..` in it.
pub(crate) fn find_record<'a>(state: &'a State, executable: &Path, signed_by: impl Fn(&str, Option<&str>) -> bool) -> Option<&'a Record> {
    let executable = executable.canonicalize().ok()?;
    state
        .extensions
        .iter()
        .filter_map(|record| record.path.canonicalize().ok().map(|folder| (record, folder)))
        .filter(|(_, folder)| executable.starts_with(folder) && executable != *folder)
        .filter(|(record, _)| match &record.team_id {
            Some(team_id) => signed_by(team_id, record.bundle_id.as_deref()),
            None => true,
        })
        .max_by_key(|(_, folder)| folder.components().count())
        .map(|(record, _)| record)
}

#[cfg(target_os = "linux")]
mod system {
    use super::Peer;
    use std::io;
    use std::os::fd::AsRawFd;
    use std::path::PathBuf;

    pub fn executable(peer: &Peer) -> io::Result<PathBuf> {
        let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: the socket is open, and `credentials` has the size in `len`.
        let result = unsafe { libc::getsockopt(peer.stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut credentials).cast(), &mut len) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        std::fs::read_link(format!("/proc/{}/exe", credentials.pid))
    }

    pub fn signed_by(_peer: &Peer, _team_id: &str, _bundle_id: Option<&str>) -> bool {
        false
    }
}

#[cfg(target_os = "macos")]
mod system {
    use super::Peer;
    use core_foundation::base::TCFType;
    use core_foundation::data::CFData;
    use security_framework::os::macos::code_signing::{Flags, GuestAttributes, SecCode, SecRequirement};
    use std::io;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;
    use std::str::FromStr;

    /// `audit_token_t`: eight integers. The pid is the sixth.
    type AuditToken = [u32; 8];

    fn audit_token(peer: &Peer) -> io::Result<AuditToken> {
        let mut token: AuditToken = [0; 8];
        let mut len = std::mem::size_of::<AuditToken>() as libc::socklen_t;
        // SAFETY: the socket is open, and `token` has the size in `len`.
        let result = unsafe { libc::getsockopt(peer.stream.as_raw_fd(), libc::SOL_LOCAL, libc::LOCAL_PEERTOKEN, token.as_mut_ptr().cast(), &mut len) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(token)
    }

    pub fn executable(peer: &Peer) -> io::Result<PathBuf> {
        let pid = audit_token(peer)?[5] as libc::c_int;
        let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the buffer has the size that the call gets.
        let len = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        if len <= 0 {
            return Err(io::Error::last_os_error());
        }
        buffer.truncate(len as usize);
        Ok(PathBuf::from(std::ffi::OsString::from_vec(buffer)))
    }

    pub fn signed_by(peer: &Peer, team_id: &str, bundle_id: Option<&str>) -> bool {
        // The ids go into a requirement text, so they must be plain.
        let plain = |id: &str| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
        if !plain(team_id) || bundle_id.is_some_and(|id| !plain(id)) {
            return false;
        }
        let mut requirement = format!("anchor apple generic and certificate leaf[subject.OU] = \"{team_id}\"");
        if let Some(bundle_id) = bundle_id {
            requirement.push_str(&format!(" and identifier \"{bundle_id}\""));
        }
        let Ok(token) = audit_token(peer) else { return false };
        let Ok(requirement) = SecRequirement::from_str(&requirement) else { return false };
        let bytes: Vec<u8> = token.iter().flat_map(|part| part.to_ne_bytes()).collect();
        let data = CFData::from_buffer(&bytes);
        let mut attributes = GuestAttributes::new();
        attributes.set_audit_token(data.as_concrete_TypeRef());
        match SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE) {
            Ok(code) => code.check_validity(Flags::NONE, &requirement).is_ok(),
            Err(e) => {
                log::warn!("host: cannot find the code of the peer: {e}");
                false
            }
        }
    }
}

#[cfg(windows)]
mod system {
    use super::Peer;
    use std::io;
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};

    pub fn executable(peer: &Peer) -> io::Result<PathBuf> {
        // SAFETY: the pipe and the process handle are open for these calls,
        // and the buffer has the size in `len`.
        unsafe {
            let mut pid = 0u32;
            if GetNamedPipeClientProcessId(peer.pipe.handle(), &mut pid) == 0 {
                return Err(io::Error::last_os_error());
            }
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut buffer = vec![0u16; 32 * 1024];
            let mut len = buffer.len() as u32;
            let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut len);
            let error = io::Error::last_os_error();
            CloseHandle(process);
            if ok == 0 {
                return Err(error);
            }
            Ok(PathBuf::from(std::ffi::OsString::from_wide(&buffer[..len as usize])))
        }
    }

    pub fn signed_by(_peer: &Peer, _team_id: &str, _bundle_id: Option<&str>) -> bool {
        false
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod system {
    use super::Peer;
    use std::io;
    use std::path::PathBuf;

    pub fn executable(_peer: &Peer) -> io::Result<PathBuf> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "the host cannot find the peer on this system"))
    }

    pub fn signed_by(_peer: &Peer, _team_id: &str, _bundle_id: Option<&str>) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, path: &Path, team_id: Option<&str>) -> Record {
        Record { id: id.into(), name: None, version: "1.0.0".into(), path: path.into(), sideloaded: false, granted: vec![], team_id: team_id.map(Into::into), bundle_id: None }
    }

    #[test]
    fn the_executable_must_be_inside_the_folder_of_a_record() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("dev.example.app/1.0.0");
        let other = dir.path().join("dev.example.other/1.0.0");
        std::fs::create_dir_all(app.join("bin")).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let exe = app.join("bin/app");
        std::fs::write(&exe, "").unwrap();
        let state = State { extensions: vec![record("dev.example.other", &other, None), record("dev.example.app", &app, None)] };
        assert_eq!(find_record(&state, &exe, |_, _| false).map(|r| r.id.as_str()), Some("dev.example.app"));
        let evil = dir.path().join("dev.example.app/1.0.0-evil");
        std::fs::create_dir_all(&evil).unwrap();
        std::fs::write(evil.join("app"), "").unwrap();
        assert!(find_record(&state, &evil.join("app"), |_, _| false).is_none(), "a folder with the same start of its name");
        assert!(find_record(&state, &app, |_, _| false).is_none(), "the folder itself is not an executable in it");
        // A path with `..` that leaves the folder, to a file that is not there.
        assert!(find_record(&state, &app.join("bin/../../../x"), |_, _| false).is_none());
        assert!(find_record(&state, &app.join("bin/../../1.0.0-evil/app"), |_, _| false).is_none());
    }

    #[test]
    fn the_deepest_folder_wins() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("outer/inner");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("exe"), "").unwrap();
        let state = State { extensions: vec![record("outer", &dir.path().join("outer"), None), record("inner", &inner, None)] };
        assert_eq!(find_record(&state, &inner.join("exe"), |_, _| false).map(|r| r.id.as_str()), Some("inner"));
    }

    #[test]
    fn a_record_with_a_team_needs_the_signature() {
        let dir = tempfile::tempdir().unwrap();
        let state = State { extensions: vec![record("signed", dir.path(), Some("ABCDE12345"))] };
        let exe = dir.path().join("Signed.app/Contents/MacOS/Signed");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "").unwrap();
        assert!(find_record(&state, &exe, |_, _| false).is_none());
        assert_eq!(find_record(&state, &exe, |team, _| team == "ABCDE12345").map(|r| r.id.as_str()), Some("signed"));
    }

    #[test]
    fn a_record_whose_folder_is_gone_matches_nothing() {
        let state = State { extensions: vec![record("gone", Path::new("/no/such/folder/sayso"), None)] };
        assert!(find_record(&state, Path::new("/no/such/folder/sayso/exe"), |_, _| true).is_none());
    }
}
