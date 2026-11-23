// Package cloud is the mobile core's opt-in cloud calls: speech-to-text
// (OpenAI Whisper, Groq, or any server with the OpenAI transcription API)
// and style enhancement (OpenAI, any OpenAI-compatible chat server, or
// Anthropic). Each call takes its whole configuration, API key included;
// the package keeps no key and never writes one to an error or a log.
package cloud

import (
	"bytes"
	"context"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"mime/multipart"
	"net/http"
	"net/url"
	"strings"
	"unicode/utf8"
)

// ASR providers; the values match CloudAsrProvider in the Android app.
const (
	ASROff = iota
	ASROpenAIWhisper
	ASRGroq
	ASROpenAICompatible
)

// Style providers; the values match CloudStyleProvider in the Android app.
const (
	StyleOff = iota
	StyleOpenAI
	StyleOpenAICompatible
	StyleAnthropic
)

// Config is one provider: which, its key, and optionally its server and
// model (empty: the provider's default; OpenAI-compatible needs BaseURL).
type Config struct {
	Provider int
	APIKey   string
	BaseURL  string
	Model    string
}

// Errors for a provider that is off and a call that ran out of time.
var (
	ErrOff     = errors.New("cloud is off")
	ErrTimeout = errors.New("timed out")
)

const (
	openAIBase    = "https://api.openai.com/v1"
	groqBase      = "https://api.groq.com/openai/v1"
	anthropicBase = "https://api.anthropic.com/v1"
	// SampleRate is the rate of the PCM Transcribe takes.
	SampleRate = 16000
	// MaxSamples keeps an upload under the 25 MB the providers accept
	// (about 13 minutes).
	MaxSamples = (25<<20 - 44) / 2
	maxReply   = 4 << 20
)

var client = &http.Client{Transport: http.DefaultTransport.(*http.Transport).Clone()}

// Transcribe sends 16 kHz mono PCM to the provider as a WAV file and
// returns the transcript. prompt, when set, is a hint of words to expect.
// ctx bounds the call.
func Transcribe(ctx context.Context, cfg Config, pcm []int16, prompt string) (string, error) {
	var base, model string
	needKey := true
	switch cfg.Provider {
	case ASROff:
		return "", ErrOff
	case ASROpenAIWhisper:
		base, model = openAIBase, "whisper-1"
	case ASRGroq:
		base, model = groqBase, "whisper-large-v3-turbo"
	case ASROpenAICompatible:
		model, needKey = "whisper-1", false
	default:
		return "", fmt.Errorf("unknown speech provider %d", cfg.Provider)
	}
	base, model, err := resolve(cfg, base, model, needKey)
	if err != nil {
		return "", err
	}
	if len(pcm) == 0 {
		return "", errors.New("no audio")
	}
	if len(pcm) > MaxSamples {
		return "", fmt.Errorf("the recording is longer than %d minutes", MaxSamples/SampleRate/60)
	}
	var body bytes.Buffer
	form := multipart.NewWriter(&body)
	form.WriteField("model", model)
	form.WriteField("response_format", "json")
	if prompt != "" {
		form.WriteField("prompt", prompt)
	}
	part, err := form.CreateFormFile("file", "audio.wav")
	if err != nil {
		return "", err
	}
	part.Write(WAV(pcm))
	form.Close()
	var reply struct {
		Text string `json:"text"`
	}
	headers := map[string]string{"Content-Type": form.FormDataContentType()}
	if cfg.APIKey != "" {
		headers["Authorization"] = "Bearer " + cfg.APIKey
	}
	if err := post(ctx, cfg.APIKey, base+"/audio/transcriptions", headers, &body, &reply); err != nil {
		return "", err
	}
	return strings.TrimSpace(reply.Text), nil
}

// Enhance sends a system prompt and a user message to the provider's chat
// model and returns its reply. ctx bounds the call.
func Enhance(ctx context.Context, cfg Config, system, user string) (string, error) {
	switch cfg.Provider {
	case StyleOff:
		return "", ErrOff
	case StyleOpenAI:
		return chat(ctx, cfg, openAIBase, "gpt-5-mini", true, system, user)
	case StyleOpenAICompatible:
		return chat(ctx, cfg, "", "", false, system, user)
	case StyleAnthropic:
		return anthropic(ctx, cfg, system, user)
	}
	return "", fmt.Errorf("unknown style provider %d", cfg.Provider)
}

func chat(ctx context.Context, cfg Config, base, model string, needKey bool, system, user string) (string, error) {
	base, model, err := resolve(cfg, base, model, needKey)
	if err != nil {
		return "", err
	}
	req := map[string]any{
		"model": model,
		"messages": []map[string]string{
			{"role": "system", "content": system},
			{"role": "user", "content": user},
		},
		"stream": false,
	}
	headers := map[string]string{"Content-Type": "application/json"}
	if cfg.APIKey != "" {
		headers["Authorization"] = "Bearer " + cfg.APIKey
	}
	var reply struct {
		Choices []struct {
			Message struct {
				Content string `json:"content"`
			} `json:"message"`
		} `json:"choices"`
	}
	if err := postJSON(ctx, cfg.APIKey, base+"/chat/completions", headers, req, &reply); err != nil {
		return "", err
	}
	if len(reply.Choices) == 0 {
		return "", errors.New("the reply has no text")
	}
	return reply.Choices[0].Message.Content, nil
}

func anthropic(ctx context.Context, cfg Config, system, user string) (string, error) {
	base, model, err := resolve(cfg, anthropicBase, "claude-haiku-4-5", true)
	if err != nil {
		return "", err
	}
	req := map[string]any{
		"model":      model,
		"max_tokens": min(8192, 256+utf8.RuneCountInString(user)),
		"system":     system,
		"messages":   []map[string]string{{"role": "user", "content": user}},
	}
	headers := map[string]string{"Content-Type": "application/json", "anthropic-version": "2023-06-01", "x-api-key": cfg.APIKey}
	var reply struct {
		Content []struct {
			Type string `json:"type"`
			Text string `json:"text"`
		} `json:"content"`
	}
	if err := postJSON(ctx, cfg.APIKey, base+"/messages", headers, req, &reply); err != nil {
		return "", err
	}
	var b strings.Builder
	for _, c := range reply.Content {
		if c.Type == "text" {
			b.WriteString(c.Text)
		}
	}
	if b.Len() == 0 {
		return "", errors.New("the reply has no text")
	}
	return b.String(), nil
}

// resolve fills in the provider's default server and model and checks
// that the call can be made. OpenAI-compatible servers may need no key.
func resolve(cfg Config, base, model string, needKey bool) (string, string, error) {
	if b := strings.TrimSpace(cfg.BaseURL); b != "" {
		base = b
	}
	if m := strings.TrimSpace(cfg.Model); m != "" {
		model = m
	}
	base = strings.TrimRight(base, "/")
	if base == "" {
		return "", "", errors.New("no server address is set")
	}
	if u, err := url.Parse(base); err != nil || (u.Scheme != "https" && u.Scheme != "http") || u.Host == "" {
		return "", "", errors.New("the server address must start with https:// or http://")
	}
	if model == "" {
		return "", "", errors.New("no model is set")
	}
	if needKey && cfg.APIKey == "" {
		return "", "", errors.New("no API key is set")
	}
	return base, model, nil
}

func postJSON(ctx context.Context, key, url string, headers map[string]string, req, reply any) error {
	data, err := json.Marshal(req)
	if err != nil {
		return err
	}
	return post(ctx, key, url, headers, bytes.NewReader(data), reply)
}

// post sends body and decodes a JSON reply. Errors never hold the key.
func post(ctx context.Context, key, url string, headers map[string]string, body io.Reader, reply any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, body)
	if err != nil {
		return redact(err, key)
	}
	req.Header.Set("User-Agent", "Sayso")
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	resp, err := client.Do(req)
	if err != nil {
		if ctx.Err() == context.DeadlineExceeded {
			return ErrTimeout
		}
		return redact(err, key)
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(io.LimitReader(resp.Body, maxReply))
	if err != nil {
		if ctx.Err() == context.DeadlineExceeded {
			return ErrTimeout
		}
		return redact(err, key)
	}
	if resp.StatusCode/100 != 2 {
		return redact(fmt.Errorf("%s: %s", resp.Status, serverMessage(data)), key)
	}
	if err := json.Unmarshal(data, reply); err != nil {
		return errors.New("the reply is not valid JSON")
	}
	return nil
}

// serverMessage is the error message in a provider's reply, or the start
// of the reply.
func serverMessage(data []byte) string {
	var e struct {
		Error any `json:"error"`
	}
	if json.Unmarshal(data, &e) == nil {
		switch v := e.Error.(type) {
		case string:
			return v
		case map[string]any:
			if m, ok := v["message"].(string); ok {
				return m
			}
		}
	}
	s := strings.TrimSpace(string(data))
	if len(s) > 200 {
		s = s[:200]
	}
	return s
}

// redact removes the key from an error's text.
func redact(err error, key string) error {
	if key == "" || !strings.Contains(err.Error(), key) {
		return err
	}
	return errors.New(strings.ReplaceAll(err.Error(), key, "[key]"))
}

// WAV wraps 16 kHz mono 16-bit PCM in a WAV header.
func WAV(pcm []int16) []byte {
	n := 2 * len(pcm)
	b := make([]byte, 44+n)
	copy(b, "RIFF")
	binary.LittleEndian.PutUint32(b[4:], uint32(36+n))
	copy(b[8:], "WAVEfmt ")
	binary.LittleEndian.PutUint32(b[16:], 16)
	binary.LittleEndian.PutUint16(b[20:], 1) // PCM
	binary.LittleEndian.PutUint16(b[22:], 1) // mono
	binary.LittleEndian.PutUint32(b[24:], SampleRate)
	binary.LittleEndian.PutUint32(b[28:], SampleRate*2)
	binary.LittleEndian.PutUint16(b[32:], 2)
	binary.LittleEndian.PutUint16(b[34:], 16)
	copy(b[36:], "data")
	binary.LittleEndian.PutUint32(b[40:], uint32(n))
	for i, s := range pcm {
		binary.LittleEndian.PutUint16(b[44+2*i:], uint16(s))
	}
	return b
}
