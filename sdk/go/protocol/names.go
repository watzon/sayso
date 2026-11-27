package protocol

import (
	"encoding/json"
	"fmt"
)

// ErrorCodes are the error codes that this version knows.
var ErrorCodes = []ErrorCode{CodeNotApproved, CodeCapability, CodeBusy, CodeDeclined, CodeModelNotDownloaded, CodeWrongRole, CodeUnsupported, CodeBadRequest, CodeTooManyStreams, CodeUnknownModel, CodeModelRetired, CodeUnavailable, CodeRefused, CodeTimeout, CodePermission, CodeHostNotRunning, CodeConnectionLost, CodeHostClosing}

// Roles, Sources, ModelStates, and TimestampLevels are the names of their
// kind that this version knows.
var (
	Roles           = []Role{RoleFinal, RoleStreaming, RoleVad, RoleDiarization}
	Sources         = []Source{SourceMic, SourceSystem, SourceClient}
	SourceEnds      = []SourceEndReason{EndPermission, EndFailed}
	ModelStates     = []ModelState{StateNotDownloaded, StateDownloading, StateDownloaded, StateOptimizing, StateReady, StateFailed}
	TimestampLevels = []TimestampLevel{TimestampsWord, TimestampsSegment, TimestampsNone}
)

// SubscribeEvents are the events that subscribe can ask for.
var SubscribeEvents = []string{"dictation_done"}

// Host features: the optional parts of the Host API in hello.features.
const (
	FeatureDictationDone    = "dictation_done"
	FeatureDownloadQuestion = "download_question"
	FeatureEnhance          = "enhance"
	FeatureSilenceLimit     = "silence_limit"
	FeatureRecordTo         = "record_to"
	FeatureMicCapture       = "mic_capture"
	FeatureSystemCapture    = "system_capture"
	FeatureEchoRule         = "echo_rule"
	FeatureVad              = "vad"
	FeatureDiarization      = "diarization"
	FeatureSegments         = "segments"
	FeatureDetectedLanguage = "detected_language"
)

// HostFeatures are the host features that this version knows.
var HostFeatures = []string{FeatureDictationDone, FeatureDownloadQuestion, FeatureEnhance, FeatureSilenceLimit, FeatureRecordTo, FeatureMicCapture, FeatureSystemCapture, FeatureEchoRule, FeatureVad, FeatureDiarization, FeatureSegments, FeatureDetectedLanguage}

// HostEngineFeatures are the engine features that a host passes on in
// hello.features.
var HostEngineFeatures = []string{FeatureVad, FeatureDiarization, FeatureSegments, FeatureDetectedLanguage}

// EngineFeatures are the features of the engine protocol
// (docs/engine-protocol.md, "Version and features").
var EngineFeatures = []string{"unload", "keep_preview", "prepare", "language_model", "concurrent_streams", "segments", "vad", "diarization", "catalog", "detected_language", "store_v1"}

// EngineReasons are the reason codes of the engine protocol.
var EngineReasons = []string{"incomplete_download", "download_failed", "load_failed", "no_gpu", "format_changed", "unsupported_cpu", "device_not_eligible", "intelligence_off", "model_not_ready", "needs_macos_26", "unavailable", "timeout", "refused", "failed"}

// EngineErrorCodes are the codes of an engine error reply.
var EngineErrorCodes = []string{"too_many_streams", "unknown_model", "model_retired", "unavailable", "refused", "timeout"}

// EngineRequestTypes and EngineEventTypes are the message types of the engine
// protocol.
var (
	EngineRequestTypes = []string{"hello", "list_models", "download", "delete", "load", "unload", "prepare", "transcribe", "stream_start", "stream_audio", "stream_end", "vad", "vad_start", "diarize", "language_model", "generate", "shutdown"}
	EngineReplyTypes   = []string{"hello", "ok", "final", "error"}
	EngineEventTypes   = []string{"model_state", "download_progress", "part_state", "partial", "speech_start", "speech_end", "log"}
)

// The cleanup runtime protocol: its message types, error codes, and stops.
var (
	CleanupProtocol     = 1
	CleanupRequestTypes = []string{"hello", "load", "count", "complete", "unload"}
	CleanupReplyTypes   = []string{"hello", "loaded", "count", "completion", "unloaded", "error"}
	CleanupErrorCodes   = []string{"bad_request", "not_loaded", "wrong_model", "load_failed", "decode_failed"}
	CleanupStops        = []string{"end", "limit", "context"}
)

// Names is every name of the protocols, as sdk/protocol/testdata/names.json
// lists them.
func Names() map[string]any {
	return map[string]any{
		"host": map[string]any{
			"protocol":         Protocol,
			"capabilities":     AllCapabilities,
			"error_codes":      ErrorCodes,
			"request_types":    RequestTypes,
			"event_types":      EventTypes,
			"roles":            Roles,
			"sources":          Sources,
			"source_ends":      SourceEnds,
			"model_states":     ModelStates,
			"timestamp_levels": TimestampLevels,
			"subscribe_events": SubscribeEvents,
			"features":         HostFeatures,
			"engine_features":  HostEngineFeatures,
		},
		"engine": map[string]any{
			"protocol":      1,
			"features":      EngineFeatures,
			"reasons":       EngineReasons,
			"error_codes":   EngineErrorCodes,
			"request_types": EngineRequestTypes,
			"reply_types":   EngineReplyTypes,
			"event_types":   EngineEventTypes,
		},
		"cleanup": map[string]any{
			"protocol":      CleanupProtocol,
			"request_types": CleanupRequestTypes,
			"reply_types":   CleanupReplyTypes,
			"error_codes":   CleanupErrorCodes,
			"stops":         CleanupStops,
		},
	}
}

// NamesJSON is Names as indented JSON with a final new line.
func NamesJSON() []byte {
	b, err := json.MarshalIndent(Names(), "", "  ")
	if err != nil {
		panic(fmt.Sprintf("names serialize to JSON: %v", err))
	}
	return append(b, '\n')
}
