// Generated from crates/sayso-core/src/model_migration.rs. Do not edit.
// Make it again with: SAYSO_WRITE_CATALOG=1 cargo test -p sayso-core model_migration

/// The old folder of each part of each model, as the migration in Sayso looks for them.
let migrationOldFolders: [(id: String, parts: [(role: String, folder: String)])] = [
    ("parakeet-unified-en", [("model", "parakeet-unified-en-0.6b"), ("ctc", "parakeet-ctc-110m-coreml")]),
    ("parakeet-ultra", [("model", "parakeet-ultra")]),
    ("fluid-speech-mini", [("model", "fluid-speech-mini-coreml")]),
    ("parakeet-tdt-v3", [("model", "parakeet-tdt-0.6b-v3")]),
    ("parakeet-redux", [("model", "parakeet-redux")]),
    ("parakeet-tdt-v2", [("model", "parakeet-tdt-0.6b-v2")]),
    ("phonon-2", [("model", "phonon-2")]),
    ("parakeet-tdt-ctc-110m", [("model", "parakeet-tdt-ctc-110m")]),
    ("parakeet-ja", [("model", "parakeet-ja")]),
    ("parakeet-eou-120m", [("model", "parakeet-eou-streaming/160ms")]),
    ("whisper-large-v3-turbo", [("model", "models/argmaxinc/whisperkit-coreml/openai_whisper-large-v3-v20240930_turbo_632MB"), ("tokenizer", "models/openai/whisper-large-v3")]),
    ("whisper-large-v3", [("model", "models/argmaxinc/whisperkit-coreml/openai_whisper-large-v3_947MB"), ("tokenizer", "models/openai/whisper-large-v3")]),
    ("whisper-large-v2", [("model", "models/argmaxinc/whisperkit-coreml/openai_whisper-large-v2_949MB"), ("tokenizer", "models/openai/whisper-large-v2")]),
    ("whisper-distil-large-v3", [("model", "models/argmaxinc/whisperkit-coreml/distil-whisper_distil-large-v3_594MB"), ("tokenizer", "models/openai/whisper-large-v3")]),
    ("whisper-medium", [("model", "models/argmaxinc/whisperkit-coreml/openai_whisper-medium"), ("tokenizer", "models/openai/whisper-medium")]),
    ("whisper-small", [("model", "models/argmaxinc/whisperkit-coreml/openai_whisper-small_216MB"), ("tokenizer", "models/openai/whisper-small")]),
    ("whisper-base", [("model", "models/argmaxinc/whisperkit-coreml/openai_whisper-base"), ("tokenizer", "models/openai/whisper-base")]),
    ("whisper-tiny", [("model", "models/argmaxinc/whisperkit-coreml/openai_whisper-tiny"), ("tokenizer", "models/openai/whisper-tiny")]),
    ("nemotron-multilingual", [("model", "nemotron-multilingual/multilingual/1120ms")]),
    ("nemotron-en", [("model", "nemotron-streaming/1120ms")]),
    ("cohere-transcribe", [("model", "cohere-transcribe/q8"), ("vocabulary", "cohere-transcribe/q8")]),
    ("canary-1b-v2", [("model", "canary-1b-v2")]),
    ("sensevoice-small", [("model", "sensevoice-small")]),
    ("paraformer-zh", [("model", "paraformer-large-zh")]),
    ("silero-vad", [("model", "silero-vad")]),
    ("speaker-diarization", [("model", "speaker-diarization")]),
]
