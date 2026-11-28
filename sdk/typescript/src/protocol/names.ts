// Every name of the protocols, as sdk/protocol/testdata/names.json lists them.

/** The version of the Host API, in `v` of each message and in `protocol` of `hello`. */
export const PROTOCOL = 1;

export const CAPABILITIES = ["models.read", "models.download", "transcribe.file", "session.live", "audio.mic", "audio.system", "ai.enhance", "dictation.observe"] as const;
export const ERROR_CODES = ["not_approved", "capability", "busy", "declined", "model_not_downloaded", "wrong_role", "unsupported", "bad_request", "too_many_streams", "unknown_model", "model_retired", "unavailable", "refused", "timeout", "permission", "host_not_running", "connection_lost", "host_closing"] as const;
export const REQUEST_TYPES = ["hello", "list_models", "download_model", "transcribe_file", "cancel_job", "session_start", "session_audio", "session_stop", "enhance", "subscribe", "bye"] as const;
export const EVENT_TYPES = ["model_state", "download_progress", "job_progress", "segment", "speakers", "job_done", "job_failed", "partial", "source_gap", "source_ended", "part_failed", "session_ended", "silence_limit", "dictation_done", "approval_waiting", "host_closing"] as const;
export const ROLES = ["final", "streaming", "vad", "diarization"] as const;
export const SOURCES = ["mic", "system", "client"] as const;
export const SOURCE_ENDS = ["permission", "failed"] as const;
export const MODEL_STATES = ["not_downloaded", "downloading", "downloaded", "optimizing", "ready", "failed"] as const;
export const TIMESTAMP_LEVELS = ["word", "segment", "none"] as const;
export const SUBSCRIBE_EVENTS = ["dictation_done"] as const;

/** The optional parts of the Host API in `hello.features`. */
export const HOST_FEATURES = ["dictation_done", "download_question", "enhance", "silence_limit", "record_to", "mic_capture", "system_capture", "echo_rule", "vad", "diarization", "segments", "detected_language"] as const;
export const HOST_ENGINE_FEATURES = ["vad", "diarization", "segments", "detected_language"] as const;

export const ENGINE_FEATURES = ["unload", "keep_preview", "prepare", "language_model", "concurrent_streams", "segments", "vad", "diarization", "catalog", "detected_language", "store_v1"] as const;
export const ENGINE_REASONS = ["incomplete_download", "download_failed", "load_failed", "no_gpu", "format_changed", "unsupported_cpu", "device_not_eligible", "intelligence_off", "model_not_ready", "needs_macos_26", "unavailable", "timeout", "refused", "failed"] as const;
export const ENGINE_ERROR_CODES = ["too_many_streams", "unknown_model", "model_retired", "unavailable", "refused", "timeout"] as const;
export const ENGINE_REQUEST_TYPES = ["hello", "list_models", "download", "delete", "load", "unload", "prepare", "transcribe", "stream_start", "stream_audio", "stream_end", "vad", "vad_start", "diarize", "language_model", "generate", "shutdown"] as const;
export const ENGINE_REPLY_TYPES = ["hello", "ok", "final", "error"] as const;
export const ENGINE_EVENT_TYPES = ["model_state", "download_progress", "part_state", "partial", "speech_start", "speech_end", "log"] as const;

export const CLEANUP_PROTOCOL = 1;
export const CLEANUP_REQUEST_TYPES = ["hello", "load", "count", "complete", "unload"] as const;
export const CLEANUP_REPLY_TYPES = ["hello", "loaded", "count", "completion", "unloaded", "error"] as const;
export const CLEANUP_ERROR_CODES = ["bad_request", "not_loaded", "wrong_model", "load_failed", "decode_failed"] as const;
export const CLEANUP_STOPS = ["end", "limit", "context"] as const;

/** Every name, in the shape of `names.json`. */
export function names(): Record<string, Record<string, unknown>> {
	return {
		host: {
			protocol: PROTOCOL,
			capabilities: CAPABILITIES,
			error_codes: ERROR_CODES,
			request_types: REQUEST_TYPES,
			event_types: EVENT_TYPES,
			roles: ROLES,
			sources: SOURCES,
			source_ends: SOURCE_ENDS,
			model_states: MODEL_STATES,
			timestamp_levels: TIMESTAMP_LEVELS,
			subscribe_events: SUBSCRIBE_EVENTS,
			features: HOST_FEATURES,
			engine_features: HOST_ENGINE_FEATURES,
		},
		engine: {
			protocol: 1,
			features: ENGINE_FEATURES,
			reasons: ENGINE_REASONS,
			error_codes: ENGINE_ERROR_CODES,
			request_types: ENGINE_REQUEST_TYPES,
			reply_types: ENGINE_REPLY_TYPES,
			event_types: ENGINE_EVENT_TYPES,
		},
		cleanup: {
			protocol: CLEANUP_PROTOCOL,
			request_types: CLEANUP_REQUEST_TYPES,
			reply_types: CLEANUP_REPLY_TYPES,
			error_codes: CLEANUP_ERROR_CODES,
			stops: CLEANUP_STOPS,
		},
	};
}
