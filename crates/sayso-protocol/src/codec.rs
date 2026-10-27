//! The envelope of every message: `v`, `type`, and `id`.

use crate::Fields;
use serde::Serialize;
use serde_json::Value;

/// The version of the protocol, in `v` of each message and in `protocol` of
/// `hello`. It changes only when a message changes in a way that breaks old
/// readers. New messages and new optional fields do not change it.
pub const PROTOCOL: u32 = 1;

/// A set of message types: [`crate::Request`], [`crate::Reply`],
/// [`crate::Event`], or [`crate::HostMessage`].
pub trait Kind: Serialize + Sized {
    /// True when `kind` is a `type` of this set.
    fn knows(kind: &str) -> bool;
    /// The `type` of this message.
    fn kind(&self) -> &'static str;
    /// Read a message of a known type from its object (with `type`, without
    /// `v` and `id`).
    fn from_fields(fields: Fields) -> serde_json::Result<Self>;
}

/// A message with its `id`.
#[derive(Debug, Clone, PartialEq)]
pub struct Message<T> {
    pub id: Option<String>,
    pub body: T,
}

/// A line that [`decode`] read.
#[derive(Debug, Clone, PartialEq)]
pub enum Decoded<T> {
    Known(Message<T>),
    /// A `type` that this version does not know, for example from a newer
    /// peer. A host replies with an `error`, and an extension ignores it.
    Unknown { id: Option<String>, kind: String, fields: Fields },
}

/// A line that is not a message that this version can read.
#[derive(Debug, Clone, PartialEq)]
pub enum DecodeError {
    /// Not a JSON object with a string `type`. The reply is an `error`
    /// without `id`.
    NotAMessage(String),
    /// A known `type` with a required field that is missing or has the wrong
    /// type. The reply is an `error` with the `id`.
    BadFields { id: Option<String>, kind: String, message: String },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::NotAMessage(message) => f.write_str(message),
            DecodeError::BadFields { kind, message, .. } => write!(f, "bad {kind} message: {message}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Read one line. `v` is not checked: protocol 1 is the only version.
pub fn decode<T: Kind>(line: &str) -> Result<Decoded<T>, DecodeError> {
    let value: Value = serde_json::from_str(line).map_err(|e| DecodeError::NotAMessage(format!("the line is not JSON: {e}")))?;
    let Value::Object(mut fields) = value else {
        return Err(DecodeError::NotAMessage("the line is not a JSON object".into()));
    };
    let kind = match fields.get("type") {
        Some(Value::String(kind)) => kind.clone(),
        _ => return Err(DecodeError::NotAMessage("the message has no string type".into())),
    };
    // An id that is not a string cannot be echoed as one: the message has none.
    let id = match fields.remove("id") {
        Some(Value::String(id)) => Some(id),
        _ => None,
    };
    fields.remove("v");
    if !T::knows(&kind) {
        fields.remove("type");
        return Ok(Decoded::Unknown { id, kind, fields });
    }
    match T::from_fields(fields) {
        Ok(body) => Ok(Decoded::Known(Message { id, body })),
        Err(e) => Err(DecodeError::BadFields { id, kind, message: e.to_string() }),
    }
}

/// Write one message as one line of JSON, without the `\n`.
pub fn encode<T: Kind>(id: Option<&str>, body: &T) -> String {
    let mut value = serde_json::to_value(body).expect("a message serializes to JSON");
    let fields = value.as_object_mut().expect("a message is a JSON object");
    fields.insert("v".into(), PROTOCOL.into());
    if let Some(id) = id {
        fields.insert("id".into(), id.into());
    }
    value.to_string()
}

/// [`Kind`] for an enum with serde's `tag = "type"` and a list of its types.
macro_rules! tagged_kind {
    ($name:ident { $($variant:ident => $text:literal),* $(,)? }) => {
        impl $name {
            /// Every `type` of this set.
            pub const TYPES: &'static [&'static str] = &[$($text),*];
        }
        impl $crate::codec::Kind for $name {
            fn knows(kind: &str) -> bool {
                Self::TYPES.contains(&kind)
            }
            fn kind(&self) -> &'static str {
                match self {
                    $($name::$variant { .. } => $text,)*
                }
            }
            fn from_fields(fields: $crate::Fields) -> serde_json::Result<Self> {
                serde_json::from_value(serde_json::Value::Object(fields))
            }
        }
    };
}
pub(crate) use tagged_kind;
