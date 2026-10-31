//! An app that sees other files than the host (`docs/sdk.md`, "Flatpak"). A path
//! that such an app names means a file in its own view. The host never opens that
//! path with its own view: a sandboxed app could then read each file of the user.

use crate::identity::Caller;
use std::fs::File;
use std::path::{Path, PathBuf};

/// What `sayso-bridge` says about the app of a connection.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) struct Attested {
    pub caller: Result<Caller, String>,
    pub files: FileAccess,
}

/// How the host opens a file that the app of a connection names.
pub(crate) enum FileAccess {
    /// The app sees the files that the host sees, and the host uses its path.
    Same,
    /// An app in a Flatpak sandbox, and the host outside it.
    #[cfg(target_os = "linux")]
    InView(sayso_bridge::files::View),
    /// `sayso-bridge` opened the file and sent it.
    #[cfg(target_os = "linux")]
    FromBridge(std::sync::Arc<std::sync::Mutex<sayso_bridge::wire::WaitingFiles>>),
}

/// A file with a path that opens it again in this process, while it is open.
pub(crate) struct OpenedFile {
    pub path: PathBuf,
    _file: File,
}

impl OpenedFile {
    #[cfg(target_os = "linux")]
    fn new(file: File) -> OpenedFile {
        OpenedFile { path: sayso_bridge::files::path_of(&file), _file: file }
    }
}

impl FileAccess {
    /// The way to open the files of `caller`, who wrote as `peer`.
    #[cfg(target_os = "linux")]
    pub fn of(caller: &Caller, peer: &crate::Peer) -> Result<FileAccess, String> {
        match caller.identity {
            crate::Identity::Flatpak { .. } => sayso_bridge::files::View::of(peer.pid).map(FileAccess::InView).map_err(|e| format!("cannot open process {}: {e}", peer.pid)),
            _ => Ok(FileAccess::Same),
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn of(_caller: &Caller, _peer: &crate::Peer) -> Result<FileAccess, String> {
        Ok(FileAccess::Same)
    }

    pub fn has_another_view(&self) -> bool {
        !matches!(self, FileAccess::Same)
    }

    /// The file at `path` for a job. None: the job uses `path` as it is. An
    /// error is an English sentence for the reply.
    #[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
    pub fn open(&self, path: &Path) -> Result<Option<OpenedFile>, String> {
        match self {
            FileAccess::Same => Ok(None),
            #[cfg(target_os = "linux")]
            FileAccess::InView(view) => view.open(path).map(|file| Some(OpenedFile::new(file))).map_err(|e| format!("cannot open {} in the sandbox of the app: {e}", path.display())),
            #[cfg(target_os = "linux")]
            FileAccess::FromBridge(files) => {
                let file = path.to_str().and_then(|path| files.lock().unwrap_or_else(|e| e.into_inner()).take(path));
                file.map(|file| Some(OpenedFile::new(file))).ok_or_else(|| "cannot open this file. Check that it is an audio file that the app can read".to_string())
            }
        }
    }
}
