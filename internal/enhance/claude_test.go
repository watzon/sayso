package enhance

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/providers"
)

func TestParseClaudeEnvelope(t *testing.T) {
	text, report, err := parseClaudeEnvelope(`{"type":"result","subtype":"success","is_error":false,"result":"","structured_output":{"text":"Hello."}}`)
	if err != nil || text != "Hello." || report.RunMs != nil || report.Turns != nil {
		t.Fatal(text, report, err)
	}
	_, report, err = parseClaudeEnvelope(`{"type":"result","subtype":"success","is_error":false,"duration_ms":5210,"duration_api_ms":4870,"num_turns":3,
		"usage":{"input_tokens":12,"cache_creation_input_tokens":160000,"cache_read_input_tokens":0,"output_tokens":41,"output_tokens_details":{"thinking_tokens":30}},
		"structured_output":{"text":"Hello."}}`)
	if err != nil {
		t.Fatal(err)
	}
	got := []uint64{*report.RunMs, *report.APIMs, *report.Turns, *report.InputTokens, *report.OutputTokens, *report.ThinkingTokens, *report.CacheReadTokens, *report.CacheWriteTokens}
	if !reflect.DeepEqual(got, []uint64{5210, 4870, 3, 12, 41, 30, 0, 160000}) {
		t.Fatal(got)
	}
	if text, _, err := parseClaudeEnvelope(`[{"type":"system"},{"type":"result","subtype":"success","structured_output":{"text":"Hi"}}]`); err != nil || text != "Hi" {
		t.Fatal(text, err)
	}
	for in, check := range map[string]func(error) bool{
		`{"type":"result","subtype":"success","is_error":true,"result":"boom"}`: func(e error) bool {
			return IsKind(e, ErrCLI) && strings.Contains(e.Error(), "boom")
		},
		`{"type":"result","subtype":"success","is_error":true,"result":"Not logged in. Run /login"}`: func(e error) bool { return IsKind(e, ErrNotConfigured) },
		`{"type":"result","subtype":"error_max_turns","is_error":false}`: func(e error) bool {
			return IsKind(e, ErrCLI) && strings.Contains(e.Error(), "error_max_turns")
		},
		`{"type":"result","subtype":"success","is_error":false,"result":"plain text"}`: func(e error) bool { return IsKind(e, ErrInvalidOutput) },
		`{"type":"result","subtype":"success","structured_output":{"txt":"x"}}`:        func(e error) bool { return IsKind(e, ErrInvalidOutput) },
		`not json`:            func(e error) bool { return IsKind(e, ErrInvalidOutput) },
		`[{"type":"system"}]`: func(e error) bool { return IsKind(e, ErrInvalidOutput) },
	} {
		if _, _, err := parseClaudeEnvelope(in); !check(err) {
			t.Errorf("%s: %v", in, err)
		}
	}
}

func TestClaudeCommandLineHasTheHygieneFlags(t *testing.T) {
	c := NewClaudeCLI("c", "/bin/claude", "haiku")
	l, _ := c.launch("haiku")
	args := l.args()
	hasPair := func(args []string, a, b string) bool {
		for i := 0; i+1 < len(args); i++ {
			if args[i] == a && args[i+1] == b {
				return true
			}
		}
		return false
	}
	for _, pair := range [][2]string{{"--model", "haiku"}, {"--effort", "low"}, {"--input-format", "stream-json"}, {"--output-format", "stream-json"}, {"--tools", ""}, {"--setting-sources", ""}} {
		if !hasPair(args, pair[0], pair[1]) {
			t.Errorf("missing %v in %q", pair, args)
		}
	}
	for _, flag := range []string{"--await-initialize", "--no-session-persistence", "--strict-mcp-config", "--disable-slash-commands", "--verbose"} {
		if !slices.Contains(args, flag) {
			t.Errorf("missing %s", flag)
		}
	}
	for _, flag := range []string{"--system-prompt", "-p", "--bare"} {
		if slices.Contains(args, flag) {
			t.Errorf("has %s: the prompt goes in on stdin, and --bare blocks the user's login", flag)
		}
	}
	high, _ := c.WithThinking("high").launch("haiku")
	if !hasPair(high.args(), "--effort", "high") {
		t.Fatal(high.args())
	}
	if !slices.Equal(l.env(), []string{"MAX_THINKING_TOKENS=0"}) || len(high.env()) != 0 {
		t.Fatal(l.env(), high.env())
	}
}

func TestClaudeRequestsCarryThePromptTheSchemaAndTheTranscript(t *testing.T) {
	var init map[string]any
	if err := json.Unmarshal([]byte(encodeLine(claudeInitRequest("SYSTEM <&>"))), &init); err != nil {
		t.Fatal(err)
	}
	req := init["request"].(map[string]any)
	schema, _ := json.Marshal(OutputSchema())
	gotSchema, _ := json.Marshal(req["jsonSchema"])
	if req["subtype"] != "initialize" || !reflect.DeepEqual(req["systemPrompt"], []any{"SYSTEM <&>"}) || string(schema) != string(gotSchema) || init["request_id"] != "sayso-init" {
		t.Fatal(init)
	}
	if strings.Contains(encodeLine(claudeInitRequest("<&>")), `\u003c`) {
		t.Fatal("HTML escaping in the line")
	}
	var user map[string]any
	_ = json.Unmarshal([]byte(encodeLine(claudeUserRequest("me and him"))), &user)
	content := user["message"].(map[string]any)["content"].([]any)[0].(map[string]any)
	if content["text"] != "<transcript>\nme and him\n</transcript>" || user["type"] != "user" {
		t.Fatal(user)
	}
}

func TestClaudeInitAnswer(t *testing.T) {
	for _, line := range []string{`{"type":"system","subtype":"init"}`, `{"type":"control_response","response":{"request_id":"other","subtype":"success"}}`, `nope`} {
		if ok, _ := claudeInitAnswer(line); ok {
			t.Fatal(line)
		}
	}
	if ok, err := claudeInitAnswer(`{"type":"control_response","response":{"request_id":"sayso-init","subtype":"success"}}`); !ok || err != nil {
		t.Fatal(ok, err)
	}
	if ok, err := claudeInitAnswer(`{"type":"control_response","response":{"request_id":"sayso-init","subtype":"error","error":"Not logged in"}}`); !ok || !IsKind(err, ErrNotConfigured) {
		t.Fatal(ok, err)
	}
	if ok, err := claudeInitAnswer(`{"type":"control_response","response":{"request_id":"sayso-init","subtype":"error","error":"bad"}}`); !ok || !IsKind(err, ErrCLI) || !strings.Contains(err.Error(), "bad") {
		t.Fatal(ok, err)
	}
	if isClaudeResultLine(`{"type":"assistant"}`) || !isClaudeResultLine(`{"type":"result","subtype":"success"}`) {
		t.Fatal("result line")
	}
}

func TestExpandDir(t *testing.T) {
	home, _ := os.UserHomeDir()
	if got := expandDir(" ~/.claude-work "); got != filepath.Join(home, ".claude-work") {
		t.Fatal(got)
	}
	if expandDir("~") != home || expandDir("/a/b") != "/a/b" {
		t.Fatal("expand")
	}
}

// fakeClaude writes a shell script named claude that plays the stream-JSON
// protocol with a canned reply. It records its arguments, environment, folder,
// and stdin lines in dir, and appends a line to dir/starts for each start.
func fakeClaude(t *testing.T, body string) (program, dir string) {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("the fake claude is a shell script")
	}
	dir = t.TempDir()
	program = filepath.Join(dir, "claude")
	script := "#!/bin/sh\nD='" + dir + "'\n" + `printf '%s\n' "$@" > "$D/args"
env > "$D/env"
pwd > "$D/pwd"
echo x >> "$D/starts"
` + body
	if err := os.WriteFile(program, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(StopSpares)
	return program, dir
}

const claudeHappy = `IFS= read -r init
printf '%s\n' "$init" > "$D/init"
echo '{"type":"system","subtype":"init"}'
echo '{"type":"control_response","response":{"request_id":"sayso-init","subtype":"success"}}'
IFS= read -r user
printf '%s\n' "$user" > "$D/user"
echo '{"type":"assistant","message":{}}'
echo '{"type":"result","subtype":"success","is_error":false,"duration_ms":900,"num_turns":2,"usage":{"input_tokens":10,"output_tokens":5},"structured_output":{"text":"Hello there."}}'
IFS= read -r rest
`

func readFile(t *testing.T, path string) string {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

func TestClaudeCLIEnhancesWithAFakeTool(t *testing.T) {
	program, dir := fakeClaude(t, claudeHappy)
	cfgDir := t.TempDir()
	c := NewClaudeCLI("claude-code", program, "sonnet").WithConfigDir(cfgDir).WithSpare(false)
	resp, err := c.Enhance(Request{SystemPrompt: "Clean it up.", Transcript: "um hello there", Timeout: 5 * time.Second})
	if err != nil {
		t.Fatal(err)
	}
	if resp.Text != "Hello there." || resp.ProviderID != "claude-code" || resp.Model != "sonnet" || resp.Mode != "claude_cli" {
		t.Fatalf("%+v", resp)
	}
	if resp.Report == nil || *resp.Report.RunMs != 900 || *resp.Report.Turns != 2 || resp.Report.ThinkingTokens != nil {
		t.Fatalf("%+v", resp.Report)
	}
	args := strings.Split(strings.TrimSpace(readFile(t, filepath.Join(dir, "args"))), "\n")
	if !slices.Contains(args, "--await-initialize") || !slices.Contains(args, "sonnet") || slices.Contains(args, "um hello there") {
		t.Fatal(args)
	}
	env := readFile(t, filepath.Join(dir, "env"))
	if !strings.Contains(env, "MAX_THINKING_TOKENS=0\n") || !strings.Contains(env, "CLAUDE_CONFIG_DIR="+cfgDir+"\n") {
		t.Fatal("env misses the thinking limit or the config folder")
	}
	if !strings.Contains(readFile(t, filepath.Join(dir, "init")), `"systemPrompt":["Clean it up."]`) {
		t.Fatal(readFile(t, filepath.Join(dir, "init")))
	}
	if !strings.Contains(readFile(t, filepath.Join(dir, "user")), `um hello there`) {
		t.Fatal(readFile(t, filepath.Join(dir, "user")))
	}
	// The tool ran in an empty folder, which is gone afterwards.
	pwd := strings.TrimSpace(readFile(t, filepath.Join(dir, "pwd")))
	if !strings.Contains(filepath.Base(pwd), "sayso-claude-") {
		t.Fatal(pwd)
	}
	if _, err := os.Stat(pwd); !os.IsNotExist(err) {
		t.Fatal("the work folder was not removed", err)
	}
	// A style model wins over the provider's model.
	resp, err = c.Enhance(Request{SystemPrompt: "s", Transcript: "t", Model: "opus", Timeout: 5 * time.Second})
	if err != nil || resp.Model != "opus" || !strings.Contains(readFile(t, filepath.Join(dir, "args")), "opus\n") {
		t.Fatal(resp, err)
	}
}

func TestClaudeCLIErrors(t *testing.T) {
	cases := []struct {
		name  string
		body  string
		check func(error) bool
	}{
		{"non-zero exit", "echo 'something broke' >&2\nexit 3\n", func(e error) bool {
			return IsKind(e, ErrCLI) && strings.Contains(e.Error(), "claude stopped before it answered: something broke")
		}},
		{"not signed in on stderr", "echo 'Invalid API key · Please run /login' >&2\nexit 1\n", func(e error) bool {
			return IsKind(e, ErrNotConfigured) && strings.Contains(e.Error(), "not signed in")
		}},
		{"init refused", `IFS= read -r init
echo '{"type":"control_response","response":{"request_id":"sayso-init","subtype":"error","error":"Not logged in"}}'
IFS= read -r rest
`, func(e error) bool { return IsKind(e, ErrNotConfigured) }},
		{"error result", `IFS= read -r init
echo '{"type":"control_response","response":{"request_id":"sayso-init","subtype":"success"}}'
IFS= read -r user
echo '{"type":"result","subtype":"success","is_error":true,"result":"overloaded"}'
`, func(e error) bool { return IsKind(e, ErrCLI) && strings.Contains(e.Error(), "overloaded") }},
		{"prose instead of the schema", `IFS= read -r init
echo '{"type":"control_response","response":{"request_id":"sayso-init","subtype":"success"}}'
IFS= read -r user
echo '{"type":"result","subtype":"success","is_error":false,"result":"Hello there."}'
`, func(e error) bool { return IsKind(e, ErrInvalidOutput) }},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			program, _ := fakeClaude(t, tc.body)
			_, err := NewClaudeCLI("c", program, "haiku").WithSpare(false).Enhance(Request{SystemPrompt: "s", Transcript: "t", Timeout: 5 * time.Second})
			if !tc.check(err) {
				t.Fatal(err)
			}
		})
	}
}

func TestClaudeCLITimesOut(t *testing.T) {
	program, _ := fakeClaude(t, "exec sleep 30\n")
	started := time.Now()
	_, err := NewClaudeCLI("c", program, "haiku").WithSpare(false).Enhance(Request{SystemPrompt: "s", Transcript: "t", Timeout: 300 * time.Millisecond})
	if !IsKind(err, ErrTimeout) || err.Error() != "timed out after 300 ms" {
		t.Fatal(err)
	}
	if time.Since(started) > 3*time.Second {
		t.Fatal("the timeout did not stop the request", time.Since(started))
	}
	// A zero request timeout uses the default.
	c := NewClaudeCLI("c", program, "haiku").WithSpare(false)
	c.DefaultTimeout = 200 * time.Millisecond
	if _, err := c.Enhance(Request{SystemPrompt: "s", Transcript: "t"}); !IsKind(err, ErrTimeout) || err.(*Error).Ms != 200 {
		t.Fatal(err)
	}
}

func TestClaudeCLIMissingBinary(t *testing.T) {
	_, err := NewClaudeCLI("c", filepath.Join(t.TempDir(), "nowhere", "claude"), "haiku").WithSpare(false).Enhance(Request{SystemPrompt: "s", Transcript: "t", Timeout: time.Second})
	if !IsKind(err, ErrNotConfigured) || !strings.Contains(err.Error(), "could not start") {
		t.Fatal(err)
	}
	c := &ClaudeCLI{id: "c", model: "haiku", DefaultTimeout: time.Second, thinking: "low"}
	if _, err := c.Enhance(Request{}); !IsKind(err, ErrNotConfigured) || !strings.Contains(err.Error(), "was not found") {
		t.Fatal(err)
	}
}

func TestClaudeCLIUsesASpare(t *testing.T) {
	program, dir := fakeClaude(t, claudeHappy)
	starts := func(want int) {
		t.Helper()
		got := 0
		for deadline := time.Now().Add(5 * time.Second); time.Now().Before(deadline); time.Sleep(10 * time.Millisecond) {
			b, _ := os.ReadFile(filepath.Join(dir, "starts"))
			if got = strings.Count(string(b), "x"); got == want {
				return
			}
		}
		t.Fatalf("%d starts, want %d", got, want)
	}
	waitSpare := func(l claudeLaunch) {
		t.Helper()
		for deadline := time.Now().Add(5 * time.Second); time.Now().Before(deadline); time.Sleep(10 * time.Millisecond) {
			spares.Lock()
			_, ok := spares.m[l]
			spares.Unlock()
			if ok {
				return
			}
		}
		t.Fatal("no spare started")
	}
	p := config.Provider{ID: "claude-code", Kind: config.KindClaudeCLI, Path: &program, Model: ptrTo("sonnet")}
	Prepare(p)
	l := claudeLaunch{program: program, model: "sonnet", thinking: "low"}
	waitSpare(l)
	starts(1)
	resp, err := BuildEnhancer(p, providers.NewMemoryStore(), config.AI{HTTPTimeoutMs: 4000, CLITimeoutMs: 8000}, nil).Enhance(Request{SystemPrompt: "s", Transcript: "t"})
	if err != nil || resp.Text != "Hello there." {
		t.Fatal(resp, err)
	}
	// The request took the spare and left a new one.
	waitSpare(l)
	starts(2)
	StopSpares()
	spares.Lock()
	n := len(spares.m)
	spares.Unlock()
	if n != 0 {
		t.Fatal(n)
	}
	Prepare(config.Provider{ID: "o", Kind: config.KindOpenAICompatible})
}

func ptrTo(s string) *string { return &s }

func TestBuildEnhancerClaudeCLI(t *testing.T) {
	path := "/nowhere/claude"
	cfgDir := "~/.claude-work"
	p := config.Provider{ID: "claude-code", Kind: config.KindClaudeCLI, Path: &path, Model: ptrTo("sonnet"), ConfigDir: &cfgDir, Thinking: ptrTo("medium")}
	e, ok := BuildEnhancer(p, providers.NewMemoryStore(), config.AI{HTTPTimeoutMs: 4000, CLITimeoutMs: 8000}, nil).(*ClaudeCLI)
	if !ok {
		t.Fatal("not a ClaudeCLI")
	}
	if e.DefaultTimeout != 8*time.Second || e.program != path || e.model != "sonnet" || e.thinking != "medium" || e.configDir != cfgDir || !e.IsCloud() || e.ID() != "claude-code" {
		t.Fatalf("%+v", e)
	}
	home, _ := os.UserHomeDir()
	l, _ := e.launch("sonnet")
	if !slices.Equal(l.env(), []string{"CLAUDE_CONFIG_DIR=" + filepath.Join(home, ".claude-work")}) {
		t.Fatal(l.env())
	}
	// No model in the provider uses haiku, as config.toml does.
	e = BuildEnhancer(config.Provider{ID: "c", Kind: config.KindClaudeCLI, Path: &path}, providers.NewMemoryStore(), config.AI{CLITimeoutMs: 8000}, nil).(*ClaudeCLI)
	if e.model != "haiku" || e.thinking != "low" {
		t.Fatalf("%+v", e)
	}
}

func TestDetectClaudeCLI(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("unix permissions")
	}
	a, b := t.TempDir(), t.TempDir()
	if err := os.WriteFile(filepath.Join(a, "claude"), nil, 0o644); err != nil { // not executable: skipped
		t.Fatal(err)
	}
	if err := os.Mkdir(filepath.Join(a, "sub"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(b, "claude"), nil, 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("HOME", t.TempDir())
	t.Setenv("PATH", a+string(os.PathListSeparator)+b)
	if got := DetectClaudeCLI(); got != filepath.Join(b, "claude") {
		t.Fatal(got)
	}
	if NewClaudeCLI("c", "", "haiku").program != filepath.Join(b, "claude") {
		t.Fatal("NewClaudeCLI did not search")
	}
	if findExecutable("sub", []string{a}) != "" {
		t.Fatal("a directory is not an executable")
	}
}

// The whole text step with Chris's setup: the rambling style on a Claude CLI
// provider with the CLI timeout.
func TestPipelineRunsTheClaudeCLI(t *testing.T) {
	program, dir := fakeClaude(t, claudeHappy)
	lib := LoadStyles(t.TempDir())
	style, ok := lib.Get("rambling")
	if !ok || !style.UsesAI() {
		t.Fatal("no rambling style")
	}
	p := config.Provider{ID: "claude-code", Kind: config.KindClaudeCLI, Path: &program, Model: ptrTo("sonnet")}
	pipe := &TextPipeline{Replacer: NewReplacer(nil), Style: style, BasePrompt: lib.BasePrompt(), ProviderKind: string(p.Kind),
		Enhancer: NewClaudeCLI(p.ID, program, "sonnet").WithSpare(false), Timeout: 8 * time.Second}
	out := pipe.Run("so um hello there I guess")
	if out.EnhanceError != nil || out.Text != "Hello there." || out.Enhance.ProviderID != "claude-code" || out.Timings.TimeoutMs != 8000 {
		t.Fatalf("%+v %v", out, out.EnhanceError)
	}
	if !strings.Contains(readFile(t, filepath.Join(dir, "init")), "systemPrompt") {
		t.Fatal("no system prompt")
	}
}
