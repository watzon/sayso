package providers

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
	"time"

	"github.com/watzon/sayso/internal/config"
)

// defaultTimeout is the deadline of a call with a zero timeout.
const defaultTimeout = 30 * time.Second

// bodyPreviewChars is the longest part of an error body put in a message.
const bodyPreviewChars = 200

type deadline struct {
	started time.Time
	timeout time.Duration
}

func newDeadline(timeout time.Duration) *deadline {
	if timeout <= 0 {
		timeout = defaultTimeout
	}
	return &deadline{started: time.Now(), timeout: timeout}
}

func (d *deadline) timeoutMs() uint64 { return uint64(d.timeout.Milliseconds()) }
func (d *deadline) elapsedMs() int64  { return time.Since(d.started).Milliseconds() }

func (d *deadline) remaining() (time.Duration, error) {
	left := d.timeout - time.Since(d.started)
	if left <= 0 {
		return 0, &config.SpeechError{Kind: config.SpeechTimeout, Ms: d.timeoutMs()}
	}
	return left, nil
}

// target is where one call goes.
type target struct {
	id     string
	kind   config.SpeechProviderKind
	model  string
	base   string
	key    string // trimmed; "" for a local server without a key
	client *http.Client
}

func newTarget(p config.SpeechProvider, apiKey, model string) (*target, error) {
	base := p.BaseURLResolved()
	if base == "" {
		return nil, &config.SpeechError{Kind: config.SpeechNotConfigured, Missing: config.MissingAddress}
	}
	key := strings.TrimSpace(apiKey)
	if key == "" && p.IsCloud() {
		return nil, &config.SpeechError{Kind: config.SpeechNotConfigured, Missing: config.MissingAPIKey}
	}
	return &target{id: p.ID, kind: p.Kind, model: model, base: base, key: key, client: clientFor(base)}, nil
}

func (t *target) url(path string) string { return t.base + path }

type header struct{ name, value string }

func (t *target) get(url string, headers []header, d *deadline) (*reply, error) {
	return t.do(http.MethodGet, url, headers, "", nil, d)
}

func (t *target) post(url string, headers []header, contentType string, body []byte, d *deadline) (*reply, error) {
	return t.do(http.MethodPost, url, headers, contentType, body, d)
}

func (t *target) do(method, url string, headers []header, contentType string, body []byte, d *deadline) (*reply, error) {
	left, err := d.remaining()
	if err != nil {
		return nil, err
	}
	ctx, cancel := context.WithTimeout(context.Background(), left)
	defer cancel()
	var rd io.Reader
	if body != nil {
		rd = bytes.NewReader(body)
	}
	req, err := http.NewRequestWithContext(ctx, method, url, rd)
	if err != nil {
		return nil, &config.SpeechError{Kind: config.SpeechNetwork, Message: err.Error()}
	}
	req.Header.Set("User-Agent", "Sayso")
	for _, h := range headers {
		req.Header.Set(h.name, h.value)
	}
	if contentType != "" {
		req.Header.Set("Content-Type", contentType)
	}
	started := time.Now()
	resp, err := t.client.Do(req)
	if err != nil {
		return nil, mapTransportError(err, d.timeoutMs())
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, mapTransportError(err, d.timeoutMs())
	}
	// Never log the key or the audio.
	slog.Debug("speech: answer", "provider", t.id, "model", t.model, "status", resp.StatusCode, "ms", time.Since(started).Milliseconds())
	return &reply{status: resp.StatusCode, body: string(data)}, nil
}

func mapTransportError(err error, timeoutMs uint64) error {
	var ne net.Error
	if errors.Is(err, context.DeadlineExceeded) || errors.Is(err, os.ErrDeadlineExceeded) || (errors.As(err, &ne) && ne.Timeout()) {
		return &config.SpeechError{Kind: config.SpeechTimeout, Ms: timeoutMs}
	}
	return &config.SpeechError{Kind: config.SpeechNetwork, Message: err.Error()}
}

// Two long-lived clients so a dictation reuses the connection of the one
// before: local servers bypass the system proxy, all others use it.
var (
	proxiedClient = &http.Client{Transport: newTransport(http.ProxyFromEnvironment)}
	directClient  = &http.Client{Transport: newTransport(nil)}
)

func newTransport(proxy func(*http.Request) (*url.URL, error)) *http.Transport {
	tr := http.DefaultTransport.(*http.Transport).Clone()
	tr.Proxy = proxy
	return tr
}

func clientFor(base string) *http.Client {
	if config.IsLocalURL(base) {
		return directClient
	}
	return proxiedClient
}

// reply is an answer with any status code.
type reply struct {
	status int
	body   string
}

func (r *reply) isSuccess() bool { return r.status >= 200 && r.status < 300 }

func (r *reply) json() (any, bool) {
	var v any
	if json.Unmarshal([]byte(r.body), &v) != nil {
		return nil, false
	}
	return v, true
}

// errorWith is an Http error; message reads the provider's text out of the
// JSON body, falling back to the start of the body.
func (r *reply) errorWith(message func(any) string) error {
	msg := ""
	if v, ok := r.json(); ok {
		msg = strings.TrimSpace(message(v))
	}
	if msg == "" {
		msg = firstChars(strings.TrimSpace(r.body), bodyPreviewChars)
	}
	return &config.SpeechError{Kind: config.SpeechHTTP, Status: r.status, Message: msg}
}

func (r *reply) intoJSON(message func(any) string) (any, error) {
	if !r.isSuccess() {
		return nil, r.errorWith(message)
	}
	var v any
	if err := json.Unmarshal([]byte(r.body), &v); err != nil {
		return nil, &config.SpeechError{Kind: config.SpeechInvalidResponse, Message: "the body is not JSON: " + err.Error()}
	}
	return v, nil
}

func firstChars(s string, n int) string {
	i := 0
	for pos := range s {
		if i == n {
			return s[:pos]
		}
		i++
	}
	return s
}

// pointer reads a JSON pointer such as "/results/channels/0/alternatives/0/transcript".
func pointer(v any, ptr string) any {
	if ptr == "" {
		return v
	}
	for _, part := range strings.Split(strings.TrimPrefix(ptr, "/"), "/") {
		switch node := v.(type) {
		case map[string]any:
			v = node[part]
		case []any:
			i, ok := atoi(part)
			if !ok || i >= len(node) {
				return nil
			}
			v = node[i]
		default:
			return nil
		}
	}
	return v
}

func atoi(s string) (int, bool) {
	if s == "" {
		return 0, false
	}
	n := 0
	for _, c := range s {
		if c < '0' || c > '9' {
			return 0, false
		}
		n = n*10 + int(c-'0')
	}
	return n, true
}

func str(v any, ptr string) (string, bool) {
	s, ok := pointer(v, ptr).(string)
	return s, ok
}

// textAt is the string at a JSON pointer, or InvalidResponse. "" is a valid
// transcript: the recording had no speech.
func textAt(v any, ptr string) (string, error) {
	if s, ok := str(v, ptr); ok {
		return s, nil
	}
	return "", &config.SpeechError{Kind: config.SpeechInvalidResponse, Message: "the body has no string at " + ptr}
}

func hintLanguage(r Request) string { return strings.TrimSpace(r.Language) }

func hintWords(r Request) []string {
	var out []string
	for _, w := range r.Vocabulary {
		if w = strings.TrimSpace(w); w != "" {
			out = append(out, w)
		}
	}
	return out
}
