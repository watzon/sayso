//! The names in messages: capabilities, error codes, roles, states, and
//! sources. Each keeps a name that this version does not know as `Other`, so
//! a message from a newer peer still reads.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

macro_rules! names {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident = $text:literal,)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$vmeta])* $variant,)*
            /// A name that this version does not know, from a newer peer.
            Other(String),
        }

        impl $name {
            /// Every name that this version knows.
            pub const ALL: &'static [$name] = &[$($name::$variant),*];

            pub fn as_str(&self) -> &str {
                match self {
                    $($name::$variant => $text,)*
                    $name::Other(text) => text,
                }
            }

            pub fn parse(text: &str) -> $name {
                match text {
                    $($text => $name::$variant,)*
                    other => $name::Other(other.to_owned()),
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                Ok($name::parse(&String::deserialize(deserializer)?))
            }
        }
    };
}

names! {
    /// A permission that an app asks for in `hello` and the user grants in the
    /// approval question (`docs/sdk.md`, "Approval").
    Capability {
        /// List models and their state. It needs no grant.
        ModelsRead = "models.read",
        /// Ask the user to download a model.
        ModelsDownload = "models.download",
        /// Jobs: transcribe files.
        TranscribeFile = "transcribe.file",
        /// Live sessions with audio from the app.
        SessionLive = "session.live",
        /// Live sessions from the microphone.
        AudioMic = "audio.mic",
        /// Live sessions from the system audio.
        AudioSystem = "audio.system",
        /// The AI providers and styles of the user.
        AiEnhance = "ai.enhance",
        /// Each dictation after it is inserted.
        DictationObserve = "dictation.observe",
    }
}

names! {
    /// The `code` of an `error`: a stable code that a client can act on.
    ErrorCode {
        /// The user did not approve this app, or its program has no identity
        /// that the host can trust. The host closes the connection.
        NotApproved = "not_approved",
        /// The user did not grant the capability in `capability`.
        Capability = "capability",
        /// Another live session runs, or too many connections wait for an
        /// approval question.
        Busy = "busy",
        /// The user said no to a download.
        Declined = "declined",
        /// The model is not on disk. Send `download_model`.
        ModelNotDownloaded = "model_not_downloaded",
        /// The model cannot do the role of the request.
        WrongRole = "wrong_role",
        /// The host or its engine cannot do this on this computer.
        Unsupported = "unsupported",
        /// A required field is missing or has the wrong type.
        BadRequest = "bad_request",
        /// The engine cannot open one more stream.
        TooManyStreams = "too_many_streams",
        /// The model id is not in the catalog.
        UnknownModel = "unknown_model",
        /// The model was taken out of the catalog.
        ModelRetired = "model_retired",
        /// The language model, or for `enhance` the AI provider, is not available.
        Unavailable = "unavailable",
        /// The language model, or for `enhance` the AI provider, refused the text.
        Refused = "refused",
        /// The language model, or for `enhance` the AI provider, gave no reply in time.
        Timeout = "timeout",
        /// The system refused a capture permission (the system audio or the microphone).
        Permission = "permission",
    }
}

names! {
    /// What a model can do.
    Role {
        /// The final pass on a whole recording.
        Final = "final",
        /// Text while the audio arrives.
        Streaming = "streaming",
        /// Voice activity detection.
        Vad = "vad",
        /// Who speaks when.
        Diarization = "diarization",
    }
}

names! {
    /// The state of a model.
    ModelState {
        NotDownloaded = "not_downloaded",
        Downloading = "downloading",
        Downloaded = "downloaded",
        /// A load runs.
        Optimizing = "optimizing",
        /// In memory.
        Ready = "ready",
        Failed = "failed",
    }
}

names! {
    /// How much timing a model gives with its text.
    TimestampLevel {
        Word = "word",
        Segment = "segment",
        None = "none",
    }
}

names! {
    /// Where the audio of a live session comes from.
    Source {
        Mic = "mic",
        System = "system",
        /// The app sends the audio with `session_audio`.
        Client = "client",
    }
}

names! {
    /// Why a source of a live session ended before the session.
    SourceEndReason {
        /// The user took the permission of the system back.
        Permission = "permission",
        /// The capture failed, and it did not come back.
        Failed = "failed",
    }
}
