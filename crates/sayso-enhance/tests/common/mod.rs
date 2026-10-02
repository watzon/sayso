//! Test helpers: a mock HTTP server and fake command line tools.
#![allow(dead_code)]

use serde_json::{Value, json};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A request the mock server received.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    /// Header names in lower case.
    pub headers: HashMap<String, String>,
    pub body: Value,
}

impl Recorded {
    /// The `response_format.type` of the body, or "" when there is none.
    pub fn format_type(&self) -> &str {
        self.body["response_format"]["type"].as_str().unwrap_or("")
    }

    pub fn has_tools(&self) -> bool {
        self.body.get("tools").is_some()
    }
}

/// What the mock server answers.
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub delay: Duration,
}

impl Reply {
    pub fn ok(body: Value) -> Self {
        Reply {
            status: 200,
            body: body.to_string(),
            delay: Duration::ZERO,
        }
    }
    pub fn status(status: u16, body: &str) -> Self {
        Reply {
            status,
            body: body.into(),
            delay: Duration::ZERO,
        }
    }
    /// A chat completion whose content is `{"text": text}`.
    pub fn text(text: &str) -> Self {
        Self::content(&json!({ "text": text }).to_string())
    }
    /// A chat completion with this exact content string.
    pub fn content(content: &str) -> Self {
        Self::ok(
            json!({ "choices": [{ "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }] }),
        )
    }
    /// A chat completion that calls `return_text`.
    pub fn tool(text: &str) -> Self {
        let args = json!({ "text": text }).to_string();
        Self::ok(json!({ "choices": [{
            "message": { "role": "assistant", "content": null,
                "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "return_text", "arguments": args } }] },
            "finish_reason": "tool_calls" }] }))
    }
    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

/// A local HTTP server. The handler gets the request number (from 0) and the request.
pub struct MockServer {
    /// `http://127.0.0.1:<port>/v1`
    pub base_url: String,
    /// `http://127.0.0.1:<port>`
    pub root_url: String,
    requests: Arc<parking_lot::Mutex<Vec<Recorded>>>,
    server: Arc<tiny_http::Server>,
}

impl MockServer {
    pub fn start(handler: impl Fn(usize, &Recorded) -> Reply + Send + Sync + 'static) -> Self {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind"));
        let port = server.server_addr().to_ip().expect("ip address").port();
        let requests: Arc<parking_lot::Mutex<Vec<Recorded>>> = Arc::default();
        let handler = Arc::new(handler);
        let (srv, recorded) = (server.clone(), requests.clone());
        std::thread::spawn(move || {
            for mut request in srv.incoming_requests() {
                let mut text = String::new();
                let _ = request.as_reader().read_to_string(&mut text);
                let headers = request
                    .headers()
                    .iter()
                    .map(|h| {
                        (
                            h.field.as_str().as_str().to_ascii_lowercase(),
                            h.value.as_str().to_string(),
                        )
                    })
                    .collect();
                let recorded_request = Recorded {
                    method: request.method().to_string(),
                    path: request.url().to_string(),
                    headers,
                    body: serde_json::from_str(&text).unwrap_or(Value::Null),
                };
                let index = {
                    let mut all = recorded.lock();
                    all.push(recorded_request.clone());
                    all.len() - 1
                };
                let handler = handler.clone();
                // One thread per request, so a slow reply does not block the next one.
                std::thread::spawn(move || {
                    let reply = handler(index, &recorded_request);
                    std::thread::sleep(reply.delay);
                    let header =
                        tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap();
                    let response = tiny_http::Response::from_string(reply.body)
                        .with_status_code(reply.status)
                        .with_header(header);
                    let _ = request.respond(response);
                });
            }
        });
        MockServer {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            root_url: format!("http://127.0.0.1:{port}"),
            requests,
            server,
        }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.server.unblock();
    }
}

/// Write an executable shell script into `dir`.
pub fn fake_tool(dir: &Path, name: &str, script: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}
