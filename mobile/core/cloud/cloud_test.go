package cloud

import (
	"context"
	"encoding/binary"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

// All tests talk to httptest servers; none reaches a real provider.

func ctxFor(t *testing.T, d time.Duration) context.Context {
	ctx, cancel := context.WithTimeout(context.Background(), d)
	t.Cleanup(cancel)
	return ctx
}

func TestTranscribe(t *testing.T) {
	pcm := []int16{0, 1000, -1000, 32767, -32768}
	var gotModel, gotPrompt, gotAuth string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/audio/transcriptions" || r.Method != http.MethodPost {
			t.Errorf("%s %s", r.Method, r.URL.Path)
		}
		gotAuth = r.Header.Get("Authorization")
		if err := r.ParseMultipartForm(1 << 20); err != nil {
			t.Fatal(err)
		}
		gotModel, gotPrompt = r.FormValue("model"), r.FormValue("prompt")
		f, h, err := r.FormFile("file")
		if err != nil {
			t.Fatal(err)
		}
		wav, _ := io.ReadAll(f)
		if h.Filename != "audio.wav" || string(wav[:4]) != "RIFF" || string(wav[8:12]) != "WAVE" ||
			binary.LittleEndian.Uint32(wav[24:]) != SampleRate || len(wav) != 44+2*len(pcm) {
			t.Errorf("bad WAV: %q, %d bytes", h.Filename, len(wav))
		}
		for i, s := range pcm {
			if int16(binary.LittleEndian.Uint16(wav[44+2*i:])) != s {
				t.Errorf("sample %d", i)
			}
		}
		w.Write([]byte(`{"text":"  hello world  "}`))
	}))
	defer srv.Close()

	cases := []struct {
		cfg       Config
		model     string
		wantAuth  string
		wantError string
	}{
		{Config{Provider: ASROpenAIWhisper, APIKey: "sk-1", BaseURL: srv.URL + "/v1"}, "whisper-1", "Bearer sk-1", ""},
		{Config{Provider: ASRGroq, APIKey: "gsk", BaseURL: srv.URL + "/v1/"}, "whisper-large-v3-turbo", "Bearer gsk", ""},
		{Config{Provider: ASROpenAICompatible, BaseURL: srv.URL + "/v1", Model: "my-whisper"}, "my-whisper", "", ""},
		{Config{Provider: ASROpenAIWhisper, BaseURL: srv.URL + "/v1"}, "", "", "no API key"},
		{Config{Provider: ASROpenAICompatible}, "", "", "no server address"},
		{Config{Provider: ASROpenAICompatible, BaseURL: "ftp://x"}, "", "", "must start with https://"},
	}
	for _, c := range cases {
		gotModel, gotAuth = "", ""
		text, err := Transcribe(ctxFor(t, 5*time.Second), c.cfg, pcm, "Sayso, GitHub")
		if c.wantError != "" {
			if err == nil || !strings.Contains(err.Error(), c.wantError) {
				t.Errorf("%+v: error %v, want %q", c.cfg, err, c.wantError)
			}
			continue
		}
		if err != nil || text != "hello world" || gotModel != c.model || gotAuth != c.wantAuth || gotPrompt != "Sayso, GitHub" {
			t.Errorf("%+v: %q %v model %q auth %q prompt %q", c.cfg, text, err, gotModel, gotAuth, gotPrompt)
		}
	}
	if _, err := Transcribe(ctxFor(t, time.Second), Config{}, pcm, ""); err != ErrOff {
		t.Errorf("off: %v", err)
	}
	if _, err := Transcribe(ctxFor(t, time.Second), Config{Provider: ASRGroq, APIKey: "k", BaseURL: srv.URL}, nil, ""); err == nil {
		t.Error("no audio sent anyway")
	}
}

func TestEnhanceOpenAI(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Model    string `json:"model"`
			Messages []struct{ Role, Content string }
		}
		json.NewDecoder(r.Body).Decode(&req)
		if r.URL.Path != "/chat/completions" || req.Model != "gpt-test" || len(req.Messages) != 2 ||
			req.Messages[0].Role != "system" || req.Messages[0].Content != "sys" || req.Messages[1].Content != "user text" {
			t.Errorf("%s %+v", r.URL.Path, req)
		}
		if r.Header.Get("Authorization") != "Bearer sk-2" {
			t.Errorf("auth %q", r.Header.Get("Authorization"))
		}
		w.Write([]byte(`{"choices":[{"message":{"role":"assistant","content":"Edited."}}]}`))
	}))
	defer srv.Close()
	for _, p := range []int{StyleOpenAI, StyleOpenAICompatible} {
		got, err := Enhance(ctxFor(t, 5*time.Second), Config{Provider: p, APIKey: "sk-2", BaseURL: srv.URL, Model: "gpt-test"}, "sys", "user text")
		if err != nil || got != "Edited." {
			t.Errorf("provider %d: %q %v", p, got, err)
		}
	}
}

func TestEnhanceAnthropic(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Model     string `json:"model"`
			System    string `json:"system"`
			MaxTokens int    `json:"max_tokens"`
			Messages  []struct{ Role, Content string }
		}
		json.NewDecoder(r.Body).Decode(&req)
		if r.URL.Path != "/messages" || req.Model != "claude-haiku-4-5" || req.System != "sys" || req.MaxTokens <= 0 ||
			len(req.Messages) != 1 || req.Messages[0].Content != "user text" {
			t.Errorf("%s %+v", r.URL.Path, req)
		}
		if r.Header.Get("x-api-key") != "ak" || r.Header.Get("anthropic-version") == "" || r.Header.Get("Authorization") != "" {
			t.Errorf("headers %v", r.Header)
		}
		w.Write([]byte(`{"content":[{"type":"text","text":"Edited "},{"type":"text","text":"text."}]}`))
	}))
	defer srv.Close()
	got, err := Enhance(ctxFor(t, 5*time.Second), Config{Provider: StyleAnthropic, APIKey: "ak", BaseURL: srv.URL}, "sys", "user text")
	if err != nil || got != "Edited text." {
		t.Errorf("%q %v", got, err)
	}
}

func TestErrorsHideTheKey(t *testing.T) {
	const key = "sk-very-secret-key"
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusUnauthorized)
		w.Write([]byte(`{"error":{"message":"Incorrect API key provided: ` + key + `"}}`))
	}))
	defer srv.Close()
	_, err := Enhance(ctxFor(t, 5*time.Second), Config{Provider: StyleOpenAI, APIKey: key, BaseURL: srv.URL}, "s", "u")
	if err == nil || strings.Contains(err.Error(), key) || !strings.Contains(err.Error(), "401") || !strings.Contains(err.Error(), "Incorrect API key") {
		t.Fatalf("error %v", err)
	}
}

func TestTimeout(t *testing.T) {
	release := make(chan struct{})
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { <-release }))
	defer srv.Close()
	defer close(release)
	start := time.Now()
	_, err := Enhance(ctxFor(t, 100*time.Millisecond), Config{Provider: StyleOpenAICompatible, BaseURL: srv.URL, Model: "m"}, "s", "u")
	if !errors.Is(err, ErrTimeout) || time.Since(start) > 2*time.Second {
		t.Fatalf("%v after %v", err, time.Since(start))
	}
}

func TestBadReplies(t *testing.T) {
	for body, want := range map[string]string{
		`not json`:       "not valid JSON",
		`{"choices":[]}`: "no text",
	} {
		srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.Write([]byte(body)) }))
		_, err := Enhance(ctxFor(t, 5*time.Second), Config{Provider: StyleOpenAICompatible, BaseURL: srv.URL, Model: "m"}, "s", "u")
		srv.Close()
		if err == nil || !strings.Contains(err.Error(), want) {
			t.Errorf("%s: %v", body, err)
		}
	}
}
