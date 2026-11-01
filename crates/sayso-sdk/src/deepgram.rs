//! Deepgram for the final pass (`docs/sdk.md`): a small client of its HTTP API,
//! and a layer around the engine.
//!
//! Sayso uses cloud models for the final role only. So the audio of a whole
//! part goes to Deepgram when the part ends, and the live preview and the
//! voice detection stay on this computer. The key comes from the user. This
//! crate does not store it, and it never writes it in a log.

use sayso_speech::{EngineError, EngineEvent, ModelId, SpeechEngine, SpeechPart, Turn};
use crossbeam_channel::Receiver;
use sayso_protocol::{ErrorCode, Segment};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// The model id of the final pass with Deepgram.
pub const MODEL: &str = "deepgram-nova-3";
const API_MODEL: &str = "nova-3";
const BASE_URL: &str = "https://api.deepgram.com/v1";
/// The whole request. A part is a few seconds of speech.
const TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The `code` of an [`EngineError`] when Deepgram cannot be reached.
pub const CODE_CONNECTION: &str = "cloud_connection";
/// The `code` of an [`EngineError`] when Deepgram refuses the key.
pub const CODE_KEY: &str = "cloud_key";

/// The HTTP client. It keeps the key in memory only.
#[derive(Clone)]
pub struct Deepgram {
    key: String,
    base_url: String,
    agent: ureq::Agent,
}

impl std::fmt::Debug for Deepgram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deepgram")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl Deepgram {
    pub fn new(key: impl Into<String>) -> Deepgram {
        Deepgram::with_base_url(key, BASE_URL)
    }

    /// A client of another server, for tests.
    pub fn with_base_url(key: impl Into<String>, base_url: impl Into<String>) -> Deepgram {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .http_status_as_error(false)
            .build();
        Deepgram {
            key: key.into(),
            base_url: base_url.into(),
            agent: config.into(),
        }
    }

    /// Ask Deepgram if it knows the key.
    pub fn check_key(&self) -> Result<(), EngineError> {
        let url = format!("{}/projects", self.base_url);
        let reply = self
            .agent
            .get(&url)
            .header("Authorization", self.auth())
            .call()
            .map_err(connection_error)?;
        let status = reply.status().as_u16();
        check_status(
            status,
            &mut reply.into_body().read_to_string().unwrap_or_default(),
        )
    }

    /// The text of `samples` (16 kHz mono). `language` is a code such as `en`, or `auto`.
    pub fn transcribe(&self, samples: &[f32], language: &str) -> Result<String, EngineError> {
        let wav = sayso_engine_sdk::wav::encode_i16(&sayso_engine_sdk::wav::f32_to_i16(samples));
        let reply = self
            .agent
            .post(&self.listen_url(language))
            .header("Authorization", self.auth())
            .header("Content-Type", "audio/wav")
            .send(&wav[..])
            .map_err(connection_error)?;
        let status = reply.status().as_u16();
        let mut body = reply
            .into_body()
            .read_to_string()
            .map_err(connection_error)?;
        check_status(status, &mut body)?;
        let json: Value = serde_json::from_str(&body).map_err(|e| {
            EngineError::new(format!("Deepgram sent a reply that is not JSON: {e}"))
        })?;
        json.pointer("/results/channels/0/alternatives/0/transcript")
            .and_then(Value::as_str)
            .map(|text| text.trim().to_string())
            .ok_or_else(|| EngineError::new("Deepgram sent a reply with no transcript"))
    }

    fn auth(&self) -> String {
        format!("Token {}", self.key)
    }

    fn listen_url(&self, language: &str) -> String {
        let mut url = format!(
            "{}/listen?model={API_MODEL}&smart_format=true&punctuate=true",
            self.base_url
        );
        if language.is_empty() || language == "auto" {
            url.push_str("&detect_language=true");
        } else {
            url.push_str("&language=");
            url.extend(
                language
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-'),
            );
        }
        url
    }
}

fn connection_error(error: impl std::fmt::Display) -> EngineError {
    EngineError::with_code(
        ErrorCode::parse(CODE_CONNECTION),
        format!("cannot reach Deepgram: {error}"),
    )
}

fn check_status(status: u16, body: &mut String) -> Result<(), EngineError> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(EngineError::with_code(
            ErrorCode::parse(CODE_KEY),
            format!("Deepgram refused the key (HTTP {status})"),
        )),
        // A server that fails or says "too many" is as good as no connection: a retry can work.
        408 | 429 | 500..=599 => Err(EngineError::with_code(
            ErrorCode::parse(CODE_CONNECTION),
            format!("Deepgram answered HTTP {status}"),
        )),
        _ => {
            body.truncate(body.floor_char_boundary(200));
            Err(EngineError::new(format!(
                "Deepgram answered HTTP {status}: {body}"
            )))
        }
    }
}

/// A [`SpeechEngine`] that sends the final pass of [`MODEL`] to Deepgram, and
/// everything else to the engine under it.
pub struct DeepgramFinal {
    local: Arc<dyn SpeechEngine>,
    client: Deepgram,
}

impl DeepgramFinal {
    pub fn new(local: Arc<dyn SpeechEngine>, client: Deepgram) -> DeepgramFinal {
        DeepgramFinal { local, client }
    }
}

impl SpeechEngine for DeepgramFinal {
    fn load(&self, model: &ModelId) -> Result<(), EngineError> {
        if model.as_str() == MODEL {
            Ok(())
        } else {
            self.local.load(model)
        }
    }

    fn transcribe(
        &self,
        samples: &[f32],
        model: &ModelId,
        language: &str,
    ) -> Result<String, EngineError> {
        if model.as_str() == MODEL {
            self.client.transcribe(samples, language)
        } else {
            self.local.transcribe(samples, model, language)
        }
    }

    fn transcribe_segments(
        &self,
        samples: &[f32],
        model: &ModelId,
        language: &str,
    ) -> Result<Vec<Segment>, EngineError> {
        if model.as_str() != MODEL {
            return self.local.transcribe_segments(samples, model, language);
        }
        let text = self.client.transcribe(samples, language)?;
        let end_ms = samples.len() as u64 * 1000 / u64::from(sayso_protocol::pcm::SAMPLE_RATE);
        Ok(vec![Segment {
            start_ms: 0,
            end_ms,
            text,
            words: None,
        }])
    }

    fn detect_speech(
        &self,
        model: &ModelId,
        samples: &[f32],
    ) -> Result<Vec<SpeechPart>, EngineError> {
        self.local.detect_speech(model, samples)
    }

    fn diarize(
        &self,
        model: &ModelId,
        samples: &[f32],
        speakers: Option<u32>,
    ) -> Result<Vec<Turn>, EngineError> {
        self.local.diarize(model, samples, speakers)
    }

    fn start_stream(
        &self,
        session: u64,
        model: &ModelId,
        language: &str,
    ) -> Result<(), EngineError> {
        self.local.start_stream(session, model, language)
    }

    fn start_vad(&self, session: u64, model: &ModelId) -> Result<(), EngineError> {
        self.local.start_vad(session, model)
    }

    fn push_audio(&self, session: u64, samples: &[f32]) -> Result<(), EngineError> {
        self.local.push_audio(session, samples)
    }

    fn end_stream(&self, session: u64) {
        self.local.end_stream(session);
    }

    fn end_vad(&self, session: u64) -> Result<(), EngineError> {
        self.local.end_vad(session)
    }

    fn events(&self) -> Receiver<EngineEvent> {
        self.local.events()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The address, the auth header, and the size of the body of a request.
    type Seen = (String, String, usize);

    /// A server that gives each reply in turn and records each request.
    fn server(replies: Vec<(u16, &'static str)>) -> (String, std::thread::JoinHandle<Vec<Seen>>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", server.server_addr().to_ip().unwrap());
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for (status, body) in replies {
                let mut request = server.recv().unwrap();
                let auth = request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .map(|h| h.value.to_string())
                    .unwrap_or_default();
                let mut bytes = Vec::new();
                request.as_reader().read_to_end(&mut bytes).unwrap();
                seen.push((request.url().to_string(), auth, bytes.len()));
                request
                    .respond(tiny_http::Response::from_string(body).with_status_code(status))
                    .unwrap();
            }
            seen
        });
        (url, handle)
    }

    const OK: &str =
        r#"{"results":{"channels":[{"alternatives":[{"transcript":" Then push the fix. "}]}]}}"#;

    #[test]
    fn it_sends_the_audio_and_reads_the_transcript() {
        let (url, server) = server(vec![(200, OK), (200, OK)]);
        let client = Deepgram::with_base_url("secret", url);
        assert_eq!(
            client.transcribe(&[0.0; 1600], "de").unwrap(),
            "Then push the fix."
        );
        assert_eq!(
            client.transcribe(&[0.0; 1600], "auto").unwrap(),
            "Then push the fix."
        );
        let seen = server.join().unwrap();
        assert!(
            seen[0].0.contains("model=nova-3") && seen[0].0.contains("language=de"),
            "{}",
            seen[0].0
        );
        assert_eq!(seen[0].1, "Token secret");
        assert!(seen[0].2 > 3200, "a WAV file with a header");
        assert!(seen[1].0.contains("detect_language=true"));
    }

    #[test]
    fn a_failure_says_what_kind_it_is() {
        let (url, server) = server(vec![
            (401, "{}"),
            (429, "{}"),
            (503, "{}"),
            (400, "bad"),
            (200, "{}"),
        ]);
        let client = Deepgram::with_base_url("secret", url);
        let code = |e: EngineError| e.code.map(|c| c.as_str().to_string());
        assert_eq!(
            code(client.transcribe(&[0.0; 160], "en").unwrap_err()).as_deref(),
            Some(CODE_KEY)
        );
        assert_eq!(
            code(client.transcribe(&[0.0; 160], "en").unwrap_err()).as_deref(),
            Some(CODE_CONNECTION)
        );
        assert_eq!(
            code(client.transcribe(&[0.0; 160], "en").unwrap_err()).as_deref(),
            Some(CODE_CONNECTION)
        );
        assert_eq!(
            code(client.transcribe(&[0.0; 160], "en").unwrap_err()),
            None
        );
        assert_eq!(
            code(client.transcribe(&[0.0; 160], "en").unwrap_err()),
            None,
            "no transcript"
        );
        server.join().unwrap();
    }

    #[test]
    fn no_server_is_a_lost_connection() {
        let client = Deepgram::with_base_url("secret", "http://127.0.0.1:9/v1");
        let error = client.transcribe(&[0.0; 160], "en").unwrap_err();
        assert_eq!(
            error.code.as_ref().map(ErrorCode::as_str),
            Some(CODE_CONNECTION)
        );
    }

    #[test]
    fn the_key_does_not_show_in_debug_text() {
        assert!(!format!("{:?}", Deepgram::new("secret")).contains("secret"));
    }
}
