//! One request for the update manifest.
//!
//! The request has no identifier of the user or the computer. The server
//! sees the IP address and the `User-Agent` header, which names the version
//! and the system.

use crate::manifest::MAX_BYTES;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CheckError {
    /// The newest release has no manifest. This is not "up to date": Sayso
    /// does not know which version is the newest.
    #[error("the newest release has no update information")]
    NotFound,
    #[error("the server answered with HTTP {0}")]
    Status(u16),
    #[error("{0}")]
    Network(String),
}

/// The `User-Agent` of an update check: `Sayso/0.3.0 (macos; aarch64)`.
pub fn user_agent() -> String {
    format!("Sayso/{} ({}; {})", crate::VERSION, std::env::consts::OS, std::env::consts::ARCH)
}

/// Get the bytes of the manifest at `url`. Blocks, for 30 seconds at most.
///
/// `https_only` is false only in tests, which use a local server. With it,
/// a redirect to a plain `http` address is an error.
pub fn fetch(url: &str, https_only: bool) -> Result<Vec<u8>, CheckError> {
    let config = ureq::Agent::config_builder()
        .user_agent(user_agent())
        .https_only(https_only)
        .max_redirects(5)
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut response = agent.get(url).call().map_err(|e| CheckError::Network(e.to_string()))?;
    match response.status().as_u16() {
        200 => {}
        404 => return Err(CheckError::NotFound),
        status => return Err(CheckError::Status(status)),
    }
    // One byte more than the limit, so `verify` can tell a file that is too large.
    response
        .body_mut()
        .with_config()
        .limit(MAX_BYTES as u64 + 1)
        .read_to_vec()
        .map_err(|e| CheckError::Network(e.to_string()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A local server that answers each request with the next response and
    /// repeats the last one. It records the `User-Agent` of each request.
    pub struct Server {
        pub url: String,
        pub requests: Arc<AtomicUsize>,
        pub agents: Arc<std::sync::Mutex<Vec<String>>>,
    }

    pub fn serve(responses: Vec<(u16, Vec<u8>)>) -> Server {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}/latest.json", server.server_addr());
        let requests = Arc::new(AtomicUsize::new(0));
        let agents = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (count, seen) = (requests.clone(), agents.clone());
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let n = count.fetch_add(1, Ordering::SeqCst);
                let agent = request.headers().iter().find(|h| h.field.equiv("User-Agent")).map(|h| h.value.to_string());
                seen.lock().unwrap().push(agent.unwrap_or_default());
                let (status, body) = responses[n.min(responses.len() - 1)].clone();
                let _ = request.respond(tiny_http::Response::from_data(body).with_status_code(status));
            }
        });
        Server { url, requests, agents }
    }

    #[test]
    fn a_good_answer_gives_the_bytes_and_names_the_version() {
        let server = serve(vec![(200, b"{}".to_vec())]);
        assert_eq!(fetch(&server.url, false), Ok(b"{}".to_vec()));
        let agent = server.agents.lock().unwrap()[0].clone();
        assert!(agent.starts_with(&format!("Sayso/{} (", crate::VERSION)), "{agent}");
    }

    #[test]
    fn a_missing_manifest_is_an_error() {
        assert_eq!(fetch(&serve(vec![(404, vec![])]).url, false), Err(CheckError::NotFound));
        assert_eq!(fetch(&serve(vec![(503, vec![])]).url, false), Err(CheckError::Status(503)));
    }

    #[test]
    fn a_large_answer_stops_at_the_limit() {
        let server = serve(vec![(200, vec![b' '; MAX_BYTES * 4])]);
        // Either the read stops with an error, or it returns one byte too many for `verify`.
        match fetch(&server.url, false) {
            Ok(bytes) => assert_eq!(bytes.len(), MAX_BYTES + 1),
            Err(e) => assert!(matches!(e, CheckError::Network(_))),
        }
    }

    #[test]
    fn plain_http_is_refused_outside_tests() {
        let server = serve(vec![(200, b"{}".to_vec())]);
        assert!(matches!(fetch(&server.url, true), Err(CheckError::Network(_))));
        assert_eq!(server.requests.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn no_server_is_a_network_error() {
        assert!(matches!(fetch("http://127.0.0.1:9/latest.json", false), Err(CheckError::Network(_))));
    }
}
