//! Replies of the host, and everything that the host sends.

use crate::codec::{Kind, tagged_kind};
use crate::{Capability, ErrorCode, Event, Fields, ModelInfo};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The reply to a request: `ok` with the fields of that request, or `error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Ok(Fields),
    Error(ErrorReply),
}

tagged_kind!(Reply {
    Ok => "ok",
    Error => "error",
});

impl Reply {
    /// `ok` with no fields.
    pub fn ok() -> Reply {
        Reply::Ok(Fields::new())
    }

    /// `ok` with the fields of `body`, a struct such as [`HelloReply`].
    pub fn ok_with(body: &impl Serialize) -> Reply {
        match serde_json::to_value(body) {
            Ok(serde_json::Value::Object(fields)) => Reply::Ok(fields),
            _ => panic!("the body of an ok reply must be a struct with named fields"),
        }
    }

    pub fn error(code: Option<ErrorCode>, message: impl Into<String>) -> Reply {
        Reply::Error(ErrorReply { code, message: message.into(), capability: None, reason: None })
    }
}

/// Read the fields of an `ok` reply as `T`, for example [`HelloReply`].
pub fn parse_ok<T: DeserializeOwned>(fields: &Fields) -> serde_json::Result<T> {
    serde_json::from_value(serde_json::Value::Object(fields.clone()))
}

/// An `error` reply. `message` is English, for people.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorReply {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<ErrorCode>,
    pub message: String,
    /// The capability that the user did not grant, with the code `capability`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<Capability>,
    /// A reason code of the engine protocol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl std::fmt::Display for ErrorReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.code {
            Some(code) => write!(f, "{} ({code})", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ErrorReply {}

/// `ok` to `hello`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloReply {
    /// The version of Sayso.
    pub host_version: String,
    pub protocol: u32,
    /// The optional parts of the Host API that this host serves.
    #[serde(default)]
    pub features: Vec<String>,
    /// The id of the extension that the host found for this client.
    pub extension_id: String,
    /// The capabilities that the user granted.
    #[serde(default)]
    pub capabilities: Vec<Capability>,
}

/// `ok` to `list_models`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListModelsReply {
    pub models: Vec<ModelInfo>,
}

/// `ok` to `transcribe_file`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobReply {
    pub job: u64,
}

/// `ok` to `session_start`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionReply {
    pub session: u64,
}

/// `ok` to `session_stop`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextReply {
    pub text: String,
}

/// `ok` to `enhance`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnhanceReply {
    pub text: String,
    pub provider: String,
    pub model: String,
}

/// `ok` to `subscribe`: the events that the host sends to this connection
/// from now on. A name that the host does not know is not in the list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubscribeReply {
    pub events: Vec<String>,
}

/// Any message from the host: a reply or an event.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum HostMessage {
    Reply(Reply),
    Event(Event),
}

impl Kind for HostMessage {
    fn knows(kind: &str) -> bool {
        Reply::knows(kind) || Event::knows(kind)
    }
    fn kind(&self) -> &'static str {
        match self {
            HostMessage::Reply(reply) => reply.kind(),
            HostMessage::Event(event) => event.kind(),
        }
    }
    fn from_fields(fields: Fields) -> serde_json::Result<Self> {
        let is_reply = fields.get("type").and_then(|kind| kind.as_str()).is_some_and(Reply::knows);
        if is_reply { Reply::from_fields(fields).map(HostMessage::Reply) } else { Event::from_fields(fields).map(HostMessage::Event) }
    }
}
