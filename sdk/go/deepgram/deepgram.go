// Package deepgram runs the final pass on Deepgram: a small client of its
// HTTP API, and an engine that sends the final pass of Model there and
// everything else to the engine under it.
//
// Sayso uses cloud models for the final role only. The audio of a whole part
// goes to Deepgram when the part ends; the live preview and the voice
// detection stay on this computer. The key comes from the user. This package
// does not store it and never writes it in a log.
package deepgram

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/watzon/sayso/sdk/go/audio"
	"github.com/watzon/sayso/sdk/go/protocol"
	"github.com/watzon/sayso/sdk/go/speech"
)

// Model is the model id of the final pass with Deepgram.
const Model = "deepgram-nova-3"

const (
	apiModel = "nova-3"
	baseURL  = "https://api.deepgram.com/v1"
	// timeout is the whole request; a part is a few seconds of speech.
	timeout        = 30 * time.Second
	connectTimeout = 10 * time.Second
)

// The codes of a speech.EngineError of Deepgram.
const (
	// CodeConnection: Deepgram cannot be reached, or asks to try later.
	CodeConnection protocol.ErrorCode = "cloud_connection"
	// CodeKey: Deepgram refuses the key.
	CodeKey protocol.ErrorCode = "cloud_key"
)

// Client is the HTTP client. It keeps the key in memory only.
type Client struct {
	key     string
	baseURL string
	http    *http.Client
}

// New is a client of Deepgram with the key of the user.
func New(key string) *Client { return WithBaseURL(key, baseURL) }

// WithBaseURL is a client of another server, for tests.
func WithBaseURL(key, base string) *Client {
	t := http.DefaultTransport.(*http.Transport).Clone()
	t.DialContext = (&net.Dialer{Timeout: connectTimeout}).DialContext
	return &Client{key: key, baseURL: base, http: &http.Client{Timeout: timeout, Transport: t}}
}

// String leaves the key out.
func (c *Client) String() string { return fmt.Sprintf("deepgram.Client{%s}", c.baseURL) }

// GoString leaves the key out of %#v.
func (c *Client) GoString() string { return c.String() }

func connectionError(err error) error {
	return &speech.EngineError{Code: CodeConnection, Message: "cannot reach Deepgram: " + err.Error()}
}

func checkStatus(status int, body string) error {
	switch {
	case status >= 200 && status <= 299:
		return nil
	case status == 401 || status == 403:
		return &speech.EngineError{Code: CodeKey, Message: fmt.Sprintf("Deepgram refused the key (HTTP %d)", status)}
	case status == 408 || status == 429 || (status >= 500 && status <= 599):
		// A server that fails or says "too many" is as good as no
		// connection: a retry can work.
		return &speech.EngineError{Code: CodeConnection, Message: fmt.Sprintf("Deepgram answered HTTP %d", status)}
	}
	if len(body) > 200 {
		cut := 200
		for cut > 0 && !utf8.RuneStart(body[cut]) {
			cut--
		}
		body = body[:cut]
	}
	return &speech.EngineError{Message: fmt.Sprintf("Deepgram answered HTTP %d: %s", status, body)}
}

func (c *Client) do(req *http.Request) (int, string, error) {
	req.Header.Set("Authorization", "Token "+c.key)
	resp, err := c.http.Do(req)
	if err != nil {
		return 0, "", connectionError(err)
	}
	defer resp.Body.Close()
	b, err := io.ReadAll(resp.Body)
	if err != nil {
		return 0, "", connectionError(err)
	}
	return resp.StatusCode, string(b), nil
}

// CheckKey asks Deepgram if it knows the key.
func (c *Client) CheckKey() error {
	req, err := http.NewRequest(http.MethodGet, c.baseURL+"/projects", nil)
	if err != nil {
		return err
	}
	status, body, err := c.do(req)
	if err != nil {
		return err
	}
	return checkStatus(status, body)
}

func (c *Client) listenURL(language string) string {
	url := c.baseURL + "/listen?model=" + apiModel + "&smart_format=true&punctuate=true"
	if language == "" || language == "auto" {
		return url + "&detect_language=true"
	}
	var b strings.Builder
	for _, r := range language {
		if r < 128 && (r == '-' || r >= '0' && r <= '9' || r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z') {
			b.WriteRune(r)
		}
	}
	return url + "&language=" + b.String()
}

// Transcribe is the text of samples (16 kHz mono). language is a code such
// as "en", or "auto".
func (c *Client) Transcribe(samples []float32, language string) (string, error) {
	wav := audio.EncodeWAV(audio.FloatsToI16(samples))
	req, err := http.NewRequest(http.MethodPost, c.listenURL(language), bytes.NewReader(wav))
	if err != nil {
		return "", err
	}
	req.Header.Set("Content-Type", "audio/wav")
	status, body, err := c.do(req)
	if err != nil {
		return "", err
	}
	if err := checkStatus(status, body); err != nil {
		return "", err
	}
	var reply struct {
		Results *struct {
			Channels []struct {
				Alternatives []struct {
					Transcript *string `json:"transcript"`
				} `json:"alternatives"`
			} `json:"channels"`
		} `json:"results"`
	}
	if err := json.Unmarshal([]byte(body), &reply); err != nil {
		return "", &speech.EngineError{Message: "Deepgram sent a reply that is not JSON: " + err.Error()}
	}
	if reply.Results == nil || len(reply.Results.Channels) == 0 || len(reply.Results.Channels[0].Alternatives) == 0 || reply.Results.Channels[0].Alternatives[0].Transcript == nil {
		return "", &speech.EngineError{Message: "Deepgram sent a reply with no transcript"}
	}
	return strings.TrimSpace(*reply.Results.Channels[0].Alternatives[0].Transcript), nil
}

// Final is an engine that sends the final pass of Model to Deepgram, and
// everything else to Local.
type Final struct {
	speech.Engine
	Client *Client
}

// NewFinal wraps local.
func NewFinal(local speech.Engine, client *Client) *Final { return &Final{Engine: local, Client: client} }

func (f *Final) Status(model string) speech.ModelStatus {
	if model == Model {
		return speech.ModelStatus{State: speech.StateReady}
	}
	return f.Engine.Status(model)
}

func (f *Final) Load(model string) error {
	if model == Model {
		return nil
	}
	return f.Engine.Load(model)
}

func (f *Final) Transcribe(samples []float32, model, language string) (string, error) {
	if model == Model {
		return f.Client.Transcribe(samples, language)
	}
	return f.Engine.Transcribe(samples, model, language)
}

func (f *Final) TranscribeSegments(samples []float32, model, language string) ([]speech.Segment, error) {
	if model != Model {
		return f.Engine.TranscribeSegments(samples, model, language)
	}
	text, err := f.Client.Transcribe(samples, language)
	if err != nil {
		return nil, err
	}
	return []speech.Segment{{StartMs: 0, EndMs: uint64(len(samples)) * 1000 / speech.SampleRate, Text: text}}, nil
}
