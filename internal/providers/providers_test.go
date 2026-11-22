package providers

import (
	"bytes"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"mime"
	mpart "mime/multipart"
	"net/http"
	"net/http/httptest"
	"net/url"
	"reflect"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/config"
)

// Bytes that are not valid UTF-8, so a test proves the body is binary-safe.
var testWAV = []byte("RIFF\x00\x9f\x92\x96WAVEfmt SAYSO-WAV-MARKER\xff\xfe")

type part struct {
	name, filename, contentType string
	data                        []byte
}

type seen struct {
	method, path, rawQuery string
	header                 http.Header
	body                   []byte
	parts                  []part
}

func (s seen) names() []string {
	out := []string{}
	for _, p := range s.parts {
		out = append(out, p.name)
	}
	return out
}

func (s seen) fields(name string) []string {
	out := []string{}
	for _, p := range s.parts {
		if p.name == name {
			out = append(out, string(p.data))
		}
	}
	return out
}

func (s seen) field(name string) (string, bool) {
	f := s.fields(name)
	if len(f) == 0 {
		return "", false
	}
	return f[0], true
}

type answer struct {
	status int
	body   string
	delay  time.Duration
}

type mock struct {
	*httptest.Server
	mu       sync.Mutex
	requests []seen
}

func startMock(t *testing.T, handle func(n int, s seen) answer) *mock {
	m := &mock{}
	m.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		s := seen{method: r.Method, path: r.URL.Path, rawQuery: r.URL.RawQuery, header: r.Header.Clone(), body: body}
		if mt, params, err := mime.ParseMediaType(r.Header.Get("Content-Type")); err == nil && mt == "multipart/form-data" {
			mr := mpart.NewReader(bytes.NewReader(body), params["boundary"])
			for {
				p, err := mr.NextPart()
				if err != nil {
					break
				}
				data, _ := io.ReadAll(p)
				s.parts = append(s.parts, part{p.FormName(), p.FileName(), p.Header.Get("Content-Type"), data})
			}
		}
		m.mu.Lock()
		n := len(m.requests)
		m.requests = append(m.requests, s)
		m.mu.Unlock()
		a := handle(n, s)
		if a.delay > 0 {
			select {
			case <-time.After(a.delay):
			case <-r.Context().Done():
				return
			}
		}
		w.WriteHeader(a.status)
		io.WriteString(w, a.body)
	}))
	t.Cleanup(m.Close)
	return m
}

func (m *mock) seen() []seen {
	m.mu.Lock()
	defer m.mu.Unlock()
	return append([]seen(nil), m.requests...)
}

func ok(text string) answer { return answer{200, fmt.Sprintf(`{"text":%q}`, text), 0} }

func provider(kind config.SpeechProviderKind, m *mock) config.SpeechProvider {
	base := m.URL + "/v1"
	return config.SpeechProvider{ID: "p", Name: "P", Kind: kind, BaseURL: &base}
}

func req(model, language string, words ...string) Request {
	return Request{Model: model, WAV: testWAV, Language: language, Vocabulary: words, Timeout: 5 * time.Second}
}

func speechErr(t *testing.T, err error) *config.SpeechError {
	t.Helper()
	var se *config.SpeechError
	if !errors.As(err, &se) {
		t.Fatalf("not a SpeechError: %v", err)
	}
	return se
}

var openAIKinds = []config.SpeechProviderKind{config.SpeechOpenAI, config.SpeechGroq, config.SpeechMistral, config.SpeechOpenAICompatible}

func TestOpenAIKindsSendFileModelAndHints(t *testing.T) {
	for _, kind := range []config.SpeechProviderKind{config.SpeechOpenAI, config.SpeechGroq, config.SpeechOpenAICompatible} {
		m := startMock(t, func(int, seen) answer { return ok("  Hello world.\n") })
		text, err := Transcribe(provider(kind, m), "sk-test", req("whisper-1", "en", "Sayso", " ", "Kubernetes"))
		if err != nil || text != "Hello world." {
			t.Fatalf("%s: %q %v", kind, text, err)
		}
		s := m.seen()[0]
		if s.method != "POST" || s.path != "/v1/audio/transcriptions" || s.header.Get("Authorization") != "Bearer sk-test" {
			t.Fatalf("%s: %+v", kind, s)
		}
		f := s.parts[0]
		if f.filename != "audio.wav" || f.contentType != "audio/wav" || !bytes.Equal(f.data, testWAV) {
			t.Fatalf("file part %+v", f)
		}
		if p, _ := s.field("prompt"); p != "Sayso, Kubernetes" {
			t.Fatalf("prompt %q", p)
		}
		if l, _ := s.field("language"); l != "en" {
			t.Fatalf("language %q", l)
		}
		if s.header.Get("User-Agent") != "Sayso" {
			t.Fatalf("user agent %q", s.header.Get("User-Agent"))
		}
	}
}

func TestOpenAIWithoutHintsSendsBasicFields(t *testing.T) {
	m := startMock(t, func(int, seen) answer { return ok("ok") })
	if _, err := Transcribe(provider(config.SpeechGroq, m), "k", req("whisper-large-v3", "")); err != nil {
		t.Fatal(err)
	}
	if got := m.seen()[0].names(); !reflect.DeepEqual(got, []string{"file", "model", "response_format"}) {
		t.Fatal(got)
	}
}

func TestGPTTranscribeFieldNames(t *testing.T) {
	m := startMock(t, func(int, seen) answer { return ok("ok") })
	if _, err := Transcribe(provider(config.SpeechOpenAI, m), "k", req("gpt-transcribe", "de", "Sayso", "<b>Ku\r\nbe</b>", "<>")); err != nil {
		t.Fatal(err)
	}
	s := m.seen()[0]
	if !reflect.DeepEqual(s.fields("languages[]"), []string{"de"}) || len(s.fields("language")) != 0 {
		t.Fatal(s.names())
	}
	if !reflect.DeepEqual(s.fields("keywords"), []string{"Sayso\nbKube/b"}) {
		t.Fatalf("%q", s.fields("keywords"))
	}
	// Only the OpenAI kind gets these names.
	m2 := startMock(t, func(int, seen) answer { return ok("ok") })
	Transcribe(provider(config.SpeechOpenAICompatible, m2), "", req("gpt-transcribe", "de", "Sayso"))
	if p, _ := m2.seen()[0].field("prompt"); p != "Sayso" {
		t.Fatal(m2.seen()[0].names())
	}
}

func TestMistralContextBiasUpTo100(t *testing.T) {
	m := startMock(t, func(int, seen) answer { return ok("ok") })
	var words []string
	for i := 0; i < 120; i++ {
		words = append(words, fmt.Sprintf("word%d", i))
	}
	Transcribe(provider(config.SpeechMistral, m), "k", req("voxtral-mini-latest", "fr", words...))
	s := m.seen()[0]
	if n := len(s.fields("context_bias")); n != 100 {
		t.Fatal(n)
	}
	if _, has := s.field("prompt"); has {
		t.Fatal("prompt sent to Mistral")
	}
}

func TestHintedRequestIsRetriedWithoutHints(t *testing.T) {
	for _, status := range []int{400, 422} {
		m := startMock(t, func(n int, _ seen) answer {
			if n == 0 {
				return answer{status: status, body: `{"error":{"message":"unknown field"}}`}
			}
			return ok("second")
		})
		text, err := Transcribe(provider(config.SpeechOpenAICompatible, m), "", req("m", "en", "Sayso"))
		if err != nil || text != "second" {
			t.Fatal(text, err)
		}
		r := m.seen()
		if len(r) != 2 || !reflect.DeepEqual(r[0].names(), []string{"file", "model", "response_format", "language", "prompt"}) ||
			!reflect.DeepEqual(r[1].names(), []string{"file", "model", "response_format"}) {
			t.Fatal(r[0].names(), r[1].names())
		}
	}
}

func TestRetryReturnsSecondErrorAndOnlyOnce(t *testing.T) {
	m := startMock(t, func(n int, _ seen) answer {
		st := 500
		if n == 0 {
			st = 400
		}
		return answer{status: st, body: fmt.Sprintf(`{"error":{"message":"try %d"}}`, n)}
	})
	_, err := Transcribe(provider(config.SpeechGroq, m), "k", req("m", "en"))
	se := speechErr(t, err)
	if se.Kind != config.SpeechHTTP || se.Status != 500 || se.Message != "try 1" || len(m.seen()) != 2 {
		t.Fatal(se, len(m.seen()))
	}
	// Without hints, no retry.
	m2 := startMock(t, func(int, seen) answer { return answer{status: 400, body: `{"error":{"message":"bad file"}}`} })
	_, err = Transcribe(provider(config.SpeechOpenAICompatible, m2), "", req("m", "", "  "))
	if se := speechErr(t, err); se.Message != "bad file" || len(m2.seen()) != 1 {
		t.Fatal(se)
	}
}

func TestRetrySharesTheDeadline(t *testing.T) {
	m := startMock(t, func(int, seen) answer { return answer{status: 400, body: "no", delay: 300 * time.Millisecond} })
	r := req("m", "en")
	r.Timeout = 500 * time.Millisecond
	started := time.Now()
	_, err := Transcribe(provider(config.SpeechOpenAICompatible, m), "", r)
	if se := speechErr(t, err); se.Kind != config.SpeechTimeout || se.Ms != 500 {
		t.Fatal(se)
	}
	if time.Since(started) > 1200*time.Millisecond {
		t.Fatal(time.Since(started))
	}
}

func TestOpenAIErrorTexts(t *testing.T) {
	cases := [][2]string{
		{`{"error":{"message":"Incorrect API key","type":"x"}}`, "Incorrect API key"},
		{`{"message":"top level"}`, "top level"},
		{`{"detail":"a detail"}`, "a detail"},
		{`{"error":"plain error"}`, "plain error"},
		{"<html>Unauthorized</html>", "<html>Unauthorized</html>"},
	}
	for _, c := range cases {
		m := startMock(t, func(int, seen) answer { return answer{status: 401, body: c[0]} })
		_, err := Transcribe(provider(config.SpeechGroq, m), "k", req("m", ""))
		if se := speechErr(t, err); se.Status != 401 || se.Message != c[1] || se.UserMessageKey() != "core.speech.error.key_refused" {
			t.Fatal(se)
		}
	}
	m := startMock(t, func(int, seen) answer { return answer{status: 502, body: strings.Repeat("x", 1000)} })
	_, err := Transcribe(provider(config.SpeechOpenAI, m), "k", req("m", ""))
	if se := speechErr(t, err); se.Message != strings.Repeat("x", 200) {
		t.Fatal(len(se.Message))
	}
}

func TestMissingTranscriptAndSilence(t *testing.T) {
	for _, body := range []string{`{"result":"no text field"}`, `{"text":null}`, "not json"} {
		m := startMock(t, func(int, seen) answer { return answer{status: 200, body: body} })
		_, err := Transcribe(provider(config.SpeechOpenAI, m), "k", req("m", ""))
		if se := speechErr(t, err); se.Kind != config.SpeechInvalidResponse {
			t.Fatal(body, se)
		}
	}
	m := startMock(t, func(int, seen) answer { return ok("  ") })
	if text, err := Transcribe(provider(config.SpeechOpenAI, m), "k", req("m", "")); err != nil || text != "" {
		t.Fatal(text, err)
	}
}

func TestNetworkErrorAndTimeout(t *testing.T) {
	base := "http://127.0.0.1:9/v1"
	p := config.SpeechProvider{ID: "p", Kind: config.SpeechOpenAICompatible, BaseURL: &base}
	_, err := Transcribe(p, "", req("m", ""))
	if se := speechErr(t, err); se.Kind != config.SpeechNetwork {
		t.Fatal(se)
	}
	m := startMock(t, func(int, seen) answer { a := ok("late"); a.delay = 2 * time.Second; return a })
	r := req("m", "")
	r.Timeout = 300 * time.Millisecond
	_, err = Transcribe(provider(config.SpeechOpenAICompatible, m), "", r)
	if se := speechErr(t, err); se.Kind != config.SpeechTimeout || se.Ms != 300 {
		t.Fatal(se)
	}
}

func TestCloudProviderWithoutKeyFailsBeforeAnyRequest(t *testing.T) {
	invalid := "https://stt.invalid"
	for _, kind := range config.AllSpeechKinds {
		p := config.SpeechProvider{ID: "p", Kind: kind, BaseURL: &invalid}
		for _, key := range []string{"", "  \n"} {
			_, err := Transcribe(p, key, req("m", ""))
			if se := speechErr(t, err); se.Kind != config.SpeechNotConfigured || se.Missing != config.MissingAPIKey {
				t.Fatal(kind, se)
			}
			if se := speechErr(t, Check(p, key, time.Second)); se.Missing != config.MissingAPIKey {
				t.Fatal(se)
			}
		}
	}
	blank := "  "
	p := config.SpeechProvider{ID: "p", Kind: config.SpeechOpenAICompatible, BaseURL: &blank}
	_, err := Transcribe(p, "k", req("m", ""))
	if se := speechErr(t, err); se.Kind != config.SpeechNotConfigured || se.Missing != config.MissingAddress {
		t.Fatal(se)
	}
	if _, err := ListModels(p, "k", time.Second); speechErr(t, err).Missing != config.MissingAddress {
		t.Fatal(err)
	}
}

func TestLocalServerNeedsNoKey(t *testing.T) {
	m := startMock(t, func(int, seen) answer { return ok("local") })
	text, err := Transcribe(provider(config.SpeechOpenAICompatible, m), "", req("m", ""))
	if err != nil || text != "local" || m.seen()[0].header.Get("Authorization") != "" {
		t.Fatal(text, err)
	}
}

func TestListModelsAndCheck(t *testing.T) {
	for _, kind := range openAIKinds {
		m := startMock(t, func(int, seen) answer {
			return answer{status: 200, body: `{"data":[{"id":"b"},{"id":"a"},{"id":"b"},{"x":1}]}`}
		})
		ids, err := ListModels(provider(kind, m), "k", time.Second)
		if err != nil || !reflect.DeepEqual(ids, []string{"a", "b"}) {
			t.Fatal(ids, err)
		}
		s := m.seen()[0]
		if s.method != "GET" || s.path != "/v1/models" || s.header.Get("Authorization") != "Bearer k" {
			t.Fatal(s)
		}
		if err := Check(provider(kind, m), "k", time.Second); err != nil {
			t.Fatal(err)
		}
	}
	ids, err := ListModels(config.SpeechProvider{Kind: config.SpeechDeepgram}, "", time.Second)
	if err != nil || !reflect.DeepEqual(ids, []string{"nova-3", "nova-2"}) {
		t.Fatal(ids)
	}
	m := startMock(t, func(int, seen) answer { return answer{status: 200, body: `{"nope":1}`} })
	if _, err := ListModels(provider(config.SpeechOpenAI, m), "k", time.Second); speechErr(t, err).Kind != config.SpeechInvalidResponse {
		t.Fatal(err)
	}
}

func TestElevenLabs(t *testing.T) {
	long := strings.Repeat("x", 50)
	m := startMock(t, func(int, seen) answer { return ok("hi") })
	text, err := Transcribe(provider(config.SpeechElevenLabs, m), "xi", req("scribe_v2", "de", "Sayso", long))
	if err != nil || text != "hi" {
		t.Fatal(text, err)
	}
	s := m.seen()[0]
	if s.path != "/v1/speech-to-text" || s.header.Get("xi-api-key") != "xi" {
		t.Fatal(s.path)
	}
	want := []string{"file", "model_id", "tag_audio_events", "diarize", "language_code", "keyterms"}
	if !reflect.DeepEqual(s.names(), want) {
		t.Fatal(s.names())
	}
	for body, msg := range map[string]string{
		`{"detail":{"status":"invalid_api_key","message":"bad key"}}`: "bad key",
		`{"detail":"plain"}`: "plain",
		`{"detail":[{"loc":["body"],"msg":"field required"}]}`: "field required",
	} {
		m := startMock(t, func(int, seen) answer { return answer{status: 401, body: body} })
		_, err := Transcribe(provider(config.SpeechElevenLabs, m), "xi", req("scribe_v2", ""))
		if speechErr(t, err).Message != msg {
			t.Fatal(err)
		}
	}
	if err := Check(provider(config.SpeechElevenLabs, m), "xi", time.Second); err != nil || m.seen()[1].path != "/v1/models" {
		t.Fatal(err)
	}
}

func TestDeepgram(t *testing.T) {
	m := startMock(t, func(int, seen) answer {
		return answer{status: 200, body: `{"results":{"channels":[{"alternatives":[{"transcript":"Hi there."}]}]}}`}
	})
	text, err := Transcribe(provider(config.SpeechDeepgram, m), "dg", req("nova-3", "en", "New York", "Zoë"))
	if err != nil || text != "Hi there." {
		t.Fatal(text, err)
	}
	s := m.seen()[0]
	if s.path != "/v1/listen" || s.header.Get("Authorization") != "Token dg" || s.header.Get("Content-Type") != "audio/wav" || !bytes.Equal(s.body, testWAV) {
		t.Fatal(s.header)
	}
	if s.rawQuery != "model=nova-3&smart_format=true&punctuate=true&language=en&keyterm=New%20York&keyterm=Zo%C3%AB" {
		t.Fatal(s.rawQuery)
	}
	Transcribe(provider(config.SpeechDeepgram, m), "dg", req("nova-2", "", "Sayso"))
	if q := m.seen()[1].rawQuery; q != "model=nova-2&smart_format=true&punctuate=true&detect_language=true" {
		t.Fatal(q)
	}
	// The keyterm budget.
	var words []string
	for i := 0; i < 100; i++ {
		words = append(words, strings.Repeat("a", 9)+fmt.Sprint(i%10))
	}
	Transcribe(provider(config.SpeechDeepgram, m), "dg", req("nova-3", "", words...))
	q, _ := url.ParseQuery(m.seen()[2].rawQuery)
	if len(q["keyterm"]) != 40 {
		t.Fatal(len(q["keyterm"]))
	}
	if err := Check(provider(config.SpeechDeepgram, m), "dg", time.Second); err != nil || m.seen()[3].path != "/v1/projects" {
		t.Fatal(err)
	}
	if percentEncode("a&b=c#d+e/f?g") != "a%26b%3Dc%23d%2Be%2Ff%3Fg" || percentEncode("nova-3.general_~") != "nova-3.general_~" {
		t.Fatal("percent encode")
	}
}

func TestAssemblyAI(t *testing.T) {
	m := startMock(t, func(int, seen) answer { return ok("aai") })
	text, err := Transcribe(provider(config.SpeechAssemblyAI, m), "aai-key", req("universal-3-5-pro", "de", "Sayso", " "))
	if err != nil || text != "aai" {
		t.Fatal(text, err)
	}
	s := m.seen()[0]
	if s.path != "/v1/transcribe" || s.header.Get("Authorization") != "aai-key" || s.header.Get("X-AAI-Model") != "universal-3-5-pro" {
		t.Fatal(s.header)
	}
	if c, _ := s.field("config"); c != `{"keyterms_prompt":["Sayso"],"language_codes":["de"]}` {
		t.Fatal(c)
	}
	if !reflect.DeepEqual(s.names(), []string{"config", "audio"}) {
		t.Fatal(s.names())
	}
	if string(assemblyConfig(Request{})) != "{}" {
		t.Fatal("empty config")
	}
	// The check accepts bad audio, not a bad key.
	for body, wantOK := range map[string]bool{`{"error_code":"audio_too_short","error":"short"}`: true, `{"error":"Invalid API key"}`: false} {
		m := startMock(t, func(int, seen) answer { return answer{status: 400, body: body} })
		err := Check(provider(config.SpeechAssemblyAI, m), "k", time.Second)
		if (err == nil) != wantOK {
			t.Fatal(body, err)
		}
		if a := m.seen()[0].parts[1]; len(a.data) != 44+16000 {
			t.Fatal(len(a.data))
		}
	}
}

func TestSilentWAV(t *testing.T) {
	w := SilentWAV(500)
	if len(w) != 44+16000 || string(w[0:4]) != "RIFF" || binary.LittleEndian.Uint32(w[4:]) != 36+16000 || string(w[8:16]) != "WAVEfmt " ||
		binary.LittleEndian.Uint16(w[22:]) != 1 || binary.LittleEndian.Uint32(w[24:]) != 16000 || string(w[36:40]) != "data" {
		t.Fatal("bad header")
	}
}

func TestTruncatePrompt(t *testing.T) {
	cases := []struct {
		words []string
		limit int
		want  string
	}{
		{[]string{"Sayso", "Kubernetes"}, 800, "Sayso, Kubernetes"},
		{[]string{"aaaa", "bbbb", "cccc"}, 12, "aaaa, bbbb"},
		{[]string{"aaaa", "bbbb"}, 10, "aaaa, bbbb"},
		{[]string{"aaaa", "bbbb"}, 9, "aaaa"},
		{[]string{"abcdef"}, 5, ""},
		{nil, 5, ""},
		{[]string{"äöü", "ñññ"}, 8, "äöü, ñññ"},
	}
	for _, c := range cases {
		if got := truncatePrompt(c.words, c.limit); got != c.want {
			t.Errorf("%v %d: %q", c.words, c.limit, got)
		}
	}
}

func TestMultipartBody(t *testing.T) {
	form := newMultipart()
	b := strings.SplitN(form.contentType(), "boundary=", 2)[1]
	form.text("model", "whisper-1")
	form.part("config", "application/json", []byte("{}"))
	form.file("file", "audio.wav", "audio/wav", []byte{0, 159, 146, 150})
	want := "--" + b + "\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nwhisper-1\r\n" +
		"--" + b + "\r\nContent-Disposition: form-data; name=\"config\"\r\nContent-Type: application/json\r\n\r\n{}\r\n" +
		"--" + b + "\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n" +
		"\x00\x9f\x92\x96\r\n--" + b + "--\r\n"
	if string(form.finish()) != want {
		t.Fatal("body differs")
	}
	if newMultipart().contentType() == newMultipart().contentType() {
		t.Fatal("same boundary")
	}
	f := newMultipart()
	f.file("fi\"le\r\nX: y", "a\"b\r\n.wav", "audio/wav", []byte("x"))
	if !strings.Contains(string(f.finish()), "name=\"fi%22leX: y\"; filename=\"a%22b.wav\"") {
		t.Fatal("quoting")
	}
}

func TestDeadlineDefaults(t *testing.T) {
	if newDeadline(0).timeoutMs() != 30000 || newDeadline(250*time.Millisecond).timeoutMs() != 250 {
		t.Fatal("defaults")
	}
	d := newDeadline(time.Millisecond)
	time.Sleep(5 * time.Millisecond)
	if _, err := d.remaining(); speechErr(t, err).Ms != 1 {
		t.Fatal(err)
	}
}

func TestMemoryStore(t *testing.T) {
	s := NewMemoryStore()
	var _ SecretStore = s
	var _ SecretStore = KeychainStore{}
	if s.Get("a") != "" {
		t.Fatal()
	}
	s.Set("a", "one")
	s.Set("a", "two")
	if s.Get("a") != "two" {
		t.Fatal()
	}
	s.Delete("a")
	s.Delete("a")
	s.Set("b", "")
	if s.Get("a") != "" || s.Get("b") != "" {
		t.Fatal()
	}
}
