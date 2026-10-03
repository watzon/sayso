//! Windows: embed the app icon and the version information in sayso.exe.
//!
//! GPUI loads icon resource 1 for its windows, and Explorer and the taskbar
//! show it for the file. The resource script is compiled with `rc.exe` from
//! the Windows SDK, so no build dependency is needed. Without the SDK the
//! build goes on with a warning and the exe has no icon.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app/AppIcon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resources();
    }
}

fn windows_resources() {
    use std::path::PathBuf;
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let icon = manifest.join("../../assets/app/AppIcon.ico");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let numbers: Vec<u32> = version.split(['.', '-']).take(3).map(|p| p.parse().unwrap_or(0)).collect();
    let (major, minor, patch) = (numbers[0], numbers.get(1).copied().unwrap_or(0), numbers.get(2).copied().unwrap_or(0));
    let rc = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Watzon Ventures LLC"
      VALUE "FileDescription", "Sayso"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "sayso"
      VALUE "LegalCopyright", "GPL-3.0-only"
      VALUE "OriginalFilename", "sayso.exe"
      VALUE "ProductName", "Sayso"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = icon.display().to_string().replace('\\', "\\\\"),
    );
    let rc_file = out.join("sayso.rc");
    let res_file = out.join("sayso.res");
    std::fs::write(&rc_file, rc).unwrap();
    let Some(rc_exe) = find_rc() else {
        println!("cargo:warning=rc.exe from the Windows SDK was not found; sayso.exe gets no icon");
        return;
    };
    let status = std::process::Command::new(rc_exe).arg("/nologo").arg("/fo").arg(&res_file).arg(&rc_file).status();
    match status {
        // The linker takes a compiled resource file as an input.
        Ok(s) if s.success() => println!("cargo:rustc-link-arg-bins={}", res_file.display()),
        other => println!("cargo:warning=rc.exe failed ({other:?}); sayso.exe gets no icon"),
    }
}

/// `rc.exe` on PATH (a developer prompt), else the newest one in the Windows SDK.
fn find_rc() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    if let Some(path) = std::env::var_os("PATH") {
        if let Some(found) = std::env::split_paths(&path).map(|d| d.join("rc.exe")).find(|p| p.is_file()) {
            return Some(found);
        }
    }
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86") => "x86",
        _ => "x64",
    };
    let kits = PathBuf::from(std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into()))
        .join(r"Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&kits).ok()?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    versions.sort();
    versions.into_iter().rev().map(|v| v.join(arch).join("rc.exe")).find(|p| p.is_file())
}
