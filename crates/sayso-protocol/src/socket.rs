//! Where the host listens (`docs/extensions.md`, "Code"). The host and the
//! SDK use the same rules.
//!
//! 1. `SAYSO_HOST_SOCKET`, when it is set. Sayso sets it for the extensions
//!    that it starts.
//! 2. Linux: `$XDG_RUNTIME_DIR/sayso/host.sock`, or
//!    `$XDG_RUNTIME_DIR/app/<app id>/host.sock` in a Flatpak.
//! 3. macOS: `sayso/host.sock` in the temporary folder of the user
//!    (`confstr(_CS_DARWIN_USER_TEMP_DIR)`). Each process of the user gets
//!    the same folder, also when Finder starts it.
//! 4. Else: `/tmp/sayso-<uid>/host.sock`.
//! 5. Windows: the named pipe `\\.\pipe\sayso-host-<user SID>`.
//!
//! On macOS and Linux the folder of the socket has mode `0700`. The host
//! makes it.

use std::io;
use std::path::PathBuf;

/// The environment variable with the path of the socket.
pub const SOCKET_ENV: &str = "SAYSO_HOST_SOCKET";

/// The file name of the socket in its folder.
pub const SOCKET_FILE: &str = "host.sock";

/// The system, for the rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    Linux,
    MacOs,
    Windows,
    /// Another Unix.
    OtherUnix,
}

impl System {
    pub fn current() -> System {
        if cfg!(target_os = "linux") {
            System::Linux
        } else if cfg!(target_os = "macos") {
            System::MacOs
        } else if cfg!(windows) {
            System::Windows
        } else {
            System::OtherUnix
        }
    }
}

/// What the rules read from the system. A trait, so tests do not touch the
/// real environment.
pub trait SocketEnv {
    /// An environment variable. An empty value counts as not set.
    fn var(&self, key: &str) -> Option<String>;
    /// macOS: `confstr(_CS_DARWIN_USER_TEMP_DIR)`.
    fn darwin_user_temp_dir(&self) -> Option<PathBuf>;
    /// Unix: the user id.
    fn uid(&self) -> Option<u32>;
    /// Windows: the SID of the user, for example `S-1-5-21-...`.
    fn user_sid(&self) -> Option<String>;
}

/// The path of the host socket (the name of the pipe on Windows) for this
/// process.
pub fn host_socket_path() -> io::Result<PathBuf> {
    host_socket_path_for(System::current(), &SystemEnv)
}

/// The path of the host socket on `system` with `env`.
pub fn host_socket_path_for(system: System, env: &impl SocketEnv) -> io::Result<PathBuf> {
    if let Some(path) = env.var(SOCKET_ENV) {
        return Ok(PathBuf::from(path));
    }
    let in_tmp = || match env.uid() {
        Some(uid) => Ok(PathBuf::from(format!("/tmp/sayso-{uid}")).join(SOCKET_FILE)),
        None => Err(io::Error::other("cannot find the user id")),
    };
    match system {
        System::Linux => match env.var("XDG_RUNTIME_DIR") {
            Some(runtime) => {
                let runtime = PathBuf::from(runtime);
                Ok(match env.var("FLATPAK_ID") {
                    // All sandboxes of the app share this folder.
                    Some(app_id) => runtime.join("app").join(app_id).join(SOCKET_FILE),
                    None => runtime.join("sayso").join(SOCKET_FILE),
                })
            }
            None => in_tmp(),
        },
        System::MacOs => match env.darwin_user_temp_dir() {
            Some(temp) => Ok(temp.join("sayso").join(SOCKET_FILE)),
            None => in_tmp(),
        },
        System::OtherUnix => in_tmp(),
        System::Windows => match env.user_sid() {
            Some(sid) => Ok(PathBuf::from(format!(r"\\.\pipe\sayso-host-{sid}"))),
            None => Err(io::Error::other("cannot find the SID of the user")),
        },
    }
}

/// The real environment of this process.
pub struct SystemEnv;

impl SocketEnv for SystemEnv {
    fn var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|value| !value.is_empty())
    }

    #[cfg(target_os = "macos")]
    fn darwin_user_temp_dir(&self) -> Option<PathBuf> {
        use std::os::unix::ffi::OsStringExt;
        // SAFETY: a null buffer asks for the size, and the second call fills a
        // buffer of that size.
        let size = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, std::ptr::null_mut(), 0) };
        if size == 0 {
            return None;
        }
        let mut buffer = vec![0u8; size];
        let written = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buffer.as_mut_ptr().cast(), size) };
        if written == 0 || written > size {
            return None;
        }
        // Drop the closing NUL.
        buffer.truncate(written - 1);
        Some(PathBuf::from(std::ffi::OsString::from_vec(buffer)))
    }

    #[cfg(not(target_os = "macos"))]
    fn darwin_user_temp_dir(&self) -> Option<PathBuf> {
        None
    }

    #[cfg(unix)]
    fn uid(&self) -> Option<u32> {
        // SAFETY: getuid has no failure mode.
        Some(unsafe { libc::getuid() })
    }

    #[cfg(not(unix))]
    fn uid(&self) -> Option<u32> {
        None
    }

    #[cfg(windows)]
    fn user_sid(&self) -> Option<String> {
        windows::user_sid()
    }

    #[cfg(not(windows))]
    fn user_sid(&self) -> Option<String> {
        None
    }
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// The SID of the user of this process, as text.
    pub fn user_sid() -> Option<String> {
        // SAFETY: each handle and buffer lives until the end of this function,
        // and the sizes come from the calls that fill them.
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return None;
            }
            let mut size = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size);
            // u64 for the alignment of TOKEN_USER.
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            let ok = GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), size, &mut size);
            CloseHandle(token);
            if ok == 0 {
                return None;
            }
            let user = &*(buffer.as_ptr() as *const TOKEN_USER);
            let mut text: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
                return None;
            }
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            Some(sid)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeEnv {
        vars: HashMap<&'static str, &'static str>,
        temp: Option<&'static str>,
        uid: Option<u32>,
        sid: Option<&'static str>,
    }

    impl SocketEnv for FakeEnv {
        fn var(&self, key: &str) -> Option<String> {
            self.vars.get(key).map(|v| v.to_string()).filter(|v| !v.is_empty())
        }
        fn darwin_user_temp_dir(&self) -> Option<PathBuf> {
            self.temp.map(PathBuf::from)
        }
        fn uid(&self) -> Option<u32> {
            self.uid
        }
        fn user_sid(&self) -> Option<String> {
            self.sid.map(str::to_string)
        }
    }

    fn path(system: System, env: &FakeEnv) -> String {
        host_socket_path_for(system, env).unwrap().display().to_string()
    }

    #[test]
    fn the_environment_variable_wins_on_each_system() {
        let env = FakeEnv { vars: HashMap::from([(SOCKET_ENV, "/x/host.sock"), ("XDG_RUNTIME_DIR", "/run/user/1000")]), uid: Some(1000), ..Default::default() };
        for system in [System::Linux, System::MacOs, System::Windows, System::OtherUnix] {
            assert_eq!(path(system, &env), "/x/host.sock");
        }
    }

    #[test]
    fn linux_uses_the_runtime_folder_and_the_folder_that_flatpak_sandboxes_share() {
        let mut env = FakeEnv { vars: HashMap::from([("XDG_RUNTIME_DIR", "/run/user/1000")]), uid: Some(1000), ..Default::default() };
        assert_eq!(path(System::Linux, &env), "/run/user/1000/sayso/host.sock");
        env.vars.insert("FLATPAK_ID", "dev.sayso.Sayso");
        assert_eq!(path(System::Linux, &env), "/run/user/1000/app/dev.sayso.Sayso/host.sock");
        env.vars.insert("XDG_RUNTIME_DIR", "");
        assert_eq!(path(System::Linux, &env), "/tmp/sayso-1000/host.sock", "an empty variable is not set");
    }

    #[test]
    fn macos_uses_the_temporary_folder_of_the_user() {
        let env = FakeEnv { temp: Some("/var/folders/ab/xyz/T/"), uid: Some(501), ..Default::default() };
        assert_eq!(path(System::MacOs, &env), "/var/folders/ab/xyz/T/sayso/host.sock");
        // XDG_RUNTIME_DIR is a Linux rule.
        let env = FakeEnv { vars: HashMap::from([("XDG_RUNTIME_DIR", "/run/user/501")]), uid: Some(501), ..Default::default() };
        assert_eq!(path(System::MacOs, &env), "/tmp/sayso-501/host.sock");
    }

    #[test]
    fn other_systems_use_a_folder_of_the_user_in_tmp() {
        let env = FakeEnv { uid: Some(42), ..Default::default() };
        assert_eq!(path(System::OtherUnix, &env), "/tmp/sayso-42/host.sock");
        assert!(host_socket_path_for(System::OtherUnix, &FakeEnv::default()).is_err());
    }

    #[test]
    fn windows_uses_a_pipe_with_the_sid_of_the_user() {
        let env = FakeEnv { sid: Some("S-1-5-21-1-2-3-1001"), ..Default::default() };
        assert_eq!(path(System::Windows, &env), r"\\.\pipe\sayso-host-S-1-5-21-1-2-3-1001");
        assert!(host_socket_path_for(System::Windows, &FakeEnv::default()).is_err());
    }

    #[test]
    fn the_real_environment_gives_a_path() {
        let path = host_socket_path().unwrap();
        if std::env::var_os(SOCKET_ENV).is_none() {
            #[cfg(windows)]
            assert!(path.display().to_string().starts_with(r"\\.\pipe\sayso-host-S-"), "{}", path.display());
            #[cfg(unix)]
            assert!(path.ends_with("host.sock"), "{}", path.display());
        }
        #[cfg(target_os = "macos")]
        assert!(SystemEnv.darwin_user_temp_dir().unwrap().is_dir());
    }
}
