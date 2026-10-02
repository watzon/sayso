//! Shared HTTP and error helpers for the adapters.
//!
//! Every adapter sends its request through a [`Target`] and reads the answer
//! as a [`Reply`]. The one [`Deadline`] of a call covers all its requests,
//! retries included.

use sayso_core::config::is_local_url;
use sayso_core::speech::{SpeechError, SpeechProvider, SpeechProviderKind, SpeechRequest};
use serde_json::Value;
use std::time::{Duration, Instant};

/// The deadline for a call that has a zero timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest part of an error body that goes into an error message.
const BODY_PREVIEW_CHARS: usize = 200;

/// One whole-call deadline.
pub(crate) struct Deadline {
    started: Instant,
    timeout: Duration,
}

impl Deadline {
    /// A zero timeout means 30 s.
    pub(crate) fn new(timeout: Duration) -> Self {
        let timeout = if timeout.is_zero() { DEFAULT_TIMEOUT } else { timeout };
        Self { started: Instant::now(), timeout }
    }

    pub(crate) fn timeout_ms(&self) -> u64 {
        self.timeout.as_millis() as u64
    }

    pub(crate) fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// What is left of the deadline. `Timeout` when nothing is left.
    pub(crate) fn remaining(&self) -> Result<Duration, SpeechError> {
        let left = self.timeout.saturating_sub(self.started.elapsed());
        if left.is_zero() { Err(SpeechError::Timeout(self.timeout_ms())) } else { Ok(left) }
    }
}

/// Where one call goes: the provider, its address, and its key.
pub(crate) struct Target<'a> {
    pub(crate) id: &'a str,
    pub(crate) kind: SpeechProviderKind,
    pub(crate) model: &'a str,
    pub(crate) base: String,
    /// Trimmed, never empty. None for a local server without a key.
    pub(crate) key: Option<&'a str>,
    agent: ureq::Agent,
}

impl<'a> Target<'a> {
    /// Check the setup before any request. Fails with `NotConfigured` when the
    /// address is missing, or when a cloud provider has no key.
    pub(crate) fn new(provider: &'a SpeechProvider, api_key: Option<&'a str>, model: &'a str) -> Result<Self, SpeechError> {
        let base = provider.base_url().ok_or_else(|| SpeechError::NotConfigured("no address is set".into()))?;
        let key = api_key.map(str::trim).filter(|k| !k.is_empty());
        if key.is_none() && provider.is_cloud() {
            return Err(SpeechError::NotConfigured("no API key is saved".into()));
        }
        Ok(Self { id: &provider.id, kind: provider.kind, model, agent: agent(&base), base, key })
    }

    /// `path` starts with a slash.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub(crate) fn get(&self, url: &str, headers: &[(&str, String)], deadline: &Deadline) -> Result<Reply, SpeechError> {
        let started = Instant::now();
        let mut http = self.agent.get(url).config().timeout_global(Some(deadline.remaining()?)).http_status_as_error(false).build();
        for (name, value) in headers {
            http = http.header(*name, value.as_str());
        }
        self.read(http.call(), started, deadline)
    }

    pub(crate) fn post(
        &self,
        url: &str,
        headers: &[(&str, String)],
        content_type: &str,
        body: &[u8],
        deadline: &Deadline,
    ) -> Result<Reply, SpeechError> {
        let started = Instant::now();
        let mut http = self.agent.post(url).config().timeout_global(Some(deadline.remaining()?)).http_status_as_error(false).build();
        for (name, value) in headers {
            http = http.header(*name, value.as_str());
        }
        http = http.header("Content-Type", content_type);
        self.read(http.send(body), started, deadline)
    }

    fn read(
        &self,
        response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
        started: Instant,
        deadline: &Deadline,
    ) -> Result<Reply, SpeechError> {
        let mut response = response.map_err(|e| map_ureq_error(e, deadline.timeout_ms()))?;
        let status = response.status().as_u16();
        let body = response.body_mut().read_to_string().map_err(|e| map_ureq_error(e, deadline.timeout_ms()))?;
        // Never log the key or the audio.
        log::debug!("{}: model {:?} answered HTTP {status} in {} ms", self.id, self.model, started.elapsed().as_millis());
        Ok(Reply { status, body })
    }
}

/// An HTTP agent. Local servers bypass any system proxy.
fn agent(base_url: &str) -> ureq::Agent {
    let mut config = ureq::Agent::config_builder().user_agent("Sayso");
    if is_local_url(base_url) {
        config = config.proxy(None);
    }
    config.build().into()
}

/// Turn a transport error into a [`SpeechError`]. `timeout_ms` is the deadline that was set.
pub(crate) fn map_ureq_error(error: ureq::Error, timeout_ms: u64) -> SpeechError {
    match error {
        ureq::Error::Timeout(_) => SpeechError::Timeout(timeout_ms),
        ureq::Error::Io(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {
            SpeechError::Timeout(timeout_ms)
        }
        other => SpeechError::Network(other.to_string()),
    }
}

/// An answer with any status code.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: String,
}

impl Reply {
    pub(crate) fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub(crate) fn json(&self) -> Option<Value> {
        serde_json::from_str(&self.body).ok()
    }

    /// An `Http` error. `message` reads the provider's error text out of the
    /// JSON body. Without it, the message is the start of the body.
    pub(crate) fn error(&self, message: fn(&Value) -> Option<String>) -> SpeechError {
        let message = self
            .json()
            .as_ref()
            .and_then(message)
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| self.body.trim().chars().take(BODY_PREVIEW_CHARS).collect());
        SpeechError::Http { status: self.status, message }
    }

    /// The JSON body of a 2xx answer. A non-2xx answer is an `Http` error.
    pub(crate) fn into_json(self, message: fn(&Value) -> Option<String>) -> Result<Value, SpeechError> {
        if !self.is_success() {
            return Err(self.error(message));
        }
        serde_json::from_str(&self.body).map_err(|e| SpeechError::InvalidResponse(format!("the body is not JSON: {e}")))
    }
}

/// The string at a JSON pointer, or `InvalidResponse`. An empty string is a valid
/// transcript: the recording had no speech.
pub(crate) fn text_at(json: &Value, pointer: &str) -> Result<String, SpeechError> {
    json.pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| SpeechError::InvalidResponse(format!("the body has no string at {pointer}")))
}

/// The language hint, if there is one.
pub(crate) fn language<'a>(request: &SpeechRequest<'a>) -> Option<&'a str> {
    request.language.map(str::trim).filter(|l| !l.is_empty())
}

/// The dictionary words, trimmed, without empty ones.
pub(crate) fn words<'a>(request: &'a SpeechRequest) -> Vec<&'a str> {
    request.vocabulary.iter().map(|w| w.trim()).filter(|w| !w.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_timeout_means_thirty_seconds() {
        assert_eq!(Deadline::new(Duration::ZERO).timeout_ms(), 30_000);
        assert_eq!(Deadline::new(Duration::from_millis(250)).timeout_ms(), 250);
    }

    #[test]
    fn spent_deadline_is_a_timeout() {
        let deadline = Deadline::new(Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(deadline.remaining().unwrap_err(), SpeechError::Timeout(1));
    }

    #[test]
    fn error_message_falls_back_to_the_start_of_the_body() {
        let reply = Reply { status: 500, body: "x".repeat(500) };
        assert!(matches!(reply.error(|_| None), SpeechError::Http { status: 500, message } if message.len() == 200));
        let reply = Reply { status: 401, body: r#"{"m":" bad key "}"#.into() };
        let error = reply.error(|v| v["m"].as_str().map(str::to_string));
        assert_eq!(error, SpeechError::Http { status: 401, message: "bad key".into() });
    }
}
