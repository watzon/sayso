//! The host of Sayso extensions: the server of the Host API
//! (`docs/extensions.md`, "Host API").
//!
//! An extension connects to a local socket (a named pipe on Windows) and
//! sends NDJSON messages of `sayso-protocol`. The host knows the extension by
//! its executable ([`Identify`]) and the record of `state.toml` ([`StateFile`]),
//! and checks the capabilities of each request.
//!
//! The host has no GPUI. It gets the engine, the capture, and the questions
//! to the user from `sayso-app` through [`HostServices`], so it runs and is
//! tested without a window.

mod connection;
mod identity;
mod state;
mod transport;

pub use identity::{Identify, Peer, SystemIdentify};
pub use state::{Record, State, StateFile};

use parking_lot::Mutex;
use sayso_core::paths::Paths;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

/// What the host gets from the app. Later sub-steps of step 6 add the model
/// list, the download question, jobs, live sessions, and `enhance`.
pub trait HostServices: Send + Sync {}

/// Where the host listens and what it reads.
pub struct HostConfig {
    /// The socket, or the name of the pipe on Windows.
    pub socket: PathBuf,
    /// `state.toml`.
    pub state_file: PathBuf,
    /// The version of Sayso, for the `hello` reply.
    pub host_version: String,
    pub identify: Arc<dyn Identify>,
}

impl HostConfig {
    pub fn new(socket: PathBuf, state_file: PathBuf) -> HostConfig {
        HostConfig { socket, state_file, host_version: env!("CARGO_PKG_VERSION").into(), identify: Arc::new(SystemIdentify) }
    }

    /// The socket of `sayso_protocol::socket`, and `state.toml` in the
    /// extensions folder.
    pub fn for_paths(paths: &Paths) -> io::Result<HostConfig> {
        Ok(HostConfig::new(sayso_protocol::socket::host_socket_path()?, paths.extensions_dir().join("state.toml")))
    }
}

/// The running host. [`Host::close`] (or drop) stops it.
pub struct Host {
    socket: PathBuf,
    shared: Arc<Shared>,
    listener: Arc<transport::Listener>,
    accept: Mutex<Option<JoinHandle<()>>>,
}

pub(crate) struct Shared {
    pub host_version: String,
    pub identify: Arc<dyn Identify>,
    pub state: StateFile,
    // Later sub-steps call it.
    pub _services: Arc<dyn HostServices>,
    pub closing: AtomicBool,
    /// The open connections by a number of the host.
    pub connections: Mutex<HashMap<u64, connection::Entry>>,
    next_connection: AtomicU64,
}

impl Host {
    /// Listen on the socket of `config`. Fails when the socket cannot be made,
    /// for example when another Sayso serves extensions.
    pub fn start(config: HostConfig, services: Arc<dyn HostServices>) -> io::Result<Host> {
        let listener = Arc::new(transport::Listener::bind(&config.socket)?);
        let shared = Arc::new(Shared {
            host_version: config.host_version,
            identify: config.identify,
            state: StateFile::new(config.state_file),
            _services: services,
            closing: AtomicBool::new(false),
            connections: Mutex::new(HashMap::new()),
            next_connection: AtomicU64::new(1),
        });
        let accept = {
            let shared = shared.clone();
            let listener = listener.clone();
            std::thread::Builder::new().name("sayso-host".into()).spawn(move || accept_loop(&shared, &listener))?
        };
        log::info!("host: extensions connect at {}", config.socket.display());
        Ok(Host { socket: config.socket, shared, listener, accept: Mutex::new(Some(accept)) })
    }

    /// The ids of the extensions that are connected now, in order.
    pub fn connected_extensions(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.shared.connections.lock().values().filter_map(|entry| entry.extension_id.clone()).collect();
        ids.sort();
        ids
    }

    /// Send `host_closing` to each connection, close them, and remove the
    /// socket. A second call does nothing.
    pub fn close(&self) {
        let writers = {
            let mut connections = self.shared.connections.lock();
            if self.shared.closing.swap(true, Ordering::SeqCst) {
                return;
            }
            connections.values_mut().filter_map(connection::Entry::close_with_host_closing).collect::<Vec<_>>()
        };
        if self.listener.wake()
            && let Some(accept) = self.accept.lock().take()
        {
            let _ = accept.join();
        }
        // Each writer stops after its last message, or after its timeout.
        for writer in writers {
            let _ = writer.join();
        }
        self.listener.remove();
        log::info!("host: closed");
    }

    /// The socket, or the name of the pipe on Windows.
    pub fn socket(&self) -> &Path {
        &self.socket
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.close();
    }
}

fn accept_loop(shared: &Arc<Shared>, listener: &transport::Listener) {
    loop {
        let accepted = listener.accept();
        if shared.closing.load(Ordering::SeqCst) {
            return;
        }
        match accepted {
            Ok(conn) => {
                let number = shared.next_connection.fetch_add(1, Ordering::SeqCst);
                let shared = shared.clone();
                let spawned = std::thread::Builder::new().name("sayso-host-read".into()).spawn(move || connection::serve(&shared, number, conn));
                if let Err(e) = spawned {
                    log::warn!("host: cannot start a connection thread: {e}");
                }
            }
            Err(e) => {
                log::warn!("host: accept failed: {e}");
                // Do not spin when the listener is broken.
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    }
}
