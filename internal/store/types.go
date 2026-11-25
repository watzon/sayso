package store

import (
	"bytes"
	"encoding/json"
	"fmt"
	"math"
	"strings"
	"time"
	"unicode/utf8"
)

// The record shapes of the history and the dictionary.
// JSON matches serde, so a database written by the Rust app reads the same here.

// Word is a dictionary word (recognition bias).
type Word struct {
	ID   int64  `json:"id"`
	Text string `json:"text"`
}

// Replacement is a rule applied after transcription.
type Replacement struct {
	ID int64 `json:"id"`
	// From is what the speaker says ("git hub").
	From string `json:"from"`
	// To is what to write; `\n` writes a line break.
	To            string `json:"to"`
	CaseSensitive bool   `json:"case_sensitive"`
	// Uses counts how many times the rule was applied.
	Uses uint64 `json:"uses"`
}

// AppliedReplacement is one rule that changed the text, for the History trail.
type AppliedReplacement struct {
	ReplacementID int64  `json:"replacement_id"`
	From          string `json:"from"`
	To            string `json:"to"`
	Count         int    `json:"count"`
}

// TargetApp is the app the text went into.
type TargetApp struct {
	BundleID string `json:"bundle_id"`
	Name     string `json:"name"`
	// Site is the host of the page, when the app showed one.
	Site string `json:"site,omitempty"`
}

// EnhanceOutcome kinds.
const (
	EnhanceNotUsed = "not_used"
	EnhanceApplied = "applied"
	EnhanceFailed  = "failed"
)

// EnhanceOutcome is what the style step did.
type EnhanceOutcome struct {
	Result     string // EnhanceNotUsed, EnhanceApplied, EnhanceFailed
	ProviderID string
	Model      string // Applied
	ElapsedMs  uint64 // Applied
	Reason     string // Failed
}

func (e EnhanceOutcome) MarshalJSON() ([]byte, error) {
	switch e.Result {
	case EnhanceApplied:
		return marshalPlain(struct {
			Result     string `json:"result"`
			ProviderID string `json:"provider_id"`
			Model      string `json:"model"`
			ElapsedMs  uint64 `json:"elapsed_ms"`
		}{e.Result, e.ProviderID, e.Model, e.ElapsedMs})
	case EnhanceFailed:
		return marshalPlain(struct {
			Result     string `json:"result"`
			ProviderID string `json:"provider_id"`
			Reason     string `json:"reason"`
		}{e.Result, e.ProviderID, e.Reason})
	}
	return []byte(`{"result":"not_used"}`), nil
}

func (e *EnhanceOutcome) UnmarshalJSON(b []byte) error {
	var raw struct {
		Result     string  `json:"result"`
		ProviderID *string `json:"provider_id"`
		Model      *string `json:"model"`
		ElapsedMs  *uint64 `json:"elapsed_ms"`
		Reason     *string `json:"reason"`
	}
	if err := json.Unmarshal(b, &raw); err != nil {
		return err
	}
	need := func(p *string, name string) (string, error) {
		if p == nil {
			return "", fmt.Errorf("missing field `%s`", name)
		}
		return *p, nil
	}
	var err error
	out := EnhanceOutcome{Result: raw.Result}
	switch raw.Result {
	case EnhanceNotUsed:
	case EnhanceApplied:
		if out.ProviderID, err = need(raw.ProviderID, "provider_id"); err != nil {
			return err
		}
		if out.Model, err = need(raw.Model, "model"); err != nil {
			return err
		}
		if raw.ElapsedMs == nil {
			return fmt.Errorf("missing field `elapsed_ms`")
		}
		out.ElapsedMs = *raw.ElapsedMs
	case EnhanceFailed:
		if out.ProviderID, err = need(raw.ProviderID, "provider_id"); err != nil {
			return err
		}
		if out.Reason, err = need(raw.Reason, "reason"); err != nil {
			return err
		}
	default:
		return fmt.Errorf("unknown variant `%s`", raw.Result)
	}
	*e = out
	return nil
}

// InsertOutcome kinds.
const (
	InsertPasted      = "pasted"
	InsertTyped       = "typed"
	InsertFailed      = "failed"
	InsertNotInserted = "not_inserted"
)

// InsertOutcome is what the insert step did.
type InsertOutcome struct {
	Result            string // InsertPasted, InsertTyped, InsertFailed, InsertNotInserted
	ClipboardRestored bool   // Pasted
	Reason            string // Failed
}

func (i InsertOutcome) MarshalJSON() ([]byte, error) {
	switch i.Result {
	case InsertPasted:
		return marshalPlain(struct {
			Result            string `json:"result"`
			ClipboardRestored bool   `json:"clipboard_restored"`
		}{i.Result, i.ClipboardRestored})
	case InsertFailed:
		return marshalPlain(struct {
			Result string `json:"result"`
			Reason string `json:"reason"`
		}{i.Result, i.Reason})
	case InsertNotInserted:
		return []byte(`{"result":"not_inserted"}`), nil
	}
	return []byte(`{"result":"typed"}`), nil
}

func (i *InsertOutcome) UnmarshalJSON(b []byte) error {
	var raw struct {
		Result            string  `json:"result"`
		ClipboardRestored *bool   `json:"clipboard_restored"`
		Reason            *string `json:"reason"`
	}
	if err := json.Unmarshal(b, &raw); err != nil {
		return err
	}
	out := InsertOutcome{Result: raw.Result}
	switch raw.Result {
	case InsertTyped, InsertNotInserted:
	case InsertPasted:
		if raw.ClipboardRestored == nil {
			return fmt.Errorf("missing field `clipboard_restored`")
		}
		out.ClipboardRestored = *raw.ClipboardRestored
	case InsertFailed:
		if raw.Reason == nil {
			return fmt.Errorf("missing field `reason`")
		}
		out.Reason = *raw.Reason
	default:
		return fmt.Errorf("unknown variant `%s`", raw.Result)
	}
	*i = out
	return nil
}

// ProviderReport is what a provider said about one run.
type ProviderReport struct {
	RunMs            *uint64 `json:"run_ms,omitempty"`
	APIMs            *uint64 `json:"api_ms,omitempty"`
	Turns            *uint64 `json:"turns,omitempty"`
	InputTokens      *uint64 `json:"input_tokens,omitempty"`
	OutputTokens     *uint64 `json:"output_tokens,omitempty"`
	ThinkingTokens   *uint64 `json:"thinking_tokens,omitempty"`
	CacheReadTokens  *uint64 `json:"cache_read_tokens,omitempty"`
	CacheWriteTokens *uint64 `json:"cache_write_tokens,omitempty"`
}

// Diagnostics is how a dictation ran, for a bug report. No dictated text.
type Diagnostics struct {
	AppVersion         string            `json:"app_version"`
	System             SystemInfo        `json:"system"`
	StartToCaptureMs   *uint64           `json:"start_to_capture_ms"`
	StopToCaptureEndMs *uint64           `json:"stop_to_capture_end_ms"`
	StopToRequestMs    *uint64           `json:"stop_to_request_ms"`
	StopToInsertMs     *uint64           `json:"stop_to_insert_ms"`
	Transcribe         TranscribeTimings `json:"transcribe"`
	Enhance            *EnhanceTimings   `json:"enhance"`
	Insert             *InsertTimings    `json:"insert"`
	ChangedLater       bool              `json:"changed_later"`
}

// SystemInfo describes the system of a dictation.
type SystemInfo struct {
	OS        string  `json:"os"`
	OSVersion *string `json:"os_version"`
	Arch      string  `json:"arch"`
	Session   *string `json:"session"`
	UI        *string `json:"ui"`
	Desktop   *string `json:"desktop"`
	Package   *string `json:"package"`
}

// TranscribeTimings is the transcribe step.
type TranscribeTimings struct {
	StepMs      uint64 `json:"step_ms"`
	ModelWaitMs uint64 `json:"model_wait_ms"`
	Cloud       bool   `json:"cloud"`
	Fallback    bool   `json:"fallback"`
	Language    string `json:"language"`
	LivePreview bool   `json:"live_preview"`
}

// EnhanceTimings is the style step.
type EnhanceTimings struct {
	StepMs       uint64           `json:"step_ms"`
	SetupMs      uint64           `json:"setup_ms"`
	ProviderKind string           `json:"provider_kind"`
	Thinking     *string          `json:"thinking"`
	TimeoutMs    uint64           `json:"timeout_ms"`
	PromptChars  int              `json:"prompt_chars"`
	Attempts     []EnhanceAttempt `json:"attempts"`
	Cleanup      *CleanupTimings  `json:"cleanup,omitempty"`
}

// MarshalJSON writes an empty attempt list as [], as serde does.
func (e EnhanceTimings) MarshalJSON() ([]byte, error) {
	type plain EnhanceTimings
	if e.Attempts == nil {
		e.Attempts = []EnhanceAttempt{}
	}
	return marshalPlain(plain(e))
}

// CleanupTimings is how a cleanup model ran.
type CleanupTimings struct {
	LoadMs      uint64  `json:"load_ms"`
	Parts       uint32  `json:"parts"`
	TokensIn    uint32  `json:"tokens_in"`
	TokensOut   uint32  `json:"tokens_out"`
	Language    *string `json:"language,omitempty"`
	CleanupOnly bool    `json:"cleanup_only"`
}

// EnhanceAttempt is one call of a provider.
type EnhanceAttempt struct {
	ElapsedMs uint64          `json:"elapsed_ms"`
	Error     *string         `json:"error"`
	Mode      *string         `json:"mode"`
	Report    *ProviderReport `json:"report"`
}

// InsertTimings is the insert step.
type InsertTimings struct {
	ElapsedMs      uint64  `json:"elapsed_ms"`
	Method         string  `json:"method"`
	RestoreDelayMs *uint64 `json:"restore_delay_ms"`
	Backend        *string `json:"backend"`
}

// HistoryEntry is one dictation.
type HistoryEntry struct {
	ID         int64
	CreatedAt  time.Time
	DurationMs uint64
	App        *TargetApp
	// Transcript is the raw model output; FinalText is what was inserted.
	Transcript   string
	FinalText    string
	StyleID      string
	Model        string
	TranscribeMs uint64
	Replacements []AppliedReplacement
	Enhance      EnhanceOutcome
	Insert       InsertOutcome
	// AudioFile is a file name in the audio directory; "" when not saved or expired.
	AudioFile string
	// Waveform is the peak summary, kept after the audio expires.
	Waveform    []byte
	Diagnostics *Diagnostics
}

// WordCount counts words as the stats do.
func (e HistoryEntry) WordCount() int { return WordCount(e.FinalText) }

// WordCount is the number of whitespace-separated words.
func WordCount(text string) int { return len(strings.Fields(text)) }

// MetadataJSON is the entry for a bug report: the lengths of the text, never
// the text or the site.
func (e HistoryEntry) MetadataJSON() string {
	var appID, appName *string
	if e.App != nil {
		appID, appName = &e.App.BundleID, &e.App.Name
	}
	replacements := 0
	for _, r := range e.Replacements {
		replacements += max(r.Count, 1)
	}
	m := struct {
		ID              int64          `json:"id"`
		CreatedAt       string         `json:"created_at"`
		RecordingMs     uint64         `json:"recording_ms"`
		AppID           *string        `json:"app_id"`
		AppName         *string        `json:"app_name"`
		Model           string         `json:"model"`
		TranscribeMs    uint64         `json:"transcribe_ms"`
		TranscriptChars int            `json:"transcript_chars"`
		FinalTextChars  int            `json:"final_text_chars"`
		Replacements    int            `json:"replacements"`
		Style           string         `json:"style"`
		Enhance         EnhanceOutcome `json:"enhance"`
		Insert          InsertOutcome  `json:"insert"`
		AudioSaved      bool           `json:"audio_saved"`
		Diagnostics     *Diagnostics   `json:"diagnostics"`
	}{
		e.ID, ChronoTime(e.CreatedAt), e.DurationMs, appID, appName, e.Model, e.TranscribeMs,
		utf8.RuneCountInString(e.Transcript), utf8.RuneCountInString(e.FinalText), replacements,
		e.StyleID, e.Enhance, e.Insert, e.AudioFile != "", e.Diagnostics,
	}
	return prettyJSON(m)
}

// ChronoTime formats a time as chrono's DateTime<Utc> serializes it:
// RFC 3339 in UTC with "Z" and only the fraction digits it needs (0, 3, 6, or 9).
func ChronoTime(t time.Time) string {
	t = t.UTC()
	ns := t.Nanosecond()
	switch {
	case ns == 0:
		return t.Format("2006-01-02T15:04:05Z")
	case ns%1_000_000 == 0:
		return t.Format("2006-01-02T15:04:05.000Z")
	case ns%1_000 == 0:
		return t.Format("2006-01-02T15:04:05.000000Z")
	}
	return t.Format("2006-01-02T15:04:05.000000000Z")
}

// prettyJSON matches serde_json::to_string_pretty: two-space indent, and
// no HTML escaping.
func prettyJSON(v any) string {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	if err := enc.Encode(v); err != nil {
		return ""
	}
	return strings.TrimSuffix(buf.String(), "\n")
}

// marshalPlain is json.Marshal without HTML escaping, as serde writes.
func marshalPlain(v any) ([]byte, error) {
	s, err := compactJSON(v)
	return []byte(s), err
}

// compactJSON matches serde_json::to_string.
func compactJSON(v any) (string, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return "", err
	}
	return strings.TrimSuffix(buf.String(), "\n"), nil
}

// WaveformBuckets is the number of peaks the history keeps.
const WaveformBuckets = 96

// WaveformSummary gives `buckets` peaks from 0 to 255 (RMS per bucket,
// normalized to the loudest).
func WaveformSummary(samples []float32, buckets int) []byte {
	out := make([]byte, buckets)
	if len(samples) == 0 || buckets == 0 {
		return out
	}
	per := math.Max(float64(len(samples))/float64(buckets), 1.0)
	peaks := make([]float32, buckets)
	var maxPeak float32
	for i := range peaks {
		start := int(float64(i) * per)
		end := min(int(float64(i+1)*per), len(samples))
		if start >= end {
			continue
		}
		var sum float32
		for _, s := range samples[start:end] {
			sum += s * s
		}
		peaks[i] = float32(math.Sqrt(float64(sum / float32(end-start))))
		maxPeak = max(maxPeak, peaks[i])
	}
	for i, p := range peaks {
		if maxPeak > 0 {
			p /= maxPeak
		}
		out[i] = uint8(math.Round(float64(min(max(p, 0), 1) * 255)))
	}
	return out
}
