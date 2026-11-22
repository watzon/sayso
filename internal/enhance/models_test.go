package enhance

import (
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/providers"
)

const claudeModelsLine = `{"type":"control_response","response":{"subtype":"success","request_id":"sayso-models","response":{"commands":[],"models":[` +
	`{"value":"default","resolvedModel":"claude-opus-5-5","displayName":"Default (recommended)","description":"Opus 5.5 · Best for everyday, complex tasks"},` +
	`{"value":"opus","displayName":"Opus 5.5","description":"For complex work and everyday tasks","supportsEffort":true,"supportedEffortLevels":["max","low","medium","high","xhigh"]},` +
	`{"value":"haiku","resolvedModel":"claude-haiku-4-5-20251001","displayName":"Haiku 4.5","description":"Fastest for quick answers"},` +
	`{"value":"bare"},` +
	`{"value":"haiku","displayName":"Haiku again"}],"account":{}}}}`

func ids(models []ModelChoice) []string {
	out := []string{}
	for _, m := range models {
		out = append(out, m.ID)
	}
	return out
}

func TestClaudeModelsAnswerDropsDefaultAndKeepsOrder(t *testing.T) {
	models, ok, err := claudeModelsAnswer(claudeModelsLine)
	if !ok || err != nil {
		t.Fatal(ok, err)
	}
	if got := ids(models); !reflect.DeepEqual(got, []string{"opus", "haiku", "bare"}) {
		t.Fatal(got)
	}
	if models[1].Name != "Haiku 4.5" || models[1].Description == nil || *models[1].Description != "Fastest for quick answers" {
		t.Fatalf("%+v", models[1])
	}
	if models[2].Name != "bare" || models[2].Description != nil {
		t.Fatalf("%+v", models[2])
	}
	// The levels of the tool, from the least thinking to the most. Haiku 4.5 has none.
	if !reflect.DeepEqual(models[0].Thinking, []string{"low", "medium", "high", "xhigh", "max"}) || len(models[1].Thinking) != 0 {
		t.Fatal(models[0].Thinking, models[1].Thinking)
	}
}

func TestClaudeModelsAnswerSkipsOtherLines(t *testing.T) {
	for _, line := range []string{
		`{"type":"system","subtype":"init"}`,
		`not json`,
		`{"type":"control_response","response":{"subtype":"success","request_id":"sayso-init","response":{}}}`,
	} {
		if _, ok, _ := claudeModelsAnswer(line); ok {
			t.Errorf("took %s", line)
		}
	}
}

func TestClaudeModelsErrors(t *testing.T) {
	errLine := func(text string) string {
		return `{"type":"control_response","response":{"subtype":"error","request_id":"sayso-models","error":"` + text + `"}}`
	}
	if _, _, err := claudeModelsAnswer(errLine("Not logged in. Run /login")); !IsKind(err, ErrNotConfigured) {
		t.Fatal(err)
	}
	if _, _, err := claudeModelsAnswer(errLine("boom")); !IsKind(err, ErrCLI) || !strings.Contains(err.Error(), "boom") {
		t.Fatal(err)
	}
	empty := `{"type":"control_response","response":{"subtype":"success","request_id":"sayso-models","response":{}}}`
	if _, _, err := claudeModelsAnswer(empty); !IsKind(err, ErrInvalidOutput) {
		t.Fatal(err)
	}
}

// claudeLists is a fake claude that answers the listing request.
const claudeLists = `IFS= read -r init
printf '%s\n' "$init" > "$D/init"
echo '{"type":"system","subtype":"init"}'
echo '` + claudeModelsLine + `'
IFS= read -r rest
`

func TestListProviderModelsRunsClaude(t *testing.T) {
	program, dir := fakeClaude(t, claudeLists)
	cfgDir := t.TempDir()
	p := config.Provider{ID: "claude-code", Kind: config.KindClaudeCLI, Path: &program, ConfigDir: &cfgDir, Model: ptrTo("sonnet")}
	models, err := ListProviderModels(p, providers.NewMemoryStore(), nil, 5*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	if got := ids(models); !reflect.DeepEqual(got, []string{"opus", "haiku", "bare"}) {
		t.Fatal(got)
	}
	// No prompt and no model: only the initialize request goes in.
	init := readFile(t, filepath.Join(dir, "init"))
	if !strings.Contains(init, `"request_id":"sayso-models"`) || !strings.Contains(init, `"subtype":"initialize"`) || strings.Contains(init, "systemPrompt") {
		t.Fatal(init)
	}
	args := strings.Split(strings.TrimSpace(readFile(t, filepath.Join(dir, "args"))), "\n")
	if !slices.Contains(args, "-p") || !slices.Contains(args, "--strict-mcp-config") || slices.Contains(args, "--model") {
		t.Fatal(args)
	}
	env := readFile(t, filepath.Join(dir, "env"))
	if !strings.Contains(env, "MAX_THINKING_TOKENS=0\n") || !strings.Contains(env, "CLAUDE_CONFIG_DIR="+cfgDir+"\n") {
		t.Fatal("env misses the thinking limit or the config folder")
	}
}

func TestListProviderModelsClaudeFailures(t *testing.T) {
	cases := []struct {
		name  string
		body  string
		check func(error) bool
	}{
		{"not signed in", `IFS= read -r init
echo '{"type":"control_response","response":{"subtype":"error","request_id":"sayso-models","error":"Invalid API key · Please run /login"}}'
IFS= read -r rest
`, func(e error) bool { return IsKind(e, ErrNotConfigured) && strings.Contains(e.Error(), "not signed in") }},
		{"stops early", "echo 'something broke' >&2\nexit 3\n", func(e error) bool {
			return IsKind(e, ErrCLI) && strings.Contains(e.Error(), "something broke")
		}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			program, _ := fakeClaude(t, tc.body)
			_, err := ListProviderModels(config.Provider{ID: "c", Kind: config.KindClaudeCLI, Path: &program}, nil, nil, 5*time.Second)
			if !tc.check(err) {
				t.Fatal(err)
			}
		})
	}
}

func TestListProviderModelsClaudeTimesOut(t *testing.T) {
	program, _ := fakeClaude(t, "exec sleep 30\n")
	started := time.Now()
	_, err := ListProviderModels(config.Provider{ID: "c", Kind: config.KindClaudeCLI, Path: &program}, nil, nil, 300*time.Millisecond)
	if !IsKind(err, ErrTimeout) || err.Error() != "timed out after 300 ms" {
		t.Fatal(err)
	}
	if time.Since(started) > 3*time.Second {
		t.Fatal("the timeout did not stop the listing", time.Since(started))
	}
}

func TestListProviderModelsOpenAICompatible(t *testing.T) {
	var auth []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/models" {
			w.WriteHeader(404)
			return
		}
		auth = append(auth, r.Header.Get("Authorization"))
		io.WriteString(w, `{"data":[{"id":"z-model"},{"id":"a-model","name":"A Model"},{"id":"z-model"}]}`)
	}))
	defer srv.Close()
	secrets := providers.NewMemoryStore()
	secrets.Set("provider.or", "sk-1")
	account := "provider.or"
	p := config.Provider{ID: "or", Kind: config.KindOpenAICompatible, BaseURL: srv.URL + "/v1/", APIKeyAccount: &account}
	models, err := ListProviderModels(p, secrets, nil, 5*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	// Sorted by id, without repeats; the name is the id when the server sends none.
	if got := ids(models); !reflect.DeepEqual(got, []string{"a-model", "z-model"}) || models[0].Name != "A Model" || models[1].Name != "z-model" {
		t.Fatalf("%+v", models)
	}
	// A server on this computer lists without a key.
	p.APIKeyAccount = nil
	if _, err := ListProviderModels(p, secrets, nil, 5*time.Second); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(auth, []string{"Bearer sk-1", ""}) {
		t.Fatal(auth)
	}
	srv.Close()
	if _, err := ListProviderModels(p, secrets, nil, time.Second); !IsKind(err, ErrNetwork) && !IsKind(err, ErrTimeout) {
		t.Fatal(err)
	}
}

func TestListProviderModelsChatGPT(t *testing.T) {
	secrets := providers.NewMemoryStore()
	fresh := &tokens{AccessToken: "at", RefreshToken: "rt", ExpiresAt: nowSecs() + 3600, AccountID: ptrTo("acct-1")}
	secrets.Set(ChatGPTAccountName("cg"), fresh.toJSON())
	var seen string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/codex/models" {
			w.WriteHeader(404)
			return
		}
		seen = r.Header.Get("Authorization")
		io.WriteString(w, `{"models":[{"slug":"gpt-mini","display_name":"GPT Mini","priority":2,"supported_reasoning_levels":[{"effort":"high"},{"effort":"low"}]},`+
			`{"slug":"gpt-big","priority":1},{"slug":"gpt-big","priority":3},{"slug":"hidden","visibility":"hide"}]}`)
	}))
	defer srv.Close()
	base, token := chatgptListBase, chatgptListTokenURL
	chatgptListBase, chatgptListTokenURL = srv.URL+"/codex", srv.URL+"/oauth/token"
	t.Cleanup(func() { chatgptListBase, chatgptListTokenURL = base, token })

	models, err := ListProviderModels(config.Provider{ID: "cg", Kind: config.KindChatGPT}, secrets, nil, 5*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	// The backend's priority order, one choice per slug, hidden models left out.
	if got := ids(models); !reflect.DeepEqual(got, []string{"gpt-big", "gpt-mini"}) {
		t.Fatal(got)
	}
	if models[1].Name != "GPT Mini" || !reflect.DeepEqual(models[1].Thinking, []string{"low", "high"}) || seen != "Bearer at" {
		t.Fatalf("%+v %q", models[1], seen)
	}
	// Not signed in.
	if _, err := ListProviderModels(config.Provider{ID: "other", Kind: config.KindChatGPT}, secrets, nil, time.Second); !IsKind(err, ErrNotConfigured) {
		t.Fatal(err)
	}
}

type fakeLanguageModel struct {
	name string
	err  error
}

func (f fakeLanguageModel) ModelName() (string, error) { return f.name, f.err }
func (f fakeLanguageModel) Generate(string, string, *float32, time.Duration) (Generated, error) {
	return Generated{}, nil
}

func TestListProviderModelsOtherKinds(t *testing.T) {
	apple := config.Provider{ID: "apple", Kind: config.KindAppleIntelligence}
	models, err := ListProviderModels(apple, nil, fakeLanguageModel{name: "AFM 3 Core"}, time.Second)
	if err != nil || len(models) != 1 || models[0].ID != "AFM 3 Core" || models[0].Name != "AFM 3 Core" ||
		models[0].Description == nil || *models[0].Description != i18n.T("enhance.runs_on_this_mac") {
		t.Fatal(models, err)
	}
	if _, err := ListProviderModels(apple, nil, nil, time.Second); !IsKind(err, ErrNotConfigured) {
		t.Fatal(err)
	}
	unavailable := errors.New("Apple Intelligence is off")
	if _, err := ListProviderModels(apple, nil, fakeLanguageModel{err: unavailable}, time.Second); !errors.Is(err, unavailable) {
		t.Fatal(err)
	}
	cleanup := config.Provider{ID: "c", Kind: config.KindCleanupModel, Model: ptrTo("sayso-cleanup")}
	if models, err := ListProviderModels(cleanup, nil, nil, time.Second); err != nil || models == nil || len(models) != 0 {
		t.Fatal(models, err)
	}
	missing := filepath.Join(t.TempDir(), "nowhere", "claude")
	if _, err := ListProviderModels(config.Provider{ID: "c", Kind: config.KindClaudeCLI, Path: &missing}, nil, nil, time.Second); !IsKind(err, ErrNotConfigured) {
		t.Fatal(err)
	}
}

func TestDedupChoicesKeepsTheFirstInOrder(t *testing.T) {
	got := dedupChoices([]ModelChoice{{ID: "b", Name: "B1"}, {ID: "a"}, {ID: "b", Name: "B2"}, {ID: "c"}})
	if !reflect.DeepEqual(ids(got), []string{"b", "a", "c"}) || got[0].Name != "B1" {
		t.Fatalf("%+v", got)
	}
}
