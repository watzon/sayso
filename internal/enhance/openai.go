package enhance

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net"
	"net/http"
	"net/url"
	"os"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/watzon/sayso/internal/config"
)

// OpenAI-compatible chat completions (sayso-enhance/src/openai.rs): OpenAI,
// OpenRouter, Groq, Anthropic and Gemini through their OpenAI endpoints,
// Ollama, LM Studio, and llama.cpp server. The output format is enforced with
// the strongest mode the server takes: json_schema, then json_object, then a
// forced tool call. The mode that worked is remembered per server and model.

const (
	schemaName         = "sayso_text"
	toolName           = "return_text"
	defaultTemperature = float32(0.2)
	thinkingTokens     = 4096
)

// Mode is how the output format is enforced.
type Mode int

const (
	ModeJSONSchema Mode = iota
	ModeJSONObject
	ModeToolCall
)

var modeLadder = []Mode{ModeJSONSchema, ModeJSONObject, ModeToolCall}

func (m Mode) String() string { return [...]string{"json_schema", "json_object", "tool_call"}[m] }

type modeKey struct{ base, model string }

var (
	modeMu    sync.Mutex
	modeCache = map[modeKey]Mode{}
)

// ClearModeCache forgets which mode each server took.
func ClearModeCache() {
	modeMu.Lock()
	modeCache = map[modeKey]Mode{}
	modeMu.Unlock()
}

// Two long-lived clients: local servers bypass the system proxy.
var (
	proxiedHTTP = &http.Client{Transport: newHTTPTransport(http.ProxyFromEnvironment)}
	directHTTP  = &http.Client{Transport: newHTTPTransport(nil)}
)

func newHTTPTransport(proxy func(*http.Request) (*url.URL, error)) *http.Transport {
	t := http.DefaultTransport.(*http.Transport).Clone()
	t.Proxy = proxy
	return t
}

func httpClientFor(base string) *http.Client {
	if config.IsLocalURL(base) {
		return directHTTP
	}
	return proxiedHTTP
}

func mapTransport(err error, timeoutMs uint64) *Error {
	var ne net.Error
	if errors.Is(err, context.DeadlineExceeded) || errors.Is(err, os.ErrDeadlineExceeded) || (errors.As(err, &ne) && ne.Timeout()) {
		return &Error{Kind: ErrTimeout, Ms: timeoutMs}
	}
	return &Error{Kind: ErrNetwork, Message: err.Error()}
}

func firstRunes(s string, n int) string {
	if utf8.RuneCountInString(s) <= n {
		return s
	}
	return string([]rune(s)[:n])
}

// postJSON sends body and returns the status and the reply text.
func postJSON(client *http.Client, url string, headers map[string]string, body any, remaining time.Duration, timeoutMs uint64) (int, string, error) {
	data, err := json.Marshal(body)
	if err != nil {
		return 0, "", invalidOutput(err.Error())
	}
	ctx, cancel := context.WithTimeout(context.Background(), remaining)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(data))
	if err != nil {
		return 0, "", &Error{Kind: ErrNetwork, Message: err.Error()}
	}
	req.Header.Set("User-Agent", "Sayso")
	req.Header.Set("Content-Type", "application/json")
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	resp, err := client.Do(req)
	if err != nil {
		return 0, "", mapTransport(err, timeoutMs)
	}
	defer resp.Body.Close()
	text, err := io.ReadAll(resp.Body)
	if err != nil {
		return 0, "", mapTransport(err, timeoutMs)
	}
	return resp.StatusCode, string(text), nil
}

// OpenAICompatible is a chat completions server.
type OpenAICompatible struct {
	id, baseURL, model string
	apiKey             string
	setupError         string
	ZeroDataRetention  bool
	cloud              bool
	openrouter         *bool
	thinking           string
	DefaultTimeout     time.Duration
	client             *http.Client
}

// NewOpenAICompatible makes an enhancer for a server at baseURL.
func NewOpenAICompatible(id, baseURL, model string) *OpenAICompatible {
	base := strings.TrimRight(baseURL, "/")
	return &OpenAICompatible{id: id, baseURL: base, model: model, cloud: !config.IsLocalURL(base), DefaultTimeout: 4 * time.Second, client: httpClientFor(base)}
}

func (o *OpenAICompatible) WithAPIKey(key string) *OpenAICompatible   { o.apiKey = key; return o }
func (o *OpenAICompatible) WithSetupError(r string) *OpenAICompatible { o.setupError = r; return o }
func (o *OpenAICompatible) WithThinking(l string) *OpenAICompatible   { o.thinking = l; return o }
func (o *OpenAICompatible) WithCloud(c bool) *OpenAICompatible        { o.cloud = c; return o }
func (o *OpenAICompatible) WithOpenRouter(on bool) *OpenAICompatible  { o.openrouter = &on; return o }

func (o *OpenAICompatible) ID() string    { return o.id }
func (o *OpenAICompatible) IsCloud() bool { return o.cloud }

func (o *OpenAICompatible) isOpenRouter() bool {
	if o.openrouter != nil {
		return *o.openrouter
	}
	rest := o.baseURL
	if _, after, ok := strings.Cut(rest, "://"); ok {
		rest = after
	}
	host := rest
	if i := strings.IndexAny(rest, "/:"); i >= 0 {
		host = rest[:i]
	}
	return host == "openrouter.ai" || strings.HasSuffix(host, ".openrouter.ai")
}

func maxTokens(r Request) int {
	n := utf8.RuneCountInString(r.Transcript)/2 + 128
	return min(max(n, 256), 8192)
}

// BuildBody is the request body for a mode.
func (o *OpenAICompatible) BuildBody(mode Mode, model string, r Request) map[string]any {
	schema := OutputSchema()
	system := r.SystemPrompt
	if mode == ModeJSONObject {
		s, _ := json.Marshal(schema)
		system += "\n\nReply with only a JSON object that matches this JSON schema:\n" + string(s)
	}
	temp := defaultTemperature
	if r.Temperature != nil {
		temp = *r.Temperature
	}
	body := map[string]any{
		"model": model,
		"messages": []map[string]any{
			{"role": "system", "content": system},
			{"role": "user", "content": UserMessage(r.Transcript)},
		},
		"temperature": temp,
		"max_tokens":  maxTokens(r),
		"stream":      false,
	}
	if o.thinking != "" {
		if o.isOpenRouter() {
			body["reasoning"] = map[string]any{"effort": o.thinking}
		} else {
			body["reasoning_effort"] = o.thinking
		}
		if o.thinking != "none" {
			body["max_tokens"] = maxTokens(r) + thinkingTokens
		}
	}
	switch mode {
	case ModeJSONSchema:
		body["response_format"] = map[string]any{"type": "json_schema", "json_schema": map[string]any{"name": schemaName, "strict": true, "schema": schema}}
	case ModeJSONObject:
		body["response_format"] = map[string]any{"type": "json_object"}
	case ModeToolCall:
		body["tools"] = []any{map[string]any{"type": "function", "function": map[string]any{"name": toolName, "description": "Return the final text.", "parameters": schema}}}
		body["tool_choice"] = map[string]any{"type": "function", "function": map[string]any{"name": toolName}}
	}
	if o.isOpenRouter() {
		routing := map[string]any{}
		if mode == ModeJSONSchema || mode == ModeToolCall {
			routing["require_parameters"] = true
		}
		if o.ZeroDataRetention {
			routing["zdr"] = true
			routing["data_collection"] = "deny"
		}
		if len(routing) > 0 {
			body["provider"] = routing
		}
	}
	return body
}

func (o *OpenAICompatible) attempt(mode Mode, model string, r Request, remaining time.Duration, timeoutMs uint64) (string, error) {
	headers := map[string]string{}
	if o.apiKey != "" {
		headers["Authorization"] = "Bearer " + o.apiKey
	}
	status, text, err := postJSON(o.client, o.baseURL+"/chat/completions", headers, o.BuildBody(mode, model, r), remaining, timeoutMs)
	if err != nil {
		return "", err
	}
	if status >= 400 {
		return "", &Error{Kind: ErrHTTP, Status: status, Message: firstRunes(text, 500)}
	}
	var reply any
	if err := json.Unmarshal([]byte(text), &reply); err != nil {
		return "", invalidOutput("the server reply is not JSON: " + err.Error())
	}
	return ParseReply(mode, reply)
}

func triesNextMode(err error) bool {
	e, ok := err.(*Error)
	return ok && (e.Kind == ErrHTTP && (e.Status == 400 || e.Status == 422) || e.Kind == ErrInvalidOutput)
}

// ParseReply reads the text out of a chat completion.
func ParseReply(mode Mode, reply any) (string, error) {
	obj, _ := reply.(map[string]any)
	choices, hasChoices := obj["choices"].([]any)
	if _, present := obj["choices"]; !present || (hasChoices && len(choices) == 0) {
		if e, ok := obj["error"]; ok {
			status := 502
			if m, ok := e.(map[string]any); ok {
				if c, ok := m["code"].(float64); ok && c >= 400 && c < 600 && c == float64(int(c)) {
					status = int(c)
				}
			}
			b, _ := json.Marshal(e)
			return "", &Error{Kind: ErrHTTP, Status: status, Message: firstRunes(string(b), 500)}
		}
	}
	if len(choices) == 0 {
		return "", invalidOutput("the reply has no choices")
	}
	choice, _ := choices[0].(map[string]any)
	message, _ := choice["message"].(map[string]any)
	if refusal, ok := message["refusal"].(string); ok && strings.TrimSpace(refusal) != "" {
		return "", &Error{Kind: ErrRefused, Message: refusal}
	}
	if choice["finish_reason"] == "length" {
		return "", invalidOutput("the reply was cut off by the token limit")
	}
	if mode == ModeToolCall {
		if calls, ok := message["tool_calls"].([]any); ok && len(calls) > 0 {
			if args, ok := pointerStr(calls[0], "function", "arguments"); ok {
				return ParseOutput(args)
			}
		}
	}
	content := ""
	switch c := message["content"].(type) {
	case string:
		content = c
	case []any:
		for _, p := range c {
			if m, ok := p.(map[string]any); ok {
				if s, ok := m["text"].(string); ok {
					content += s
				}
			}
		}
	}
	if strings.TrimSpace(content) == "" {
		return "", invalidOutput("the reply has no content")
	}
	return ParseOutput(content)
}

func pointerStr(v any, keys ...string) (string, bool) {
	for _, k := range keys {
		m, ok := v.(map[string]any)
		if !ok {
			return "", false
		}
		v = m[k]
	}
	s, ok := v.(string)
	return s, ok
}

func (o *OpenAICompatible) Enhance(r Request) (Response, error) {
	if o.setupError != "" {
		return Response{}, notConfigured(o.setupError)
	}
	started := time.Now()
	timeout := r.Timeout
	if timeout == 0 {
		timeout = o.DefaultTimeout
	}
	timeoutMs := uint64(timeout.Milliseconds())
	deadline := started.Add(timeout)
	model := r.Model
	if model == "" {
		model = o.model
	}
	key := modeKey{o.baseURL, model}
	modeMu.Lock()
	cached, has := modeCache[key]
	modeMu.Unlock()
	first := 0
	if has {
		first = int(cached)
	}
	var lastErr error
	for _, mode := range modeLadder[first:] {
		remaining := time.Until(deadline)
		if remaining <= 0 {
			return Response{}, &Error{Kind: ErrTimeout, Ms: timeoutMs}
		}
		text, err := o.attempt(mode, model, r, remaining, timeoutMs)
		if err == nil {
			modeMu.Lock()
			modeCache[key] = mode
			modeMu.Unlock()
			return Response{Text: text, ProviderID: o.id, Model: model, ElapsedMs: uint64(time.Since(started).Milliseconds()), Mode: mode.String()}, nil
		}
		if !triesNextMode(err) {
			return Response{}, err
		}
		slog.Info("enhance: mode failed, trying the next mode", "provider", o.id, "mode", mode.String(), "error", err)
		lastErr = err
	}
	modeMu.Lock()
	delete(modeCache, key)
	modeMu.Unlock()
	if lastErr == nil {
		lastErr = invalidOutput("no mode worked")
	}
	return Response{}, lastErr
}
