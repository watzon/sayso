package enhance

import (
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/providers"
	"github.com/watzon/sayso/internal/store"
)

func TestParseOutput(t *testing.T) {
	for in, want := range map[string]string{
		`{"text":"Hello."}`:                     "Hello.",
		"```json\n{\"text\": \"Hi\"}\n```":      "Hi",
		`{"text":"{\"text\": \"Sayso is great.\"}"}`: "Sayso is great.",
		`{"text":"{a, b} is a set."}`:           "{a, b} is a set.",
		`{"text":"<transcript>x</transcript>"}`: "x",
	} {
		if got, err := ParseOutput(in); err != nil || got != want {
			t.Errorf("%q: %q %v", in, got, err)
		}
	}
	for _, in := range []string{"no", `[1]`, `{"x":1}`, `{"text":"  "}`} {
		if _, err := ParseOutput(in); !IsKind(err, ErrInvalidOutput) {
			t.Errorf("%q: %v", in, err)
		}
	}
}

type chatServer struct {
	*httptest.Server
	mu     sync.Mutex
	bodies []map[string]any
	auth   []string
}

func newChatServer(t *testing.T, reply func(n int, body map[string]any) (int, string)) *chatServer {
	s := &chatServer{}
	s.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var body map[string]any
		json.NewDecoder(r.Body).Decode(&body)
		s.mu.Lock()
		n := len(s.bodies)
		s.bodies = append(s.bodies, body)
		s.auth = append(s.auth, r.Header.Get("Authorization"))
		s.mu.Unlock()
		st, text := reply(n, body)
		w.WriteHeader(st)
		io.WriteString(w, text)
	}))
	t.Cleanup(s.Close)
	return s
}

func completion(content string) string {
	b, _ := json.Marshal(map[string]any{"choices": []any{map[string]any{"message": map[string]any{"content": content}, "finish_reason": "stop"}}})
	return string(b)
}

func TestOpenAICompatibleModeLadder(t *testing.T) {
	ClearModeCache()
	srv := newChatServer(t, func(n int, body map[string]any) (int, string) {
		if rf, _ := body["response_format"].(map[string]any); rf != nil && rf["type"] == "json_schema" {
			return 400, `{"error":"json_schema unsupported"}`
		}
		return 200, completion(`{"text":"Edited."}`)
	})
	e := NewOpenAICompatible("p", srv.URL+"/v1/", "m").WithAPIKey("sk")
	r := Request{SystemPrompt: "s", Transcript: "t", Timeout: 5 * time.Second}
	resp, err := e.Enhance(r)
	if err != nil || resp.Text != "Edited." || resp.Mode != "json_object" || srv.auth[0] != "Bearer sk" {
		t.Fatal(resp, err)
	}
	if !strings.Contains(srv.bodies[1]["messages"].([]any)[0].(map[string]any)["content"].(string), `{"additionalProperties":false`) {
		t.Fatal("json_object mode names the schema")
	}
	// The mode is remembered: the next call starts at json_object.
	e.Enhance(r)
	if len(srv.bodies) != 3 {
		t.Fatal(len(srv.bodies))
	}
	if e.IsCloud() {
		t.Fatal("localhost is not cloud")
	}
}

func TestOpenAICompatibleBodyAndErrors(t *testing.T) {
	e := NewOpenAICompatible("p", "https://openrouter.ai/api/v1", "m").WithThinking("high")
	e.ZeroDataRetention = true
	b := e.BuildBody(ModeToolCall, "m", Request{Transcript: "hello"})
	if b["reasoning"].(map[string]any)["effort"] != "high" || b["max_tokens"] != 256+thinkingTokens || b["tool_choice"] == nil {
		t.Fatal(b)
	}
	if pr := b["provider"].(map[string]any); pr["zdr"] != true || pr["require_parameters"] != true || pr["data_collection"] != "deny" {
		t.Fatal(pr)
	}
	if !e.IsCloud() {
		t.Fatal("openrouter is cloud")
	}
	cases := []struct {
		reply any
		kind  ErrorKind
	}{
		{map[string]any{"error": map[string]any{"code": 429.0, "message": "slow"}}, ErrHTTP},
		{map[string]any{"choices": []any{map[string]any{"message": map[string]any{"refusal": "no"}}}}, ErrRefused},
		{map[string]any{"choices": []any{map[string]any{"message": map[string]any{"content": "x"}, "finish_reason": "length"}}}, ErrInvalidOutput},
		{map[string]any{"choices": []any{}}, ErrInvalidOutput},
	}
	for _, c := range cases {
		if _, err := ParseReply(ModeJSONSchema, c.reply); !IsKind(err, c.kind) {
			t.Errorf("%v: %v", c.reply, err)
		}
	}
	tool := map[string]any{"choices": []any{map[string]any{"message": map[string]any{"tool_calls": []any{map[string]any{"function": map[string]any{"arguments": `{"text":"T"}`}}}}}}}
	if got, err := ParseReply(ModeToolCall, tool); err != nil || got != "T" {
		t.Fatal(got, err)
	}
}

func TestBuildEnhancer(t *testing.T) {
	secrets := providers.NewMemoryStore()
	cfg := config.AI{HTTPTimeoutMs: 4000, CLITimeoutMs: 8000}
	acct := "openrouter"
	m := "m"
	p := config.Provider{ID: "p", Kind: config.KindOpenAICompatible, BaseURL: "https://openrouter.ai/api/v1", Model: &m, APIKeyAccount: &acct}
	e := BuildEnhancer(p, secrets, cfg, nil)
	if _, err := e.Enhance(Request{}); !IsKind(err, ErrNotConfigured) || !strings.Contains(err.Error(), "openrouter") || !e.IsCloud() {
		t.Fatal(err)
	}
	for _, k := range []config.ProviderKind{config.KindClaudeCLI, config.KindACP, config.KindChatGPT} {
		if !BuildEnhancer(config.Provider{ID: "p", Kind: k}, secrets, cfg, nil).IsCloud() {
			t.Fatal(k)
		}
	}
	apple := BuildEnhancer(config.Provider{ID: "a", Kind: config.KindAppleIntelligence}, secrets, cfg, nil)
	if _, err := apple.Enhance(Request{}); !IsKind(err, ErrNotConfigured) {
		t.Fatal(err)
	}
	apple = BuildEnhancer(config.Provider{ID: "a", Kind: config.KindAppleIntelligence}, secrets, cfg, scripted{"Done."})
	if r, err := apple.Enhance(Request{SystemPrompt: "s", Transcript: "t", Timeout: time.Second}); err != nil || r.Text != "Done." || r.Mode != "guided_generation" {
		t.Fatal(r, err)
	}
}

type scripted struct{ text string }

func (s scripted) ModelName() (string, error) { return "Scripted", nil }
func (s scripted) Generate(instructions, prompt string, _ *float32, _ time.Duration) (Generated, error) {
	if prompt != "<transcript>\nt\n</transcript>" {
		return Generated{}, errors.New("bad prompt " + prompt)
	}
	return Generated{Text: s.text, Model: "Scripted"}, nil
}

func jwt(claims map[string]any) string {
	b, _ := json.Marshal(claims)
	return "e30." + base64.RawURLEncoding.EncodeToString(b) + ".sig"
}

func idToken() string {
	return jwt(map[string]any{"email": "a@b.com", "https://api.openai.com/auth": map[string]any{"chatgpt_account_id": "acct-1", "chatgpt_plan_type": "plus"}})
}

func TestChatGPTTokens(t *testing.T) {
	tk, err := tokensFromReply(map[string]any{"access_token": "at", "refresh_token": "rt", "id_token": idToken(), "expires_in": 3600.0}, nil)
	if err != nil || *tk.AccountID != "acct-1" || tk.label() != "Plus plan · a@b.com" || !tk.fresh() {
		t.Fatal(tk, err)
	}
	back := tokensFromJSON(tk.toJSON())
	if back == nil || back.toJSON() != tk.toJSON() {
		t.Fatal("round trip")
	}
	next, _ := tokensFromReply(map[string]any{"access_token": "a2"}, tk)
	if next.RefreshToken != "rt" || *next.AccountID != "acct-1" {
		t.Fatal(next)
	}
	if _, err := tokensFromReply(map[string]any{"refresh_token": "r"}, nil); err == nil {
		t.Fatal("no access token")
	}
}

func TestChatGPTEnhanceRefreshAndStream(t *testing.T) {
	secrets := providers.NewMemoryStore()
	stale := &tokens{AccessToken: "old", RefreshToken: "rt", ExpiresAt: 1}
	secrets.Set(ChatGPTAccountName("cg"), stale.toJSON())
	var seenAuth []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/oauth/token":
			r.ParseForm()
			if r.Form.Get("grant_type") != "refresh_token" || r.Form.Get("refresh_token") != "rt" {
				w.WriteHeader(400)
				return
			}
			io.WriteString(w, `{"access_token":"new","id_token":"`+idToken()+`","expires_in":3600}`)
		case "/codex/models":
			io.WriteString(w, `{"models":[{"slug":"gpt-big","priority":1},{"slug":"gpt-mini","priority":2,"supported_reasoning_levels":[{"effort":"high"},{"effort":"low"}]},{"slug":"hidden","visibility":"hide"}]}`)
		case "/codex/responses":
			seenAuth = append(seenAuth, r.Header.Get("Authorization")+"|"+r.Header.Get("chatgpt-account-id"))
			var body map[string]any
			json.NewDecoder(r.Body).Decode(&body)
			if body["model"] != "gpt-mini" || body["stream"] != true {
				w.WriteHeader(400)
				return
			}
			fmt.Fprint(w, "event: x\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"text\\\":\"}\n\n")
			fmt.Fprint(w, "data: {\"type\":\"response.output_text.delta\",\"delta\":\"\\\"Hi.\\\"}\"}\n\n")
			fmt.Fprint(w, "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":7,\"output_tokens\":3,\"output_tokens_details\":{\"reasoning_tokens\":1}}}}\n\n")
		default:
			w.WriteHeader(404)
		}
	}))
	defer srv.Close()
	e := NewChatGPT("cg", "", secrets).WithEndpoints(srv.URL+"/codex", srv.URL)
	resp, err := e.Enhance(Request{SystemPrompt: "s", Transcript: "hi", Timeout: 5 * time.Second})
	if err != nil || resp.Text != "Hi." || resp.Model != "gpt-mini" || *resp.Report.InputTokens != 7 || *resp.Report.ThinkingTokens != 1 {
		t.Fatal(resp, err)
	}
	if seenAuth[0] != "Bearer new|acct-1" {
		t.Fatal(seenAuth)
	}
	if st := ChatGPTStatus(secrets, "cg"); st.State != "signed_in" || st.Account != "Plus plan · a@b.com" {
		t.Fatal(st)
	}
	models, _ := parseChatGPTModels(map[string]any{"models": []any{map[string]any{"slug": "b", "priority": 2.0}, map[string]any{"slug": "a", "priority": 1.0}}})
	if len(models) != 2 || models[0].ID != "a" {
		t.Fatal(models)
	}
	if _, err := NewChatGPT("none", "m", secrets).Enhance(Request{Timeout: time.Second}); !IsKind(err, ErrNotConfigured) {
		t.Fatal(err)
	}
}

func TestChatGPTStreamErrors(t *testing.T) {
	for stream, kind := range map[string]ErrorKind{
		"data: {\"type\":\"response.refusal.done\",\"refusal\":\"no\"}\n":                                                          ErrRefused,
		"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"usage_limit_reached\",\"message\":\"m\"}}}\n": ErrHTTP,
		"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"incomplete\"}}\n":                                       ErrInvalidOutput,
		"data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n":                                                       ErrNetwork,
	} {
		_, _, err := readStream(strings.NewReader(stream))
		if !IsKind(err, kind) {
			t.Errorf("%q: %v", stream, err)
		}
		if kind == ErrHTTP && err.(*Error).Status != 429 {
			t.Error("usage limit is 429")
		}
	}
}

func TestChatGPTSignIn(t *testing.T) {
	secrets := providers.NewMemoryStore()
	issuer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		r.ParseForm()
		if r.URL.Path != "/oauth/token" || r.Form.Get("code") != "the-code" || r.Form.Get("code_verifier") == "" {
			w.WriteHeader(400)
			return
		}
		io.WriteString(w, `{"access_token":"at","refresh_token":"rt","id_token":"`+idToken()+`"}`)
	}))
	defer issuer.Close()
	s, err := StartChatGPTSignIn(secrets, "cg", 10*time.Second, &ChatGPTEndpoints{Issuer: issuer.URL, Ports: []int{0}}, DefaultSignInTexts)
	if err != nil {
		t.Fatal(err)
	}
	first := <-s.Events
	u, _ := url.Parse(first.URL)
	q := u.Query()
	if q.Get("client_id") != chatgptClientID || q.Get("code_challenge_method") != "S256" || q.Get("originator") != "sayso" {
		t.Fatal(first.URL)
	}
	cb, _ := url.Parse(q.Get("redirect_uri"))
	// A wrong state is refused and the flow keeps waiting.
	resp, _ := http.Get(cb.String() + "?state=wrong&code=x")
	if resp.StatusCode != 400 {
		t.Fatal(resp.StatusCode)
	}
	resp, _ = http.Get(cb.String() + "?state=" + url.QueryEscape(q.Get("state")) + "&code=the-code")
	if resp.StatusCode != 200 {
		t.Fatal(resp.StatusCode)
	}
	done := <-s.Events
	if done.Finished == nil || done.Finished.State != "signed_in" {
		t.Fatal(done)
	}
	if ChatGPTSignOut(secrets, "cg") != nil || ChatGPTStatus(secrets, "cg").State != "signed_out" {
		t.Fatal("sign out")
	}
}

type fakeEnhancer struct {
	replies []func() (Response, error)
	seen    []Request
}

func (f *fakeEnhancer) ID() string    { return "fake" }
func (f *fakeEnhancer) IsCloud() bool { return false }
func (f *fakeEnhancer) Enhance(r Request) (Response, error) {
	f.seen = append(f.seen, r)
	next := f.replies[0]
	f.replies = f.replies[1:]
	return next()
}

func okReply(text string) func() (Response, error) {
	return func() (Response, error) { return Response{Text: text, ProviderID: "fake", Model: "m", Mode: "json_schema"}, nil }
}

func TestPipeline(t *testing.T) {
	clean, raw := builtin("clean"), builtin("raw")
	rep := NewReplacer([]store.Replacement{{ID: 1, From: "git hub", To: "GitHub"}})
	fake := &fakeEnhancer{replies: []func() (Response, error){okReply("Open a GitHub issue.")}}
	p := TextPipeline{Replacer: rep, Style: &clean, Enhancer: fake, Timeout: 4 * time.Second}
	out := p.Run("open a git hub issue")
	if out.Text != "Open a GitHub issue." || fake.seen[0].Transcript != "open a GitHub issue" || len(out.Replacements) != 1 || out.Enhance.Result != store.EnhanceApplied {
		t.Fatal(out)
	}
	p.Style = &raw
	if out := p.Run("hi"); out.Enhance.Result != store.EnhanceNotUsed || p.WillEnhance() {
		t.Fatal(out)
	}
	// An invalid answer is tried once more; a second failure keeps the replaced text.
	bad := func() (Response, error) { return Response{}, invalidOutput("x") }
	fake = &fakeEnhancer{replies: []func() (Response, error){bad, bad}}
	p = TextPipeline{Replacer: rep, Style: &clean, Enhancer: fake, Timeout: 4 * time.Second}
	out = p.Run("git hub")
	if out.Text != "GitHub" || out.Enhance.Result != store.EnhanceFailed || len(out.Timings.Attempts) != 2 || out.ErrorFrom != "fake" {
		t.Fatal(out)
	}
	// A runaway reply counts as invalid.
	fake = &fakeEnhancer{replies: []func() (Response, error){okReply(strings.Repeat("x ", 300)), okReply("Short.")}}
	p.Enhancer = fake
	if out := p.Run("short"); out.Text != "Short." {
		t.Fatal(out.Text)
	}
	// The timeout grows 50 ms a word over 100 words.
	if p.stepTimeout(strings.Repeat("w ", 120)) != 4*time.Second+20*50*time.Millisecond {
		t.Fatal(p.stepTimeout(strings.Repeat("w ", 120)))
	}
}

type fakeCleaner struct{ resp CleanupResponse }

func (f fakeCleaner) Clean(r CleanupRequest) (CleanupResponse, error) {
	f.resp.Text = strings.ToUpper(r.Text[:1]) + r.Text[1:] + "."
	return f.resp, nil
}

func TestPipelineCleanup(t *testing.T) {
	email := builtin("email")
	sp, _ := FindCleanupModel("spokenly-cleanup-1")
	m := sp.ID
	run := &CleanerRun{Cleaner: fakeCleaner{CleanupResponse{Model: m, Parts: 1}}, ProviderID: "cm", Model: sp,
		Plan: email.RunPlanOn(config.Provider{Kind: config.KindCleanupModel, Model: &m}), LanguageSetting: "en"}
	p := TextPipeline{Replacer: NewReplacer(nil), Style: &email, Vocabulary: []string{"GPUI"}, Cleaner: run}
	out := p.Run("open the GPUI docs")
	if out.Text != "Open the GPUI docs." || out.Notice == nil || out.Notice.Kind != "cleanup_only" || out.Timings.Cleanup.Parts != 1 {
		t.Fatal(out)
	}
	run.LanguageSetting = "pl"
	if out := p.Run("x"); out.Notice.Kind != "language" || out.Enhance.Result != store.EnhanceNotUsed {
		t.Fatal(out)
	}
}
