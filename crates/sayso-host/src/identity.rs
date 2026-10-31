//! Who is calling (`docs/sdk.md`, "Identity"). The host knows an app by its
//! executable, not by a token and not by what the app says:
//!
//! The check runs on the process that wrote the first line of the connection,
//! not on the process that connected: a program can connect, give the socket
//! to a child, and then start an approved program. After that, a line from
//! another process closes the connection (`docs/sdk.md`, "Security").
//!
//! - macOS: `LOCAL_PEERTOKEN` gives the audit token of the last writer, and the
//!   code signature of that process gives the team id and the identifier. An
//!   ad hoc or missing signature gives the path of `proc_pidpath_audittoken`.
//!   A signature that is not valid gives no identity.
//! - Linux: `SCM_CREDENTIALS` of each read gives the pid of the writer, then
//!   `/proc/<pid>/exe`. A file that was deleted or replaced after the start,
//!   another file at that path, and another PID namespace give no identity.
//! - Windows: `GetNamedPipeClientProcessId`, then `QueryFullProcessImageNameW`,
//!   made canonical with `GetFinalPathNameByHandleW`.

use std::path::{Path, PathBuf};

/// The process that wrote a line. Only the host makes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// `audit_token_t`. It changes with an exec, and the pid does not.
    #[cfg(target_os = "macos")]
    pub(crate) token: [u32; 8],
    #[cfg(target_os = "linux")]
    pub(crate) pid: libc::pid_t,
    #[cfg(windows)]
    pub(crate) pid: u32,
}

impl Peer {
    /// True when both come from one process. On macOS also from one exec of it.
    pub(crate) fn same_process(&self, other: &Peer) -> bool {
        #[cfg(target_os = "macos")]
        {
            (self.token[5], self.token[7]) == (other.token[5], other.token[7])
        }
        #[cfg(not(target_os = "macos"))]
        {
            self == other
        }
    }
}

/// How the host knows an app, and how `apps.toml` keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Identity {
    /// macOS, signed with a team id. It stays the same through updates.
    Signed { team_id: String, identifier: String },
    /// The canonical path of the executable: every other program.
    Path(PathBuf),
}

impl Identity {
    /// The key of the app in the host. It has the kind, so a path can never
    /// be the key of a signed app. Paths are compared without case on Windows.
    /// A path that is not UTF-8 keeps its bytes, so two such paths have two keys.
    pub fn id(&self) -> String {
        match self {
            Identity::Signed { team_id, identifier } => format!("signed:{team_id}/{identifier}"),
            Identity::Path(path) => match path.to_str() {
                Some(text) if cfg!(windows) => format!("path:{}", text.to_lowercase()),
                Some(text) => format!("path:{text}"),
                None => format!("bytes:{}", path.as_os_str().as_encoded_bytes().escape_ascii()),
            },
        }
    }
}

/// What the system says about the process at the other end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub identity: Identity,
    /// The name from the system: the bundle name on macOS, else the file name.
    pub name: String,
    /// The organization on the certificate of a signed app.
    pub signer: Option<String>,
    /// The executable is a known interpreter, so an approval covers each of its scripts.
    pub interpreter: bool,
}

impl Caller {
    /// A caller with the path of `executable` as its identity.
    pub fn from_path(executable: PathBuf) -> Caller {
        Caller { name: system_name(&executable), interpreter: is_interpreter(&executable), identity: Identity::Path(executable), signer: None }
    }
}

/// Finds the process at the other end of a connection. A trait, so tests can
/// give the host another identity.
pub trait Identify: Send + Sync {
    /// The identity of the process that wrote the first line of a connection.
    /// An error says why it has none: the host
    /// refuses the connection with `not_approved` and asks no question.
    fn identify(&self, peer: &Peer) -> Result<Caller, String>;
}

/// The checks of the system.
pub struct SystemIdentify;

impl Identify for SystemIdentify {
    fn identify(&self, peer: &Peer) -> Result<Caller, String> {
        system::identify(peer)
    }
}

/// The longest name that Sayso shows from an app or from a file.
const MAX_NAME: usize = 40;

/// True for a character that a person cannot see or that changes the text
/// around it: control characters, the format characters of Unicode (`Cf`), the
/// line and paragraph separators, and letters and marks that show as nothing.
fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(c,
            // Format characters, with the direction marks and the tag characters.
            '\u{00AD}' | '\u{0600}'..='\u{0605}' | '\u{061C}' | '\u{06DD}' | '\u{070F}' | '\u{0890}'..='\u{0891}' | '\u{08E2}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}' | '\u{110CD}' | '\u{13430}'..='\u{1343F}' | '\u{1BCA0}'..='\u{1BCA3}' | '\u{1D173}'..='\u{1D17A}' | '\u{E0001}' | '\u{E0020}'..='\u{E007F}'
            // The line and paragraph separators.
            | '\u{2028}' | '\u{2029}'
            // Letters and marks that show as nothing.
            | '\u{034F}' | '\u{115F}' | '\u{1160}' | '\u{17B4}' | '\u{17B5}' | '\u{180B}'..='\u{180D}' | '\u{180F}' | '\u{2800}' | '\u{3164}' | '\u{FE00}'..='\u{FE0F}' | '\u{FFA0}' | '\u{E0100}'..='\u{E01EF}')
}

/// `text` without the characters that can mislead.
pub(crate) fn without_hidden(text: &str) -> String {
    text.chars().filter(|c| !hidden(*c)).collect()
}

/// A name that is safe to show: no control characters, no characters that
/// change the direction of text or hide in it, and at most 40 characters.
/// None when nothing is left.
pub fn clean_name(name: &str) -> Option<String> {
    let cleaned = without_hidden(name);
    let cleaned: String = cleaned.trim().chars().take(MAX_NAME).collect();
    let cleaned = cleaned.trim_end();
    (!cleaned.is_empty()).then(|| cleaned.to_string())
}

/// Text from a file name or a file, to show in one line. Each character that
/// can mislead shows as U+FFFD, so the person sees that this text has one.
/// The identity keeps the real text.
pub fn visible_text(text: &str) -> String {
    text.chars().map(|c| if hidden(c) { '\u{FFFD}' } else { c }).collect()
}

/// A path to show, as [`visible_text`].
pub fn visible_path(path: &Path) -> String {
    visible_text(&path.to_string_lossy())
}

/// The name from the system.
fn system_name(executable: &Path) -> String {
    #[cfg(target_os = "macos")]
    return system::display_name(executable);
    #[cfg(not(target_os = "macos"))]
    file_name(executable)
}

/// The file name to show. On Windows without `.exe`.
fn file_name(executable: &Path) -> String {
    let name = if cfg!(windows) { executable.file_stem() } else { executable.file_name() };
    name.and_then(|name| clean_name(&name.to_string_lossy())).unwrap_or_else(|| "?".into())
}

/// True for the file name of a known interpreter, also with a version in it
/// (`python3.12`) and without case (`Python`, `node.exe`).
fn is_interpreter(executable: &Path) -> bool {
    const KNOWN: [&str; 35] = ["awk", "bash", "bun", "busybox", "cmd", "cscript", "dash", "deno", "dotnet", "fish", "gawk", "java", "javaw", "julia", "ksh", "lua", "mawk", "node", "nodejs", "osascript", "perl", "php", "powershell", "pwsh", "pypy", "python", "pythonw", "r", "rscript", "ruby", "sh", "tclsh", "wish", "wscript", "zsh"];
    let Some(stem) = executable.file_name().map(|name| name.to_string_lossy().to_lowercase()) else { return false };
    // The loader of Linux runs any program: `ld-linux-x86-64.so.2 ./app`.
    if stem.starts_with("ld-linux") || stem.starts_with("ld-musl") || stem == "ld.so" {
        return true;
    }
    let stem = stem.strip_suffix(".exe").unwrap_or(&stem);
    // The builds of python.org: `python3-intel64`.
    let stem = stem.strip_suffix("-intel64").unwrap_or(stem);
    let stem = stem.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-');
    KNOWN.contains(&stem)
}

/// The company in the summary of a certificate:
/// `Developer ID Application: Example LLC (ABCDE12345)` gives `Example LLC`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn signer_name(summary: &str, team_id: &str) -> Option<String> {
    let name = summary.split_once(": ").map_or(summary, |(_, name)| name);
    let name = name.strip_suffix(&format!(" ({team_id})")).unwrap_or(name);
    clean_name(name)
}

/// A Linux link to a file that was deleted or replaced after the start.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn deleted_link(link: &Path) -> bool {
    link.as_os_str().as_encoded_bytes().ends_with(b" (deleted)")
}

#[cfg(target_os = "linux")]
mod system {
    use super::{Caller, Peer, deleted_link};
    use std::os::unix::fs::MetadataExt;

    pub fn identify(peer: &Peer) -> Result<Caller, String> {
        let exe = format!("/proc/{}/exe", peer.pid);
        let link = std::fs::read_link(&exe).map_err(|e| format!("no {exe}: {e}"))?;
        if deleted_link(&link) {
            return Err(format!("{} was deleted or replaced after it started", link.display()));
        }
        // Who has a capability in a PID namespace can write the pid of another
        // process of it in `SCM_CREDENTIALS`. In the namespace of the host, only the true pid passes.
        let namespace = |pid: &str| std::fs::read_link(format!("/proc/{pid}/ns/pid")).map_err(|e| format!("no PID namespace of {pid}: {e}"));
        if namespace(&peer.pid.to_string())? != namespace("self")? {
            return Err(format!("process {} is in another PID namespace", peer.pid));
        }
        // A private mount namespace can show another file at the path of an approved app.
        let file_of = |path: &std::path::Path| std::fs::metadata(path).map(|meta| (meta.dev(), meta.ino())).map_err(|e| format!("cannot read {}: {e}", path.display()));
        if file_of(exe.as_ref())? != file_of(&link)? {
            return Err(format!("process {} does not run the file at {}", peer.pid, link.display()));
        }
        Ok(Caller::from_path(link))
    }
}

#[cfg(target_os = "macos")]
pub(crate) mod system {
    use super::{Caller, Identity, Path, Peer, clean_name, is_interpreter, signer_name};
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::TCFType;
    use core_foundation::bundle::CFBundle;
    use core_foundation::data::CFData;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::{CFString, CFStringRef};
    use core_foundation::url::CFURL;
    use security_framework::certificate::SecCertificate;
    use security_framework::os::macos::code_signing::{Flags, GuestAttributes, SecCode, SecRequirement, SecStaticCode};
    use std::ffi::c_void;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::str::FromStr;

    /// `audit_token_t`: eight integers. The pid is the sixth.
    type AuditToken = [u32; 8];

    /// `errSecCSUnsigned`: the code has no signature.
    const NOT_SIGNED: i32 = -67062;
    /// `kSecCSSigningInformation`.
    const SIGNING_INFORMATION: u32 = 1 << 1;

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        static kSecCodeInfoIdentifier: CFStringRef;
        static kSecCodeInfoTeamIdentifier: CFStringRef;
        static kSecCodeInfoCertificates: CFStringRef;
        fn SecCodeCopySigningInformation(code: *const c_void, flags: u32, information: *mut CFDictionaryRef) -> i32;
    }

    unsafe extern "C" {
        fn proc_pidpath_audittoken(token: *mut AuditToken, buffer: *mut c_void, size: u32) -> libc::c_int;
    }

    /// The process that wrote to `stream` last.
    pub fn last_writer(stream: &UnixStream) -> Result<Peer, String> {
        let mut token: AuditToken = [0; 8];
        let mut len = std::mem::size_of::<AuditToken>() as libc::socklen_t;
        // SAFETY: the socket is open, and `token` has the size in `len`.
        let result = unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_LOCAL, libc::LOCAL_PEERTOKEN, token.as_mut_ptr().cast(), &mut len) };
        if result != 0 {
            return Err(format!("no audit token: {}", std::io::Error::last_os_error()));
        }
        Ok(Peer { token })
    }

    /// The path of the process with this token. A process that did an exec
    /// after the token gets an error, also when the pid is the same.
    fn executable(mut token: AuditToken) -> Result<PathBuf, String> {
        let pid = token[5];
        let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the buffer has the size that the call gets, and the token lives for the call.
        let len = unsafe { proc_pidpath_audittoken(&mut token, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        if len <= 0 {
            return Err(format!("no path for pid {pid}: {}", std::io::Error::last_os_error()));
        }
        buffer.truncate(len as usize);
        PathBuf::from(std::ffi::OsString::from_vec(buffer)).canonicalize().map_err(|e| format!("no file for pid {pid}: {e}"))
    }

    /// What the signature of valid code says.
    struct Signing {
        team_id: Option<String>,
        identifier: Option<String>,
        leaf: Option<SecCertificate>,
    }

    fn signing_information(code: &SecCode) -> Result<Signing, String> {
        let mut raw: CFDictionaryRef = std::ptr::null();
        // SAFETY: the code is alive, and `raw` gets a dictionary that we own.
        let status = unsafe { SecCodeCopySigningInformation(code.as_concrete_TypeRef().cast(), SIGNING_INFORMATION, &mut raw) };
        if status != 0 || raw.is_null() {
            return Err(format!("no signing information: {status}"));
        }
        // SAFETY: the call above gave a dictionary under the create rule.
        let information: CFDictionary = unsafe { TCFType::wrap_under_create_rule(raw) };
        let text = |key: CFStringRef| {
            // SAFETY: the values of these keys are strings, and the dictionary keeps them alive.
            information.find(key.cast()).map(|value| unsafe { CFString::wrap_under_get_rule((*value).cast()) }.to_string())
        };
        // SAFETY: the keys are constants of the framework.
        let (team_id, identifier, certificates) = unsafe { (text(kSecCodeInfoTeamIdentifier), text(kSecCodeInfoIdentifier), kSecCodeInfoCertificates) };
        let leaf = information.find(certificates.cast()).and_then(|value| {
            // SAFETY: the value of this key is an array of certificates.
            let array: CFArray<SecCertificate> = unsafe { CFArray::wrap_under_get_rule(*value as CFArrayRef) };
            array.get(0).map(|certificate| certificate.clone())
        });
        Ok(Signing { team_id, identifier, leaf })
    }

    pub fn identify(peer: &Peer) -> Result<Caller, String> {
        let token = peer.token;
        let executable = executable(token)?;
        let bytes: Vec<u8> = token.iter().flat_map(|part| part.to_ne_bytes()).collect();
        let data = CFData::from_buffer(&bytes);
        let mut attributes = GuestAttributes::new();
        attributes.set_audit_token(data.as_concrete_TypeRef());
        let code = SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE).map_err(|e| format!("cannot find the code of {}: {e}", executable.display()))?;
        let any = SecRequirement::from_str("always").map_err(|e| e.to_string())?;
        match code.check_validity(Flags::NONE, &any) {
            Ok(()) => {}
            Err(e) if e.code() == NOT_SIGNED => return Ok(Caller::from_path(executable)),
            Err(e) => return Err(format!("the signature of {} is not valid: {e}", executable.display())),
        }
        // The check above reads the state of the process and the directory of
        // the signature, not each page of the file. A change to a page that
        // the process has not loaded shows only here. Connections do this
        // once, and a dictation never does. The resources of a bundle stay out:
        // a large app would take seconds.
        let url = CFURL::from_path(&executable, false).ok_or_else(|| format!("no URL for {}", executable.display()))?;
        let on_disk = SecStaticCode::from_path(&url, Flags::NONE).map_err(|e| format!("cannot read the file of {}: {e}", executable.display()))?;
        on_disk.check_validity(Flags::DO_NOT_VALIDATE_RESOURCES, &any).map_err(|e| format!("the signature of {} is not valid: {e}", executable.display()))?;
        let Signing { team_id, identifier, leaf } = signing_information(&code)?;
        let (Some(team_id), Some(identifier)) = (team_id.filter(|team| !team.is_empty()), identifier) else {
            // Ad hoc: each build of a developer has a new signature, so the path is the identity.
            return Ok(Caller::from_path(executable));
        };
        if !signed_by(&code, &team_id, &identifier) {
            return Err(format!("{} says that it has the team {team_id} and the identifier {identifier}, and its signature does not prove it", executable.display()));
        }
        let signer = leaf.and_then(|leaf| signer_name(&leaf.subject_summary(), &team_id));
        Ok(Caller { name: display_name(&executable), interpreter: is_interpreter(&executable), identity: Identity::Signed { team_id, identifier }, signer })
    }

    fn signed_by(code: &SecCode, team_id: &str, identifier: &str) -> bool {
        // The ids go into a requirement text, so they must be plain.
        let plain = |id: &str| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
        if !plain(team_id) || !plain(identifier) {
            return false;
        }
        let requirement = format!("anchor apple generic and certificate leaf[subject.OU] = \"{team_id}\" and identifier \"{identifier}\"");
        let Ok(requirement) = SecRequirement::from_str(&requirement) else { return false };
        code.check_validity(Flags::NONE, &requirement).is_ok()
    }

    /// The bundle name of the `.app` that has the executable, else the file name.
    pub fn display_name(executable: &Path) -> String {
        let bundle = executable.ancestors().skip(1).find(|folder| folder.extension().is_some_and(|extension| extension == "app"));
        bundle.and_then(bundle_name).unwrap_or_else(|| super::file_name(executable))
    }

    fn bundle_name(folder: &Path) -> Option<String> {
        let plist = CFURL::from_path(folder, true).and_then(CFBundle::new).map(|bundle| bundle.info_dictionary());
        let from_plist = plist.and_then(|info| {
            ["CFBundleDisplayName", "CFBundleName"].into_iter().find_map(|key| {
                let value = info.find(CFString::new(key))?;
                value.downcast::<CFString>().and_then(|name: CFString| clean_name(&name.to_string()))
            })
        });
        from_plist.or_else(|| folder.file_stem().and_then(|stem| clean_name(&stem.to_string_lossy())))
    }
}

#[cfg(windows)]
pub(crate) mod system {
    use super::{Caller, Peer};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_NAME_NORMALIZED, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFinalPathNameByHandleW, OPEN_EXISTING};
    use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};

    /// The client of `pipe`. The pipe keeps the pid of the process that connected.
    pub fn client(pipe: &crate::transport::windows::Pipe) -> Result<Peer, String> {
        let mut pid = 0u32;
        // SAFETY: the pipe is open for this call.
        if unsafe { GetNamedPipeClientProcessId(pipe.handle(), &mut pid) } == 0 {
            return Err(format!("no client process: {}", std::io::Error::last_os_error()));
        }
        Ok(Peer { pid })
    }

    fn image_path(peer: &Peer) -> Result<PathBuf, String> {
        let pid = peer.pid;
        // SAFETY: the process handle is open for these calls, and the buffer
        // has the size in `len`.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return Err(format!("cannot open process {pid}: {}", std::io::Error::last_os_error()));
            }
            let mut buffer = vec![0u16; 32 * 1024];
            let mut len = buffer.len() as u32;
            let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut len);
            let error = std::io::Error::last_os_error();
            CloseHandle(process);
            if ok == 0 {
                return Err(format!("no image path of process {pid}: {error}"));
            }
            Ok(PathBuf::from(std::ffi::OsString::from_wide(&buffer[..len as usize])))
        }
    }

    /// The final path of the file: no short names, no links, and the case of the disk.
    fn canonical(path: &std::path::Path) -> Result<PathBuf, String> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` ends with a zero, the handle is open for the second
        // call, and the buffer has the size that the call gets.
        unsafe {
            let file = CreateFileW(wide.as_ptr(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, std::ptr::null_mut());
            if file == INVALID_HANDLE_VALUE {
                return Err(format!("cannot open {}: {}", path.display(), std::io::Error::last_os_error()));
            }
            let mut buffer = vec![0u16; 32 * 1024];
            let len = GetFinalPathNameByHandleW(file, buffer.as_mut_ptr(), buffer.len() as u32, FILE_NAME_NORMALIZED);
            let error = std::io::Error::last_os_error();
            CloseHandle(file);
            if len == 0 || len as usize >= buffer.len() {
                return Err(format!("no final path of {}: {error}", path.display()));
            }
            let text = String::from_utf16_lossy(&buffer[..len as usize]);
            // `\\?\C:\x` is `C:\x`. `\\?\UNC\server\x` is `\\server\x`.
            let text = match text.strip_prefix(r"\\?\UNC\") {
                Some(rest) => format!(r"\\{rest}"),
                None => text.strip_prefix(r"\\?\").map_or_else(|| text.clone(), str::to_string),
            };
            Ok(PathBuf::from(text))
        }
    }

    pub fn identify(peer: &Peer) -> Result<Caller, String> {
        Ok(Caller::from_path(canonical(&image_path(peer)?)?))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod system {
    use super::{Caller, Peer};

    pub fn identify(_peer: &Peer) -> Result<Caller, String> {
        Err("the host cannot find the peer on this system".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_loses_the_characters_that_can_mislead() {
        assert_eq!(clean_name("Notes").as_deref(), Some("Notes"));
        assert_eq!(clean_name("  No\ttes\n").as_deref(), Some("Notes"));
        // U+202E turns the text around: "exe.txt" would read as "txt.exe".
        assert_eq!(clean_name("ev\u{202E}il\u{2066}\u{200F}").as_deref(), Some("evil"));
        assert_eq!(clean_name("\u{200B}\u{FEFF} "), None);
        // Tag characters, format characters, and letters that show as nothing.
        assert_eq!(clean_name("No\u{E0041}\u{E0067}tes\u{180E}\u{0600}\u{3164}\u{FFA0}\u{2028}").as_deref(), Some("Notes"));
        assert_eq!(clean_name(&"é".repeat(50)).unwrap().chars().count(), 40);
        // The cut does not leave a space at the end.
        assert_eq!(clean_name(&format!("{} x", "a".repeat(39))).unwrap().chars().count(), 39);
    }

    #[test]
    fn known_interpreters_are_found_by_their_file_name() {
        for name in ["python3", "python3.12", "Python", "node", "node.exe", "bash", "zsh", "ruby", "perl5.34", "pwsh", "osascript", "pythonw.exe", "javaw.exe", "nodejs", "pypy3", "gawk", "mawk", "python3-intel64", "python3.12-intel64", "busybox", "wish", "wish8.6", "ld-linux-x86-64.so.2", "ld-linux-aarch64.so.1", "ld-musl-x86_64.so.1", "ld.so"] {
            assert!(is_interpreter(Path::new(name)), "{name}");
        }
        for name in ["notes", "transcribe", "pythonista", "sayso-ext", "node-gyp-helper", "ldd", "wishlist"] {
            assert!(!is_interpreter(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn the_signer_is_the_name_on_the_certificate() {
        assert_eq!(signer_name("Developer ID Application: Example LLC (ABCDE12345)", "ABCDE12345").as_deref(), Some("Example LLC"));
        assert_eq!(signer_name("Apple Development: Chris (XYZ)", "ABCDE12345").as_deref(), Some("Chris (XYZ)"));
        assert_eq!(signer_name("Plain Name", "ABCDE12345").as_deref(), Some("Plain Name"));
    }

    #[test]
    fn a_link_to_a_deleted_file_is_found() {
        assert!(deleted_link(Path::new("/opt/app/bin/app (deleted)")));
        assert!(!deleted_link(Path::new("/opt/app/bin/app")));
    }

    #[test]
    fn an_identity_gives_one_key() {
        let signed = Identity::Signed { team_id: "ABCDE12345".into(), identifier: "app.example".into() };
        assert_eq!(signed.id(), "signed:ABCDE12345/app.example");
        let lower = Identity::Path("/opt/App".into()).id();
        assert_eq!(lower, if cfg!(windows) { "path:/opt/app" } else { "path:/opt/App" });
    }

    #[test]
    fn a_path_is_never_the_key_of_a_signed_app() {
        let signed = Identity::Signed { team_id: "TEAM".into(), identifier: "app.x".into() };
        assert_ne!(Identity::Path("TEAM/app.x".into()).id(), signed.id());
        assert_ne!(Identity::Path("signed:TEAM/app.x".into()).id(), signed.id());
    }

    #[cfg(unix)]
    #[test]
    fn paths_that_are_not_utf8_keep_two_keys() {
        use std::os::unix::ffi::OsStrExt;
        let path = |byte: u8| Identity::Path(PathBuf::from(std::ffi::OsStr::from_bytes(&[b'/', b'x', byte])));
        assert_ne!(path(0xFE).id(), path(0xFF).id());
        assert_ne!(path(0xFF).id(), Identity::Path("/x\u{FFFD}".into()).id(), "not the text that shows");
    }

    #[test]
    fn a_path_shows_each_character_that_can_mislead() {
        let path = Path::new("/opt/ev\u{202E}il\nname\u{E0041}");
        assert_eq!(visible_path(path), "/opt/ev\u{FFFD}il\u{FFFD}name\u{FFFD}");
        assert_eq!(visible_path(Path::new("/opt/My App/é")), "/opt/My App/é");
    }

    #[test]
    fn the_name_from_the_file_name_has_nothing_hidden() {
        assert_eq!(file_name(Path::new("/opt/ev\u{202E}il\nname")), "evilname");
    }
}
