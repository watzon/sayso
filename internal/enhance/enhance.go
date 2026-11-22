package enhance

import (
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/watzon/sayso/internal/store"
)

// Request is one enhancement of a transcript.
type Request struct {
	SystemPrompt string
	Transcript   string
	Model        string   // "" uses the provider's model
	Temperature  *float32 // nil uses the provider's default
	Timeout      time.Duration
}

// Response is the edited text.
type Response struct {
	Text       string                `json:"text"`
	ProviderID string                `json:"provider_id"`
	Model      string                `json:"model"`
	ElapsedMs  uint64                `json:"elapsed_ms"`
	Mode       string                `json:"mode"` // how the format was enforced
	Report     *store.ProviderReport `json:"report,omitempty"`
}

// ErrorKind is the kind of an enhancement error.
type ErrorKind int

const (
	ErrTimeout ErrorKind = iota
	ErrNotConfigured
	ErrHTTP
	ErrNetwork
	ErrInvalidOutput
	ErrRefused
	ErrCLI
)

// Error is an enhancement failure. Its text is the Rust Display text.
type Error struct {
	Kind    ErrorKind
	Ms      uint64 // Timeout
	Status  int    // HTTP
	Message string
}

func (e *Error) Error() string {
	switch e.Kind {
	case ErrTimeout:
		return fmt.Sprintf("timed out after %d ms", e.Ms)
	case ErrNotConfigured:
		return "the provider is not set up: " + e.Message
	case ErrHTTP:
		return fmt.Sprintf("HTTP %d: %s", e.Status, e.Message)
	case ErrNetwork:
		return "network error: " + e.Message
	case ErrInvalidOutput:
		return "the response did not match the schema: " + e.Message
	case ErrRefused:
		return "the model refused: " + e.Message
	}
	return e.Message
}

func notConfigured(msg string) *Error { return &Error{Kind: ErrNotConfigured, Message: msg} }
func invalidOutput(msg string) *Error { return &Error{Kind: ErrInvalidOutput, Message: msg} }

// IsKind is true when err is an *Error of kind k.
func IsKind(err error, k ErrorKind) bool {
	e, ok := err.(*Error)
	return ok && e.Kind == k
}

// ModelChoice is a model a provider offers.
type ModelChoice struct {
	ID          string   `json:"id"`
	Name        string   `json:"name"`
	Description *string  `json:"description"`
	Thinking    []string `json:"thinking"`
}

// Enhancer edits a transcript with a style prompt. Calls block.
type Enhancer interface {
	ID() string
	IsCloud() bool
	Enhance(Request) (Response, error)
}

// Generated is the reply of a language model.
type Generated struct{ Text, Model string }

// LanguageModel is a model that runs instructions on a prompt: Apple
// Intelligence through the engine.
type LanguageModel interface {
	ModelName() (string, error)
	Generate(instructions, prompt string, temperature *float32, timeout time.Duration) (Generated, error)
}

// OutputSchema is the JSON schema every provider enforces in its own way.
func OutputSchema() map[string]any {
	return map[string]any{
		"type":                 "object",
		"properties":           map[string]any{"text": map[string]any{"type": "string"}},
		"required":             []string{"text"},
		"additionalProperties": false,
	}
}

// UserMessage wraps the transcript as data.
func UserMessage(transcript string) string { return "<transcript>\n" + transcript + "\n</transcript>" }

// ParseOutput reads {"text": ...} from a reply, also inside a ``` fence.
func ParseOutput(raw string) (string, error) {
	trimmed := strings.TrimSpace(raw)
	body := trimmed
	for _, fence := range []string{"```json", "```"} {
		if rest, ok := strings.CutPrefix(trimmed, fence); ok {
			if inner, ok := strings.CutSuffix(rest, "```"); ok {
				body = inner
			}
			break
		}
	}
	body = strings.TrimSpace(body)
	var v any
	if err := json.Unmarshal([]byte(body), &v); err != nil {
		return "", invalidOutput("not JSON: " + err.Error())
	}
	return ParseValue(v)
}

// ParseValue reads the "text" field of a decoded reply.
func ParseValue(v any) (string, error) {
	obj, ok := v.(map[string]any)
	if !ok {
		return "", invalidOutput("not a JSON object")
	}
	text, ok := obj["text"].(string)
	if !ok {
		return "", invalidOutput(`missing string field "text"`)
	}
	text = unwrapText(text)
	if strings.TrimSpace(text) == "" {
		return "", invalidOutput(`"text" is empty`)
	}
	return text, nil
}

// unwrapText removes JSON or transcript tags that a model put in the field.
func unwrapText(text string) string {
	text = strings.TrimSpace(text)
	if strings.HasPrefix(text, "{") {
		var obj map[string]any
		if json.Unmarshal([]byte(text), &obj) == nil && len(obj) == 1 {
			if inner, ok := obj["text"].(string); ok {
				text = strings.TrimSpace(inner)
			}
		}
	}
	text = strings.TrimPrefix(text, "<transcript>")
	text = strings.TrimSuffix(text, "</transcript>")
	return strings.TrimSpace(text)
}

// EngineModel runs a style on the engine's language model (Apple Intelligence).
type EngineModel struct {
	id             string
	model          LanguageModel
	DefaultTimeout time.Duration
}

// NewEngineModel makes an enhancer on the engine; model nil when the engine does not run.
func NewEngineModel(id string, model LanguageModel) *EngineModel {
	return &EngineModel{id: id, model: model, DefaultTimeout: 4 * time.Second}
}

func (e *EngineModel) ID() string    { return e.id }
func (e *EngineModel) IsCloud() bool { return false }

func (e *EngineModel) Enhance(r Request) (Response, error) {
	if e.model == nil {
		return Response{}, notConfigured("the engine does not run")
	}
	timeout := r.Timeout
	if timeout == 0 {
		timeout = e.DefaultTimeout
	}
	started := time.Now()
	reply, err := e.model.Generate(r.SystemPrompt, UserMessage(r.Transcript), r.Temperature, timeout)
	if err != nil {
		return Response{}, err
	}
	text, err := ParseValue(map[string]any{"text": reply.Text})
	if err != nil {
		return Response{}, err
	}
	return Response{Text: text, ProviderID: e.id, Model: reply.Model, ElapsedMs: uint64(time.Since(started).Milliseconds()), Mode: "guided_generation"}, nil
}

// stubEnhancer is a provider kind that this build does not run.
type stubEnhancer struct {
	id, reason string
	cloud      bool
}

func (s stubEnhancer) ID() string    { return s.id }
func (s stubEnhancer) IsCloud() bool { return s.cloud }
func (s stubEnhancer) Enhance(Request) (Response, error) {
	return Response{}, notConfigured(s.reason)
}
