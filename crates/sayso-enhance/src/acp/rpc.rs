//! JSON-RPC 2.0 with one JSON message per line, over any reader and writer
//! pair. A helper thread reads the messages: it routes the responses to the
//! callers that wait for them, the notifications to the subscriber of their
//! session, and it answers the requests of the other side with a handler.

use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

/// The error object of a JSON-RPC response.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    fn from_value(value: &Value) -> Self {
        Self {
            code: value["code"].as_i64().unwrap_or(0),
            message: value["message"].as_str().unwrap_or("no message").to_string(),
            data: value.get("data").filter(|d| !d.is_null()).cloned(),
        }
    }

    /// The message, with the data when it has some.
    pub fn detail(&self) -> String {
        match &self.data {
            Some(Value::String(text)) => format!("{}: {text}", self.message),
            Some(data) => format!("{}: {data}", self.message),
            None => self.message.clone(),
        }
    }
}

/// Why a call did not give a result.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CallError {
    /// The other side answered with an error.
    Rpc(RpcError),
    /// No answer came before the deadline.
    Timeout,
    /// The caller stopped the wait.
    Cancelled,
    /// The connection is closed. The text says why.
    Closed(String),
}

/// A notification of the other side.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Notification {
    pub method: String,
    pub params: Value,
}

/// Answers a request of the other side: its method and its params.
pub(crate) type Handler = Box<dyn Fn(&str, &Value) -> Result<Value, RpcError> + Send + Sync>;

type Reply = Result<Value, RpcError>;

/// State that the connection and its reader thread share.
struct Shared {
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    pending: Mutex<HashMap<u64, Sender<Reply>>>,
    /// The subscriber of each session, by `params.sessionId`.
    subscribers: Mutex<HashMap<String, Sender<Notification>>>,
    /// Set when the connection closed, with the reason.
    closed: Mutex<Option<String>>,
}

impl Shared {
    fn write(&self, message: &Value) -> Result<(), CallError> {
        let mut writer = self.writer.lock();
        let Some(pipe) = writer.as_mut() else { return Err(CallError::Closed(self.reason())) };
        let mut line = message.to_string();
        line.push('\n');
        if let Err(e) = pipe.write_all(line.as_bytes()).and_then(|()| pipe.flush()) {
            *writer = None;
            drop(writer);
            self.close(format!("could not write to the agent: {e}"));
            return Err(CallError::Closed(self.reason()));
        }
        Ok(())
    }

    /// Mark the connection closed. Every caller that waits gets `Closed`.
    fn close(&self, reason: String) {
        self.closed.lock().get_or_insert(reason);
        self.pending.lock().clear();
        self.subscribers.lock().clear();
    }

    fn reason(&self) -> String {
        self.closed.lock().clone().unwrap_or_else(|| "the connection is closed".into())
    }
}

/// One JSON-RPC connection. `Send + Sync`: several threads can call at once.
pub(crate) struct Connection {
    shared: Arc<Shared>,
    next_id: AtomicU64,
}

impl Connection {
    /// Start the reader thread. `handler` answers the requests of the other side.
    pub fn new(reader: Box<dyn Read + Send>, writer: Box<dyn Write + Send>, handler: Handler) -> Self {
        let shared = Arc::new(Shared {
            writer: Mutex::new(Some(writer)),
            pending: Mutex::new(HashMap::new()),
            subscribers: Mutex::new(HashMap::new()),
            closed: Mutex::new(None),
        });
        let reader_shared = shared.clone();
        std::thread::spawn(move || read_loop(reader, &reader_shared, &handler));
        Self { shared, next_id: AtomicU64::new(0) }
    }

    /// Send a request and wait for its result until `deadline`.
    pub fn request(&self, method: &str, params: Value, deadline: Instant) -> Result<Value, CallError> {
        self.request_until(method, params, deadline, None)
    }

    /// Like [`Connection::request`]. The wait also stops when `cancel` becomes true.
    pub fn request_until(&self, method: &str, params: Value, deadline: Instant, cancel: Option<&AtomicBool>) -> Result<Value, CallError> {
        let (id, rx) = self.send_request(method, params)?;
        let result = wait(&rx, deadline, cancel);
        if result.is_err() {
            self.shared.pending.lock().remove(&id);
        }
        result.map_err(|e| match e {
            CallError::Closed(_) => CallError::Closed(self.shared.reason()),
            other => other,
        })
    }

    /// Send a request and do not wait for its result.
    pub fn request_detached(&self, method: &str, params: Value) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let _ = self.shared.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
    }

    fn send_request(&self, method: &str, params: Value) -> Result<(u64, Receiver<Reply>), CallError> {
        if self.shared.closed.lock().is_some() {
            return Err(CallError::Closed(self.shared.reason()));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel();
        self.shared.pending.lock().insert(id, tx);
        if let Err(e) = self.shared.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })) {
            self.shared.pending.lock().remove(&id);
            return Err(e);
        }
        Ok((id, rx))
    }

    /// Send a notification.
    pub fn notify(&self, method: &str, params: Value) -> Result<(), CallError> {
        self.shared.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    /// Receive the notifications of one session until the subscription drops.
    pub fn subscribe(&self, session_id: &str) -> Subscription {
        let (tx, rx) = channel();
        self.shared.subscribers.lock().insert(session_id.to_string(), tx);
        Subscription { shared: self.shared.clone(), session_id: session_id.to_string(), rx }
    }

    /// True when the other side closed its output, or a write failed.
    pub fn is_closed(&self) -> bool {
        self.shared.closed.lock().is_some()
    }

    /// Close the output, so the other side reads the end of its input.
    pub fn close(&self) {
        self.shared.writer.lock().take();
        self.shared.close("the connection was closed".into());
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.close();
    }
}

/// Wait for a reply until the deadline, in short steps when the wait can be cancelled.
fn wait(rx: &Receiver<Reply>, deadline: Instant, cancel: Option<&AtomicBool>) -> Result<Value, CallError> {
    loop {
        if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
            return Err(CallError::Cancelled);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        let step = if cancel.is_some() { left.min(Duration::from_millis(100)) } else { left };
        match rx.recv_timeout(step) {
            Ok(reply) => return reply.map_err(CallError::Rpc),
            Err(RecvTimeoutError::Timeout) if Instant::now() >= deadline => return Err(CallError::Timeout),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(CallError::Closed(String::new())),
        }
    }
}

/// The notifications of one session. Dropping it ends the subscription.
pub(crate) struct Subscription {
    shared: Arc<Shared>,
    session_id: String,
    rx: Receiver<Notification>,
}

impl Subscription {
    /// The notifications that arrived so far, in order.
    pub fn drain(&self) -> Vec<Notification> {
        self.rx.try_iter().collect()
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.shared.subscribers.lock().remove(&self.session_id);
    }
}

fn read_loop(reader: Box<dyn Read + Send>, shared: &Shared, handler: &Handler) {
    for raw in BufReader::new(reader).split(b'\n') {
        let Ok(raw) = raw else { break };
        let line = String::from_utf8_lossy(&raw);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            // An agent should log to its error output, but some print to the protocol pipe.
            log::debug!("acp: skipped a line that is not JSON: {}", line.chars().take(200).collect::<String>());
            continue;
        };
        route(message, shared, handler);
    }
    shared.close("the agent closed its output".into());
}

fn route(message: Value, shared: &Shared, handler: &Handler) {
    let id = message.get("id").filter(|id| !id.is_null()).cloned();
    match (message["method"].as_str(), id) {
        (Some(method), Some(id)) => {
            let reply = match handler(method, &message["params"]) {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": error.code, "message": error.message } }),
            };
            let _ = shared.write(&reply);
        }
        (Some(method), None) => {
            let Some(session) = message["params"]["sessionId"].as_str() else { return };
            if let Some(tx) = shared.subscribers.lock().get(session) {
                let _ = tx.send(Notification { method: method.to_string(), params: message["params"].clone() });
            }
        }
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else { return };
            let Some(tx) = shared.pending.lock().remove(&id) else { return };
            let reply = match message.get("error").filter(|e| !e.is_null()) {
                Some(error) => Err(RpcError::from_value(error)),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = tx.send(reply);
        }
        (None, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{PipeReader, PipeWriter};

    /// A connection to a fake peer. The peer reads our lines from `from_us` and writes to `to_us`.
    fn pair(handler: Handler) -> (Connection, BufReader<PipeReader>, PipeWriter) {
        let (our_reader, to_us) = std::io::pipe().unwrap();
        let (from_us, our_writer) = std::io::pipe().unwrap();
        (Connection::new(Box::new(our_reader), Box::new(our_writer), handler), BufReader::new(from_us), to_us)
    }

    fn no_requests() -> Handler {
        Box::new(|method, _| Err(RpcError::new(-32601, format!("no {method}"))))
    }

    fn read_message(reader: &mut BufReader<PipeReader>) -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn send(writer: &mut PipeWriter, message: Value) {
        writeln!(writer, "{message}").unwrap();
    }

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    #[test]
    fn a_result_and_an_error_reach_their_callers() {
        let (conn, mut from_us, mut to_us) = pair(no_requests());
        let peer = std::thread::spawn(move || {
            let first = read_message(&mut from_us);
            let second = read_message(&mut from_us);
            assert_eq!((first["jsonrpc"].as_str(), first["method"].as_str()), (Some("2.0"), Some("one")));
            assert_eq!(first["params"], json!({ "x": 1 }));
            // The answers come in the other order, and a log line comes first.
            writeln!(to_us, "not json, a log line").unwrap();
            send(&mut to_us, json!({ "jsonrpc": "2.0", "id": second["id"], "error": { "code": -32000, "message": "Authentication required", "data": "log in" } }));
            send(&mut to_us, json!({ "jsonrpc": "2.0", "id": first["id"], "result": { "ok": true } }));
            to_us
        });
        let conn = Arc::new(conn);
        let other = conn.clone();
        let one = std::thread::spawn(move || other.request("one", json!({ "x": 1 }), soon()));
        std::thread::sleep(Duration::from_millis(50));
        let two = conn.request("two", json!({}), soon());
        assert_eq!(one.join().unwrap(), Ok(json!({ "ok": true })));
        let Err(CallError::Rpc(error)) = two else { panic!("{two:?}") };
        assert_eq!((error.code, error.detail().as_str()), (-32000, "Authentication required: log in"));
        drop(peer.join().unwrap());
    }

    #[test]
    fn notifications_go_to_the_subscriber_of_their_session() {
        let (conn, mut from_us, mut to_us) = pair(no_requests());
        let mine = conn.subscribe("s1");
        let peer = std::thread::spawn(move || {
            let request = read_message(&mut from_us);
            for (session, text) in [("s1", "a"), ("s2", "lost"), ("s1", "b")] {
                send(&mut to_us, json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": session, "update": { "text": text } } }));
            }
            send(&mut to_us, json!({ "jsonrpc": "2.0", "id": request["id"], "result": null }));
            to_us
        });
        assert_eq!(conn.request("go", json!({}), soon()), Ok(Value::Null));
        // Lines are read in order, so the notifications before the result are already there.
        let texts: Vec<Value> = mine.drain().into_iter().map(|n| n.params["update"]["text"].clone()).collect();
        assert_eq!(texts, [json!("a"), json!("b")]);
        drop(peer.join().unwrap());
    }

    #[test]
    fn a_request_of_the_peer_gets_the_answer_of_the_handler() {
        let handler: Handler = Box::new(|method, params| match method {
            "echo" => Ok(params.clone()),
            other => Err(RpcError::new(-32601, format!("Method not found: {other}"))),
        });
        let (_conn, mut from_us, mut to_us) = pair(handler);
        send(&mut to_us, json!({ "jsonrpc": "2.0", "id": "a", "method": "echo", "params": { "v": 2 } }));
        assert_eq!(read_message(&mut from_us), json!({ "jsonrpc": "2.0", "id": "a", "result": { "v": 2 } }));
        send(&mut to_us, json!({ "jsonrpc": "2.0", "id": 7, "method": "fs/read_text_file", "params": {} }));
        let refused = read_message(&mut from_us);
        assert_eq!((refused["id"].as_i64(), refused["error"]["code"].as_i64()), (Some(7), Some(-32601)));
    }

    #[test]
    fn the_deadline_and_the_cancel_flag_end_the_wait() {
        let (conn, _from_us, _to_us) = pair(no_requests());
        let started = Instant::now();
        assert_eq!(conn.request("slow", json!({}), Instant::now() + Duration::from_millis(100)), Err(CallError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(2));
        let cancel = AtomicBool::new(true);
        assert_eq!(conn.request_until("slow", json!({}), soon(), Some(&cancel)), Err(CallError::Cancelled));
        assert!(conn.shared.pending.lock().is_empty(), "a call that gave up leaves no entry");
    }

    #[test]
    fn a_closed_peer_ends_the_waiting_calls() {
        let (conn, from_us, to_us) = pair(no_requests());
        let closer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(to_us);
            from_us
        });
        assert_eq!(conn.request("x", json!({}), soon()), Err(CallError::Closed("the agent closed its output".into())));
        assert!(conn.is_closed());
        assert!(matches!(conn.request("y", json!({}), soon()), Err(CallError::Closed(_))));
        drop(closer.join().unwrap());
    }
}
