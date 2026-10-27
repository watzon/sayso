//! The host of extensions (`sayso-host`, `docs/extensions.md`). It starts
//! after the services and closes when Sayso quits.

use sayso_core::paths::Paths;
use std::sync::Arc;

/// What the host gets from the app. Later sub-steps of step 6 add the
/// engine, the capture, and the questions to the user.
struct AppHostServices;

impl sayso_host::HostServices for AppHostServices {}

/// Start the host. When that fails, Sayso runs without extensions.
pub fn start(paths: &Paths) -> Option<sayso_host::Host> {
    let started = sayso_host::HostConfig::for_paths(paths).and_then(|mut config| {
        config.host_version = env!("CARGO_PKG_VERSION").into();
        sayso_host::Host::start(config, Arc::new(AppHostServices))
    });
    started.map_err(|e| log::warn!("extensions cannot connect: {e}")).ok()
}
