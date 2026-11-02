//! The ACP registry: the agents that the Agent Client Protocol project lists
//! (<https://agentclientprotocol.com/registry>), and how to start them.
//!
//! An agent comes as an `npx` package, a `uvx` package, or a download for
//! each platform. Sayso installs a download in a folder that the caller
//! chooses, and checks its SHA-256 when the registry gives one.

use crate::openai::{agent, map_ureq_error};
use sayso_core::enhance::EnhanceError;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// The registry file.
pub const REGISTRY_URL: &str = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

/// The largest download that Sayso installs.
const MAX_DOWNLOAD: u64 = 1024 * 1024 * 1024;

/// The registry file: its version and its agents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    pub version: String,
    pub agents: Vec<RegistryAgent>,
}

/// One agent of the registry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RegistryAgent {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// The URL of an SVG icon.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub distribution: Distribution,
}

/// The ways an agent comes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Distribution {
    #[serde(default)]
    pub npx: Option<Package>,
    #[serde(default)]
    pub uvx: Option<Package>,
    /// The downloads, by platform: `darwin-aarch64`, `linux-x86_64`, `windows-x86_64`, ...
    #[serde(default)]
    pub binary: BTreeMap<String, Binary>,
}

/// An `npx` or `uvx` package.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Package {
    /// The package with its version, for example `@google/gemini-cli@0.66.0`.
    pub package: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// The download of one platform.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Binary {
    /// The URL of the archive (`.tar.gz`, `.tgz`, `.tar.bz2`, `.zip`), or of the program itself.
    pub archive: String,
    /// The program, relative to the folder of the archive, for example `./opencode`.
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// The SHA-256 of the archive, in hex. Some entries have none.
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// How to start an agent: the command, its arguments, and extra environment variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// A bare name (`npx`, `uvx`) that Sayso searches, or the path of a program.
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// Read the registry file. An agent entry that does not parse is skipped.
pub fn parse_registry(text: &str) -> Result<Registry, EnhanceError> {
    let value: Value = serde_json::from_str(text).map_err(|e| EnhanceError::InvalidOutput(format!("the ACP registry is not JSON: {e}")))?;
    let list = value["agents"].as_array().ok_or_else(|| EnhanceError::InvalidOutput("the ACP registry has no agent list".into()))?;
    let agents = list
        .iter()
        .filter_map(|entry| match RegistryAgent::deserialize(entry) {
            Ok(agent) => Some(agent),
            Err(e) => {
                log::warn!("skipped the ACP registry entry {}: {e}", entry["id"]);
                None
            }
        })
        .collect();
    Ok(Registry { version: value["version"].as_str().unwrap_or_default().to_string(), agents })
}

/// The registry from the cache file, when there is a readable one.
pub fn cached_registry(cache: &Path) -> Option<Registry> {
    parse_registry(&std::fs::read_to_string(cache).ok()?).ok()
}

/// Download the registry and save it in the `cache` file. When the download
/// fails, the cache is used; the error comes back only when there is no cache.
pub fn fetch_registry(cache: &Path, timeout: Duration) -> Result<Registry, EnhanceError> {
    fetch_registry_from(REGISTRY_URL, cache, timeout)
}

fn fetch_registry_from(url: &str, cache: &Path, timeout: Duration) -> Result<Registry, EnhanceError> {
    let fetched = download_text(url, timeout).and_then(|text| parse_registry(&text).map(|registry| (text, registry)));
    match fetched {
        Ok((text, registry)) => {
            if let Err(e) = write_atomic(cache, text.as_bytes()) {
                log::warn!("could not save the ACP registry in {}: {e}", cache.display());
            }
            Ok(registry)
        }
        Err(error) => match cached_registry(cache) {
            Some(registry) => {
                log::info!("the ACP registry did not load ({error}), using the saved copy");
                Ok(registry)
            }
            None => Err(error),
        },
    }
}

fn download_text(url: &str, timeout: Duration) -> Result<String, EnhanceError> {
    let timeout_ms = timeout.as_millis() as u64;
    let mut response =
        agent(url).get(url).config().timeout_global(Some(timeout)).http_status_as_error(false).build().call().map_err(|e| map_ureq_error(e, timeout_ms))?;
    let status = response.status().as_u16();
    let text = response.body_mut().with_config().limit(16 * 1024 * 1024).read_to_string().map_err(|e| map_ureq_error(e, timeout_ms))?;
    if status >= 400 {
        return Err(EnhanceError::Http { status, body: text.chars().take(500).collect() });
    }
    Ok(text)
}

/// Write a file through a temporary file in the same folder, so a reader never sees half of it.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(bytes)?;
    file.persist(path).map(|_| ()).map_err(|e| e.error)
}

/// The registry name of this platform, for example `darwin-aarch64`. None for
/// a platform that the registry does not have.
pub fn current_platform() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-aarch64",
        ("macos", "x86_64") => "darwin-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        ("linux", "x86_64") => "linux-x86_64",
        ("windows", "aarch64") => "windows-aarch64",
        ("windows", "x86_64") => "windows-x86_64",
        _ => return None,
    })
}

impl RegistryAgent {
    /// The download for this computer.
    pub fn binary(&self) -> Option<&Binary> {
        self.distribution.binary.get(current_platform()?)
    }
}

/// The program of a download, relative to its folder, without `./`. Err for
/// a path that leaves the folder.
fn relative_cmd(cmd: &str) -> Result<PathBuf, EnhanceError> {
    let normal = cmd.trim().replace('\\', "/");
    let path = Path::new(&normal);
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(name) => out.push(name),
            Component::CurDir => {}
            _ => return Err(EnhanceError::InvalidOutput(format!("the registry gives an unsafe program path \"{cmd}\""))),
        }
    }
    if out.as_os_str().is_empty() {
        return Err(EnhanceError::InvalidOutput("the registry gives no program for the download".into()));
    }
    Ok(out)
}

/// The folder of an installed agent: `<install_root>/<id>/<version>`.
fn install_dir(agent: &RegistryAgent, install_root: &Path) -> Result<PathBuf, EnhanceError> {
    let safe = |part: &str| !part.is_empty() && part != "." && part != ".." && !part.contains(['/', '\\']);
    if !safe(&agent.id) || !safe(&agent.version) {
        return Err(EnhanceError::InvalidOutput(format!("the registry gives an unsafe id or version for \"{}\"", agent.id)));
    }
    Ok(install_root.join(&agent.id).join(&agent.version))
}

/// The installed program of the download for this computer. None when it is
/// not installed, or when there is no download for this computer.
pub fn installed_binary(agent: &RegistryAgent, install_root: &Path) -> Option<PathBuf> {
    let program = install_dir(agent, install_root).ok()?.join(relative_cmd(&agent.binary()?.cmd).ok()?);
    program.is_file().then_some(program)
}

/// How to start a registry agent. In order: the installed download, `npx`,
/// `uvx`. `NotConfigured` when the download is not installed yet (call
/// [`install_binary`]), or when `npx` or `uvx` is not on this computer.
pub fn resolve_launch(agent: &RegistryAgent, install_root: &Path) -> Result<Launch, EnhanceError> {
    if let (Some(program), Some(binary)) = (installed_binary(agent, install_root), agent.binary()) {
        return Ok(Launch { command: program.to_string_lossy().into_owned(), args: binary.args.clone(), env: binary.env.clone() });
    }
    let package = |tool: &str, package: &Package, extra: &[&str]| Launch {
        command: tool.to_string(),
        args: extra.iter().map(|a| a.to_string()).chain([package.package.clone()]).chain(package.args.iter().cloned()).collect(),
        env: package.env.clone(),
    };
    let dist = &agent.distribution;
    if let Some(npx) = &dist.npx
        && crate::detect::detect_command("npx").is_some()
    {
        return Ok(package("npx", npx, &["-y"]));
    }
    if let Some(uvx) = &dist.uvx
        && crate::detect::detect_command("uvx").is_some()
    {
        return Ok(package("uvx", uvx, &[]));
    }
    let name = &agent.name;
    Err(EnhanceError::NotConfigured(if agent.binary().is_some() {
        format!("{name} is not installed yet")
    } else if dist.npx.is_some() {
        format!("{name} needs npx, which comes with Node.js; install Node.js and try again")
    } else if dist.uvx.is_some() {
        format!("{name} needs uvx, which comes with uv; install uv and try again")
    } else {
        format!("{name} has no download for this computer")
    }))
}

/// Download the agent for this computer into `<install_root>/<id>/<version>`,
/// check its SHA-256, unpack it, and make the program executable. Returns the
/// path of the program. An agent that is installed already is not downloaded again.
pub fn install_binary(agent: &RegistryAgent, install_root: &Path, timeout: Duration) -> Result<PathBuf, EnhanceError> {
    let binary = agent.binary().ok_or_else(|| EnhanceError::NotConfigured(format!("{} has no download for this computer", agent.name)))?;
    let dir = install_dir(agent, install_root)?;
    let cmd = relative_cmd(&binary.cmd)?;
    if dir.join(&cmd).is_file() {
        return Ok(dir.join(&cmd));
    }
    let parent = dir.parent().unwrap_or(install_root);
    std::fs::create_dir_all(parent).map_err(|e| io_error("could not create", parent, e))?;
    let work = tempfile::tempdir_in(parent).map_err(|e| io_error("could not create a folder in", parent, e))?;
    let archive = work.path().join("download");
    download_file(&binary.archive, &archive, binary.sha256.as_deref(), timeout)?;
    let unpacked = work.path().join("unpacked");
    unpack(&archive, archive_name(&binary.archive), &cmd, &unpacked)?;
    let program = unpacked.join(&cmd);
    if !program.is_file() {
        return Err(EnhanceError::InvalidOutput(format!("the download of {} has no {}", agent.name, cmd.display())));
    }
    make_executable(&program)?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| io_error("could not remove", &dir, e))?;
    }
    std::fs::rename(&unpacked, &dir).map_err(|e| io_error("could not move the download to", &dir, e))?;
    Ok(dir.join(&cmd))
}

fn io_error(what: &str, path: &Path, e: std::io::Error) -> EnhanceError {
    EnhanceError::Cli(format!("{what} {}: {e}", path.display()))
}

/// The file name in a URL, without the query.
fn archive_name(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/').next().unwrap_or(path)
}

/// Download `url` into `dest` and check the SHA-256 when there is one.
fn download_file(url: &str, dest: &Path, sha256: Option<&str>, timeout: Duration) -> Result<(), EnhanceError> {
    let timeout_ms = timeout.as_millis() as u64;
    let response =
        agent(url).get(url).config().timeout_global(Some(timeout)).http_status_as_error(false).build().call().map_err(|e| map_ureq_error(e, timeout_ms))?;
    let status = response.status().as_u16();
    if status >= 400 {
        return Err(EnhanceError::Http { status, body: format!("could not download {url}") });
    }
    let mut reader = response.into_body().into_with_config().limit(MAX_DOWNLOAD).reader();
    let mut file = std::fs::File::create(dest).map_err(|e| io_error("could not create", dest, e))?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| match e.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => EnhanceError::Timeout(timeout_ms),
            _ => EnhanceError::Network(format!("the download of {url} broke: {e}")),
        })?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| io_error("could not write", dest, e))?;
    }
    check_sha256(&format!("{:x}", hash.finalize()), sha256, url)
}

/// Compare the SHA-256 of a download with the one of the registry.
fn check_sha256(actual: &str, expected: Option<&str>, url: &str) -> Result<(), EnhanceError> {
    match expected.map(str::trim).filter(|e| !e.is_empty()) {
        Some(expected) if !expected.eq_ignore_ascii_case(actual) => {
            Err(EnhanceError::InvalidOutput(format!("the download of {url} has the SHA-256 {actual}, and the registry gives {expected}")))
        }
        Some(_) => Ok(()),
        None => {
            log::warn!("the ACP registry gives no SHA-256 for {url}; it is not checked");
            Ok(())
        }
    }
}

/// Unpack an archive into `dest`, by the extension of its name. A file that
/// is no archive is the program itself.
fn unpack(archive: &Path, name: &str, cmd: &Path, dest: &Path) -> Result<(), EnhanceError> {
    std::fs::create_dir_all(dest).map_err(|e| io_error("could not create", dest, e))?;
    let open = || std::fs::File::open(archive).map_err(|e| io_error("could not open", archive, e));
    let broken = |e: std::io::Error| EnhanceError::InvalidOutput(format!("could not unpack {name}: {e}"));
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(open()?).map_err(|e| EnhanceError::InvalidOutput(format!("could not unpack {name}: {e}")))?;
        // `extract` keeps every entry inside `dest`.
        zip.extract(dest).map_err(|e| EnhanceError::InvalidOutput(format!("could not unpack {name}: {e}")))
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        // `unpack` skips entries that would leave `dest`.
        tar::Archive::new(flate2::read::GzDecoder::new(open()?)).unpack(dest).map_err(broken)
    } else if lower.ends_with(".tar.bz2") || lower.ends_with(".tbz2") {
        tar::Archive::new(bzip2::read::BzDecoder::new(open()?)).unpack(dest).map_err(broken)
    } else if lower.ends_with(".tar") {
        tar::Archive::new(open()?).unpack(dest).map_err(broken)
    } else {
        let program = dest.join(cmd);
        if let Some(dir) = program.parent() {
            std::fs::create_dir_all(dir).map_err(|e| io_error("could not create", dir, e))?;
        }
        std::fs::copy(archive, &program).map(|_| ()).map_err(|e| io_error("could not copy to", &program, e))
    }
}

#[cfg(unix)]
fn make_executable(program: &Path) -> Result<(), EnhanceError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(program).map_err(|e| io_error("could not read", program, e))?.permissions().mode();
    std::fs::set_permissions(program, std::fs::Permissions::from_mode(mode | 0o755)).map_err(|e| io_error("could not make executable", program, e))
}

#[cfg(not(unix))]
fn make_executable(_program: &Path) -> Result<(), EnhanceError> {
    Ok(())
}

/// Agents whose own command speaks ACP, by registry id: the command and its
/// arguments. Each one was checked with the tool's help and an `initialize`
/// handshake.
const LOCAL_COMMANDS: &[(&str, &str, &[&str])] = &[
    ("opencode", "opencode", &["acp"]),
    ("gemini", "gemini", &["--acp"]),
    ("cursor", "cursor-agent", &["acp"]),
];

/// The agent's own command when it is on this computer, for a registry agent
/// that has one (see [`LOCAL_COMMANDS`]). The UI shows it as found and starts
/// it instead of a download.
pub fn local_command(agent_id: &str) -> Option<Launch> {
    let (_, name, args) = LOCAL_COMMANDS.iter().find(|(id, _, _)| *id == agent_id)?;
    let program = crate::detect::detect_command(name)?;
    Some(Launch { command: program.to_string_lossy().into_owned(), args: args.iter().map(|a| a.to_string()).collect(), env: BTreeMap::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed copy of the registry of 2026-11-02, one entry of each kind.
    const REGISTRY: &str = r#"{
      "version": "1.0.0",
      "agents": [
        {
          "id": "opencode", "name": "OpenCode", "version": "2.2.5", "description": "The open source coding agent",
          "repository": "https://github.com/anomalyco/opencode", "website": "https://opencode.ai", "authors": ["Anomaly"],
          "license": "MIT", "license_url": "https://github.com/anomalyco/opencode/blob/dev/LICENSE",
          "icon": "https://cdn.agentclientprotocol.com/registry/v1/latest/opencode.svg",
          "distribution": { "binary": {
            "darwin-aarch64": { "archive": "https://github.com/anomalyco/opencode/releases/download/v2.2.5/opencode-darwin-arm64.zip", "cmd": "./opencode", "args": ["acp"], "sha256": "25ba9908979fa7d764792c82589b4b5768ab1511154e0317d98aa77d216ce767" },
            "linux-x86_64": { "archive": "https://github.com/anomalyco/opencode/releases/download/v2.2.5/opencode-linux-x64.tar.gz", "cmd": "./opencode", "args": ["acp"], "sha256": "09f44302c10fd036912f59c81fd1aef5df45dd06f649197f9dfc9bbe2a79614e" },
            "windows-x86_64": { "archive": "https://github.com/anomalyco/opencode/releases/download/v2.2.5/opencode-windows-x64.zip", "cmd": "./opencode.exe", "args": ["acp"], "sha256": "2f3bfd6b3527c60c2a882557b20ddc9ba6bdff4b5a8fe61c853da8fcfb3076e8" }
          } }
        },
        {
          "id": "claude-acp", "name": "Claude Agent", "version": "0.100.0", "description": "ACP wrapper for Anthropic's Claude",
          "authors": ["Anthropic", "Zed Industries", "JetBrains"], "license": "proprietary",
          "distribution": { "npx": { "package": "@agentclientprotocol/claude-agent-acp@0.100.0" } },
          "icon": "https://cdn.agentclientprotocol.com/registry/v1/latest/claude-acp.svg"
        },
        {
          "id": "fast-agent", "name": "fast-agent", "version": "0.10.1", "description": "Code and build agents",
          "distribution": { "uvx": { "package": "fast-agent-acp==0.10.1", "args": ["-x"], "env": { "FAST_AGENT_MODEL": "codexplan" } } }
        },
        {
          "id": "vtcode", "name": "VT Code", "version": "0.96.14", "description": "Semantic coding agent",
          "distribution": { "binary": { "darwin-aarch64": { "archive": "https://example.test/vtcode.tar.gz", "cmd": "./vtcode", "args": ["acp"], "env": { "VT_ACP_ENABLED": "1" } } } }
        },
        { "id": "broken", "distribution": {} }
      ],
      "extensions": []
    }"#;

    fn agent_with_binary(archive: &str, cmd: &str, sha256: Option<String>) -> RegistryAgent {
        let mut agent = parse_registry(REGISTRY).unwrap().agents.remove(0);
        let entry = Binary { archive: archive.into(), cmd: cmd.into(), args: vec!["acp".into()], sha256, env: BTreeMap::new() };
        agent.distribution.binary = BTreeMap::from([(current_platform().unwrap().to_string(), entry)]);
        agent
    }

    #[test]
    fn the_registry_parses_and_skips_a_broken_entry() {
        let registry = parse_registry(REGISTRY).unwrap();
        assert_eq!(registry.version, "1.0.0");
        let ids: Vec<&str> = registry.agents.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["opencode", "claude-acp", "fast-agent", "vtcode"]);
        let opencode = &registry.agents[0];
        assert_eq!(opencode.icon.as_deref(), Some("https://cdn.agentclientprotocol.com/registry/v1/latest/opencode.svg"));
        assert_eq!(opencode.authors, ["Anomaly"]);
        let mac = &opencode.distribution.binary["darwin-aarch64"];
        assert_eq!((mac.cmd.as_str(), mac.args.as_slice()), ("./opencode", ["acp".to_string()].as_slice()));
        assert_eq!(mac.sha256.as_deref().map(str::len), Some(64));
        let claude = registry.agents[1].distribution.npx.as_ref().unwrap();
        assert_eq!((claude.package.as_str(), claude.args.len()), ("@agentclientprotocol/claude-agent-acp@0.100.0", 0));
        let fast = registry.agents[2].distribution.uvx.as_ref().unwrap();
        assert_eq!(fast.env["FAST_AGENT_MODEL"], "codexplan");
        assert_eq!(registry.agents[3].distribution.binary["darwin-aarch64"].sha256, None);
        assert!(matches!(parse_registry("{}"), Err(EnhanceError::InvalidOutput(_))));
    }

    #[test]
    fn launches_for_npx_uvx_and_an_installed_download() {
        let registry = parse_registry(REGISTRY).unwrap();
        let root = tempfile::tempdir().unwrap();
        if crate::detect::detect_command("npx").is_some() {
            let launch = resolve_launch(&registry.agents[1], root.path()).unwrap();
            assert_eq!(launch.command, "npx");
            assert_eq!(launch.args, ["-y", "@agentclientprotocol/claude-agent-acp@0.100.0"]);
        }
        if crate::detect::detect_command("uvx").is_some() {
            let launch = resolve_launch(&registry.agents[2], root.path()).unwrap();
            assert_eq!((launch.command.as_str(), launch.args.as_slice()), ("uvx", ["fast-agent-acp==0.10.1".to_string(), "-x".into()].as_slice()));
            assert_eq!(launch.env["FAST_AGENT_MODEL"], "codexplan");
        }
        let agent = agent_with_binary("https://example.test/a.tar.gz", "./bin/agent", None);
        assert!(matches!(resolve_launch(&agent, root.path()), Err(EnhanceError::NotConfigured(m)) if m.contains("not installed")));
        let program = root.path().join("opencode/2.2.5/bin/agent");
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, "").unwrap();
        let launch = resolve_launch(&agent, root.path()).unwrap();
        assert_eq!((launch.command, launch.args), (program.to_string_lossy().into_owned(), vec!["acp".to_string()]));
    }

    #[test]
    fn program_paths_stay_in_their_folder() {
        assert_eq!(relative_cmd("./opencode").unwrap(), Path::new("opencode"));
        assert_eq!(relative_cmd("./goose-package\\goose.exe").unwrap(), Path::new("goose-package/goose.exe"));
        assert_eq!(relative_cmd("amp-acp.exe").unwrap(), Path::new("amp-acp.exe"));
        assert!(relative_cmd("../outside").is_err());
        assert!(relative_cmd("/abs/agent").is_err());
        assert!(relative_cmd("./").is_err());
        assert_eq!(archive_name("https://a.test/x/agent-1.0.tar.gz?sig=1"), "agent-1.0.tar.gz");
    }

    fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (path, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn zip_file(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (path, data) in files {
            writer.start_file(*path, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn sha(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    /// A server that serves each body at its path.
    fn file_server(files: Vec<(&'static str, Vec<u8>)>) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let found = files.iter().find(|(path, _)| *path == request.url()).map(|(_, body)| body.clone());
                let _ = match found {
                    Some(body) => request.respond(tiny_http::Response::from_data(body)),
                    None => request.respond(tiny_http::Response::empty(404)),
                };
            }
        });
        base
    }

    #[test]
    fn a_download_is_checked_unpacked_and_made_executable() {
        let archive = tar_gz(&[("pkg/bin/agent", b"#!/bin/sh\necho hi\n"), ("pkg/README", b"read me")]);
        let zipped = zip_file(&[("agent.exe", b"MZ")]);
        let raw = b"#!/bin/sh\n".to_vec();
        let base = file_server(vec![("/a.tar.gz", archive.clone()), ("/a.zip", zipped.clone()), ("/agent-linux-amd64", raw.clone())]);
        let root = tempfile::tempdir().unwrap();
        let timeout = Duration::from_secs(10);

        let agent = agent_with_binary(&format!("{base}/a.tar.gz"), "./pkg/bin/agent", Some(sha(&archive).to_uppercase()));
        let program = install_binary(&agent, root.path(), timeout).unwrap();
        assert_eq!(program, root.path().join("opencode/2.2.5/pkg/bin/agent"));
        assert_eq!(std::fs::read_to_string(root.path().join("opencode/2.2.5/pkg/README")).unwrap(), "read me");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(std::fs::metadata(&program).unwrap().permissions().mode() & 0o111, 0, "the program is executable");
        }
        assert_eq!(installed_binary(&agent, root.path()), Some(program.clone()));
        // Installed already: no second download.
        let offline = agent_with_binary("http://127.0.0.1:9/a.tar.gz", "./pkg/bin/agent", None);
        assert_eq!(install_binary(&offline, root.path(), timeout).unwrap(), program);

        let other = tempfile::tempdir().unwrap();
        let zip_agent = agent_with_binary(&format!("{base}/a.zip"), "agent.exe", Some(sha(&zipped)));
        assert_eq!(std::fs::read(install_binary(&zip_agent, other.path(), timeout).unwrap()).unwrap(), b"MZ");
        let third = tempfile::tempdir().unwrap();
        let raw_agent = agent_with_binary(&format!("{base}/agent-linux-amd64"), "./agent-linux-amd64", None);
        assert_eq!(std::fs::read(install_binary(&raw_agent, third.path(), timeout).unwrap()).unwrap(), raw);
    }

    #[test]
    fn a_download_with_the_wrong_sha256_is_rejected() {
        let archive = tar_gz(&[("agent", b"x")]);
        let base = file_server(vec![("/a.tar.gz", archive)]);
        let root = tempfile::tempdir().unwrap();
        let agent = agent_with_binary(&format!("{base}/a.tar.gz"), "./agent", Some(sha(b"something else")));
        let error = install_binary(&agent, root.path(), Duration::from_secs(10)).unwrap_err();
        assert!(matches!(&error, EnhanceError::InvalidOutput(m) if m.contains("SHA-256")), "{error:?}");
        assert!(!root.path().join("opencode/2.2.5").exists(), "nothing is installed");
        assert_eq!(std::fs::read_dir(root.path().join("opencode")).unwrap().count(), 0, "the temporary folder is gone");
    }

    #[test]
    fn the_registry_comes_from_the_cache_when_the_download_fails() {
        let base = file_server(vec![("/registry.json", REGISTRY.as_bytes().to_vec())]);
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache/registry.json");
        let timeout = Duration::from_secs(10);
        assert!(fetch_registry_from("http://127.0.0.1:9/registry.json", &cache, timeout).is_err(), "no download and no cache");
        let online = fetch_registry_from(&format!("{base}/registry.json"), &cache, timeout).unwrap();
        assert_eq!(online.agents.len(), 4);
        assert!(cache.is_file(), "the download is saved");
        let offline = fetch_registry_from("http://127.0.0.1:9/registry.json", &cache, timeout).unwrap();
        assert_eq!(offline, online);
        assert_eq!(cached_registry(&cache), Some(online));
    }
}
