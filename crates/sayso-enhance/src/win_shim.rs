//! Windows: start command line tools without a console window, and look
//! through npm's `.cmd` shims.
//!
//! npm installs `claude` and `codex` as `claude.cmd` and `codex.cmd`, which
//! run `node <package>\cli.js %*`. A batch file goes through `cmd.exe`, and
//! `cmd.exe` cannot pass a multi-line system prompt or a JSON schema intact.
//! So Sayso reads the shim and runs node with the script itself.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `CREATE_NO_WINDOW`: a GUI app's child gets no console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A command for `program`: node and the script for an npm shim, else the program.
pub(crate) fn command(program: &Path) -> Command {
    use std::os::windows::process::CommandExt;
    let mut command = match shim_target(program) {
        Some((node, script)) => {
            let mut c = Command::new(node);
            c.arg(script);
            c
        }
        None => Command::new(program),
    };
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

/// For an npm `.cmd` shim: the node binary and the script it runs.
fn shim_target(program: &Path) -> Option<(PathBuf, PathBuf)> {
    let ext = program.extension()?.to_string_lossy().to_ascii_lowercase();
    if ext != "cmd" && ext != "bat" {
        return None;
    }
    let text = std::fs::read_to_string(program).ok()?;
    let dir = program.parent()?;
    let script = script_in_shim(&text, dir)?;
    // npm puts node.exe beside the shim when node is not on PATH.
    let local_node = dir.join("node.exe");
    let node = if local_node.is_file() { local_node } else { PathBuf::from("node") };
    Some((node, script))
}

/// The script path in a shim: the quoted `%dp0%\...\*.js` (npm's cmd-shim)
/// or `%~dp0\...\*.js` argument, resolved against the shim's folder.
fn script_in_shim(text: &str, dir: &Path) -> Option<PathBuf> {
    for quoted in text.split('"').skip(1).step_by(2) {
        let lower = quoted.to_ascii_lowercase();
        let rest = ["%dp0%\\", "%~dp0\\", "%dp0%/", "%~dp0/"].iter().find_map(|p| lower.starts_with(p).then(|| &quoted[p.len()..]));
        if let Some(rest) = rest
            && (lower.ends_with(".js") || lower.ends_with(".mjs") || lower.ends_with(".cjs"))
        {
            let path = dir.join(rest.replace('/', "\\"));
            return path.is_file().then_some(path);
        }
    }
    None
}

/// The executable extensions to try for a bare name, from `PATHEXT`.
pub(crate) fn executable_names(name: &str) -> Vec<String> {
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    // .exe first: a native install beats an npm shim in the same folder.
    let mut exts: Vec<String> = pathext.split(';').filter(|e| !e.is_empty()).map(|e| e.to_ascii_lowercase()).collect();
    exts.sort_by_key(|e| e != ".exe");
    exts.into_iter().map(|e| format!("{name}{e}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NPM_SHIM: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\cli.js\" %*\r\n";

    #[test]
    fn reads_the_script_of_an_npm_shim() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("node_modules\\@anthropic-ai\\claude-code\\cli.js");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "").unwrap();
        let shim = dir.path().join("claude.cmd");
        std::fs::write(&shim, NPM_SHIM).unwrap();
        let (node, found) = shim_target(&shim).unwrap();
        assert_eq!(found, script);
        assert_eq!(node, PathBuf::from("node"));
        std::fs::write(dir.path().join("node.exe"), "").unwrap();
        assert_eq!(shim_target(&shim).unwrap().0, dir.path().join("node.exe"));
    }

    #[test]
    fn a_shim_whose_script_is_missing_runs_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("claude.cmd");
        std::fs::write(&shim, NPM_SHIM).unwrap();
        assert_eq!(shim_target(&shim), None);
        assert_eq!(shim_target(Path::new("C:\\tools\\claude.exe")), None);
    }

    #[test]
    fn exe_comes_before_other_extensions() {
        let names = executable_names("codex");
        assert_eq!(names.first().map(String::as_str), Some("codex.exe"));
        assert!(names.contains(&"codex.cmd".to_string()));
    }
}
