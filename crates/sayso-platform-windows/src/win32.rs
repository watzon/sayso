//! Small helpers around the `windows` crate: wide strings, environment
//! variables, registry values and keys, and handles that close themselves.

use windows::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HANDLE, WIN32_ERROR};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Registry::{
    HKEY, KEY_READ, REG_BINARY, REG_ROUTINE_FLAGS, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    RegCloseKey, RegDeleteKeyValueW, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, RegSetKeyValueW,
};
use windows::core::{PCWSTR, PWSTR};

/// A NUL-terminated UTF-16 copy of `s`.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The UTF-16 text up to the first NUL.
pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// A kernel handle that is closed when dropped.
pub struct OwnedHandle(pub HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: we own this handle and close it once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// `s` with every `%NAME%` replaced by the value of that environment
/// variable. A name that is not set stays as it is.
pub fn expand_env(s: &str) -> String {
    let src = wide(s);
    // SAFETY: `src` is NUL-terminated. The first call only asks for the
    // size; the second writes at most the length of the buffer.
    unsafe {
        let size = ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), None);
        if size == 0 {
            return s.to_string();
        }
        let mut buf = vec![0u16; size as usize];
        let written = ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), Some(&mut buf));
        if written == 0 || written as usize > buf.len() {
            return s.to_string();
        }
        from_wide(&buf)
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Raw bytes of a registry value of the given type, or `None` when it is missing.
fn read_raw(root: HKEY, subkey: &str, value: &str, flags: REG_ROUTINE_FLAGS) -> Option<Vec<u8>> {
    let (subkey, value) = (wide(subkey), wide(value));
    let (subkey, value) = (PCWSTR(subkey.as_ptr()), PCWSTR(value.as_ptr()));
    let mut size = 0u32;
    // SAFETY: the strings are NUL-terminated and live for the calls. The
    // first call only asks for the size; the second fills a buffer of that size.
    unsafe {
        if RegGetValueW(root, subkey, value, flags, None, None, Some(&mut size)) != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        if RegGetValueW(root, subkey, value, flags, None, Some(buf.as_mut_ptr().cast()), Some(&mut size))
            != ERROR_SUCCESS
        {
            return None;
        }
        buf.truncate(size as usize);
        Some(buf)
    }
}

/// A `REG_SZ` (or `REG_EXPAND_SZ`, expanded) value.
pub fn read_string(root: HKEY, subkey: &str, value: &str) -> Option<String> {
    let bytes = read_raw(root, subkey, value, RRF_RT_REG_SZ)?;
    Some(from_wide(&units_from_bytes(&bytes)))
}

/// The names of the keys directly under a key. Empty when the key is missing.
pub fn subkey_names(root: HKEY, subkey: &str) -> Vec<String> {
    let path = wide(subkey);
    let mut key = HKEY::default();
    // SAFETY: the string is NUL-terminated, and `key` is an out-pointer to a local.
    if unsafe { RegOpenKeyExW(root, PCWSTR(path.as_ptr()), None, KEY_READ, &mut key) } != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut names = Vec::new();
    // A key name has at most 255 characters.
    let mut buf = [0u16; 256];
    for index in 0.. {
        let mut len = buf.len() as u32;
        // SAFETY: `buf` holds `len` units, and the call writes at most that many.
        let status =
            unsafe { RegEnumKeyExW(key, index, Some(PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None) };
        if status != ERROR_SUCCESS {
            break;
        }
        names.push(String::from_utf16_lossy(&buf[..len as usize]));
    }
    // SAFETY: we opened this key and close it once.
    let _ = unsafe { RegCloseKey(key) };
    names
}

/// Little-endian bytes as UTF-16 units.
pub fn units_from_bytes(bytes: &[u8]) -> Vec<u16> {
    bytes.as_chunks::<2>().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect()
}

pub fn read_dword(root: HKEY, subkey: &str, value: &str) -> Option<u32> {
    let bytes = read_raw(root, subkey, value, RRF_RT_REG_DWORD)?;
    Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
}

pub fn read_binary(root: HKEY, subkey: &str, value: &str) -> Option<Vec<u8>> {
    read_raw(root, subkey, value, RRF_RT_REG_BINARY)
}

fn check(status: WIN32_ERROR, what: &str) -> Result<(), String> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("{what}: {}", windows::core::Error::from(status.to_hresult())))
    }
}

/// Write a `REG_SZ` value. Creates the key when it is missing.
pub fn write_string(root: HKEY, subkey: &str, value: &str, data: &str) -> Result<(), String> {
    let (key, name, data) = (wide(subkey), wide(value), wide(data));
    // SAFETY: the strings are NUL-terminated, and the size covers `data` with its NUL.
    let status = unsafe {
        RegSetKeyValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    };
    check(status, "cannot write the registry value")
}

/// Write a `REG_BINARY` value. Creates the key when it is missing.
pub fn write_binary(root: HKEY, subkey: &str, value: &str, data: &[u8]) -> Result<(), String> {
    let (key, name) = (wide(subkey), wide(value));
    // SAFETY: the strings are NUL-terminated, and the size is the slice length.
    let status = unsafe {
        RegSetKeyValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            REG_BINARY.0,
            Some(data.as_ptr().cast()),
            data.len() as u32,
        )
    };
    check(status, "cannot write the registry value")
}

/// Delete a value. A value or key that does not exist is not an error.
pub fn delete_value(root: HKEY, subkey: &str, value: &str) -> Result<(), String> {
    let (key, name) = (wide(subkey), wide(value));
    // SAFETY: the strings are NUL-terminated.
    let status = unsafe { RegDeleteKeyValueW(root, PCWSTR(key.as_ptr()), PCWSTR(name.as_ptr())) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    check(status, "cannot delete the registry value")
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Registry::HKEY_CURRENT_USER;

    #[test]
    fn wide_strings_round_trip() {
        let w = wide("héllo");
        assert_eq!(w.last(), Some(&0));
        assert_eq!(from_wide(&w), "héllo");
        assert_eq!(from_wide(&[0x41, 0, 0x42]), "A");
        assert_eq!(from_wide(&[0x41, 0x42]), "AB");
    }

    #[test]
    fn missing_registry_values_are_none() {
        let key = r"Software\Sayso\ThisKeyDoesNotExist";
        assert_eq!(read_string(HKEY_CURRENT_USER, key, "Nothing"), None);
        assert_eq!(read_dword(HKEY_CURRENT_USER, key, "Nothing"), None);
        assert_eq!(read_binary(HKEY_CURRENT_USER, key, "Nothing"), None);
        assert_eq!(delete_value(HKEY_CURRENT_USER, key, "Nothing"), Ok(()));
        assert!(subkey_names(HKEY_CURRENT_USER, key).is_empty());
    }

    #[test]
    fn lists_the_keys_under_a_key_every_windows_has() {
        let names = subkey_names(HKEY_CURRENT_USER, "Software");
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case("Microsoft")));
    }

    #[test]
    fn environment_variables_are_expanded() {
        let expanded = expand_env(r"%WINDIR%\explorer.exe");
        assert!(!expanded.contains('%') && expanded.ends_with("explorer.exe"));
        assert_eq!(expand_env("%SaysoNoSuchVariable%"), "%SaysoNoSuchVariable%");
        assert_eq!(expand_env("plain"), "plain");
    }

    #[test]
    fn reads_a_value_every_windows_has() {
        let name = read_string(
            windows::Win32::System::Registry::HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "ProductName",
        );
        assert!(name.is_some_and(|n| n.starts_with("Windows")));
    }
}
