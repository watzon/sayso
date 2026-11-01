//! The protocol of the cleanup runtime (`sayso-cleanup`): NDJSON on stdin and
//! stdout, one request at a time. Both sides use these types.
//!
//! Every line has `"v"` and `"id"`. A reply has the `id` of its request. The
//! runtime knows no cleanup model: it loads a GGUF file and completes text. See
//! `docs/cleanup-models.md`, "Runtime and protocol".

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// One request line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestLine {
    pub v: u32,
    pub id: String,
    #[serde(flatten)]
    pub request: Request,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Hello,
    /// Load a model file. A loaded model with another id is unloaded first.
    Load {
        model_id: String,
        path: String,
        n_ctx: u32,
    },
    /// Count the tokens of `text` with the tokenizer of the loaded model.
    Count {
        model_id: String,
        text: String,
    },
    /// Complete `prompt` with greedy decoding until an end-of-generation token,
    /// one of the `end` strings, or `max_tokens`.
    Complete {
        model_id: String,
        prompt: String,
        end: Vec<String>,
        max_tokens: u32,
    },
    Unload,
}

/// One reply line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplyLine {
    pub v: u32,
    pub id: String,
    #[serde(flatten)]
    pub reply: Reply,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Hello {
        version: u32,
        llama_cpp: String,
        backend: String,
    },
    Loaded {
        model_id: String,
        load_ms: u64,
    },
    Count {
        tokens: u32,
    },
    Completion {
        text: String,
        stop: Stop,
        tokens_in: u32,
        tokens_out: u32,
        ms: u64,
    },
    Unloaded,
    Error {
        code: ErrorCode,
        message: String,
    },
}

/// Why a completion stopped. Only `End` is a whole answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    /// An end-of-generation token or one of the `end` strings.
    End,
    /// `max_tokens` ran out.
    Limit,
    /// The context of the model is full.
    Context,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The line is not a request of this protocol.
    BadRequest,
    /// The request needs a model, and no model is loaded.
    NotLoaded,
    /// The request names another model than the loaded one. Load it again.
    WrongModel,
    LoadFailed,
    DecodeFailed,
}

impl RequestLine {
    pub fn new(id: impl Into<String>, request: Request) -> Self {
        RequestLine {
            v: PROTOCOL_VERSION,
            id: id.into(),
            request,
        }
    }
}

impl ReplyLine {
    pub fn new(id: impl Into<String>, reply: Reply) -> Self {
        ReplyLine {
            v: PROTOCOL_VERSION,
            id: id.into(),
            reply,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_have_the_documented_shape() {
        let line = RequestLine::new(
            "3",
            Request::Complete {
                model_id: "s1-mini".into(),
                prompt: "p".into(),
                end: vec!["<|im_end|>".into()],
                max_tokens: 40,
            },
        );
        let json = serde_json::to_value(&line).unwrap();
        assert_eq!(json["v"], 1);
        assert_eq!(json["id"], "3");
        assert_eq!(json["type"], "complete");
        assert_eq!(json["max_tokens"], 40);
        assert_eq!(serde_json::from_value::<RequestLine>(json).unwrap(), line);

        let hello: RequestLine =
            serde_json::from_str(r#"{"v":1,"id":"1","type":"hello"}"#).unwrap();
        assert_eq!(hello.request, Request::Hello);

        let reply = ReplyLine::new(
            "3",
            Reply::Completion {
                text: " Hi.".into(),
                stop: Stop::End,
                tokens_in: 9,
                tokens_out: 2,
                ms: 31,
            },
        );
        let json = serde_json::to_string(&reply).unwrap();
        assert!(
            json.contains(r#""type":"completion""#) && json.contains(r#""stop":"end""#),
            "{json}"
        );
        assert_eq!(serde_json::from_str::<ReplyLine>(&json).unwrap(), reply);

        let error = ReplyLine::new(
            "4",
            Reply::Error {
                code: ErrorCode::WrongModel,
                message: "x".into(),
            },
        );
        assert!(
            serde_json::to_string(&error)
                .unwrap()
                .contains(r#""code":"wrong_model""#)
        );
    }
}
