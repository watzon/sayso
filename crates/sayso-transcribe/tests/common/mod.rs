//! A mock HTTP server that records every request, with a parser for multipart bodies.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One part of a multipart body.
#[derive(Debug, Clone)]
pub struct Part {
    pub name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub data: Vec<u8>,
}

impl Part {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }
}

/// A request the mock server received.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    /// The path with the query.
    pub path: String,
    /// Header names in lower case.
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// The query items of the path, split at `&`.
    pub fn query(&self) -> Vec<&str> {
        self.path.split_once('?').map(|(_, q)| q.split('&').collect()).unwrap_or_default()
    }

    /// The path without the query.
    pub fn route(&self) -> &str {
        self.path.split('?').next().unwrap_or("")
    }

    /// The parts of a multipart body, in order.
    pub fn parts(&self) -> Vec<Part> {
        let content_type = self.header("content-type").unwrap_or("");
        let boundary = content_type.split("boundary=").nth(1).expect("a multipart content type");
        let delimiter = format!("--{boundary}").into_bytes();
        let mut parts = Vec::new();
        let mut rest = &self.body[..];
        let first = find(rest, &delimiter).expect("a first delimiter");
        rest = &rest[first + delimiter.len()..];
        // Each part ends at the next delimiter. The last one is followed by "--".
        while !rest.starts_with(b"--") {
            let end = find(rest, &delimiter).expect("a closing delimiter");
            let part = &rest[..end];
            rest = &rest[end + delimiter.len()..];
            let part = part.strip_prefix(b"\r\n").expect("a line break after the delimiter");
            let part = part.strip_suffix(b"\r\n").expect("a line break before the delimiter");
            let split = find(part, b"\r\n\r\n").expect("a blank line after the headers");
            let head = String::from_utf8_lossy(&part[..split]).into_owned();
            let mut part_headers = HashMap::new();
            for line in head.split("\r\n") {
                let (name, value) = line.split_once(": ").expect("a header line");
                part_headers.insert(name.to_ascii_lowercase(), value.to_string());
            }
            let disposition = &part_headers["content-disposition"];
            parts.push(Part {
                name: quoted_value(disposition, "name").expect("a name"),
                filename: quoted_value(disposition, "filename"),
                content_type: part_headers.get("content-type").cloned(),
                data: part[split + 4..].to_vec(),
            });
        }
        parts
    }

    /// The text of the first part with this name.
    pub fn field(&self, name: &str) -> Option<String> {
        self.parts().into_iter().find(|p| p.name == name).map(|p| p.text())
    }

    /// The text of every part with this name.
    pub fn fields(&self, name: &str) -> Vec<String> {
        self.parts().into_iter().filter(|p| p.name == name).map(|p| p.text()).collect()
    }

    /// The names of all parts, in order.
    pub fn names(&self) -> Vec<String> {
        self.parts().into_iter().map(|p| p.name).collect()
    }

    pub fn file(&self, name: &str) -> Part {
        self.parts().into_iter().find(|p| p.name == name).expect("the file part")
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// `name="x"` from a header value. The leading space keeps `name` apart from `filename`.
fn quoted_value(header: &str, key: &str) -> Option<String> {
    let marker = format!(" {key}=\"");
    let start = header.find(&marker)? + marker.len();
    let end = header[start..].find('"')?;
    Some(header[start..start + end].to_string())
}

/// What the mock server answers.
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub delay: Duration,
}

impl Reply {
    pub fn json(status: u16, body: serde_json::Value) -> Self {
        Reply { status, body: body.to_string(), delay: Duration::ZERO }
    }
    pub fn ok(body: serde_json::Value) -> Self {
        Self::json(200, body)
    }
    pub fn raw(status: u16, body: &str) -> Self {
        Reply { status, body: body.into(), delay: Duration::ZERO }
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
    requests: Arc<Mutex<Vec<Recorded>>>,
    server: Arc<tiny_http::Server>,
}

impl MockServer {
    pub fn start(handler: impl Fn(usize, &Recorded) -> Reply + Send + Sync + 'static) -> Self {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind"));
        let port = server.server_addr().to_ip().expect("ip address").port();
        let requests: Arc<Mutex<Vec<Recorded>>> = Arc::default();
        let handler = Arc::new(handler);
        let (srv, recorded) = (server.clone(), requests.clone());
        std::thread::spawn(move || {
            for mut request in srv.incoming_requests() {
                let mut body = Vec::new();
                let _ = request.as_reader().read_to_end(&mut body);
                let headers = request
                    .headers()
                    .iter()
                    .map(|h| (h.field.as_str().as_str().to_ascii_lowercase(), h.value.as_str().to_string()))
                    .collect();
                let recorded_request =
                    Recorded { method: request.method().to_string(), path: request.url().to_string(), headers, body };
                let index = {
                    let mut all = recorded.lock().unwrap();
                    all.push(recorded_request.clone());
                    all.len() - 1
                };
                let handler = handler.clone();
                // One thread per request, so a slow reply does not block the next one.
                std::thread::spawn(move || {
                    let reply = handler(index, &recorded_request);
                    std::thread::sleep(reply.delay);
                    let header = tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap();
                    let response = tiny_http::Response::from_string(reply.body).with_status_code(reply.status).with_header(header);
                    let _ = request.respond(response);
                });
            }
        });
        MockServer { base_url: format!("http://127.0.0.1:{port}/v1"), requests, server }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.server.unblock();
    }
}
