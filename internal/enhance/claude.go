package enhance

// Claude Code provider: the user's own claude command, driven over the
// stream-JSON protocol that the Claude Agent SDK uses (sayso-enhance claude.rs).
//
// Sayso starts claude with --input-format stream-json and --await-initialize,
// so the process waits for an initialize request. That request carries the
// system prompt and the JSON schema of the answer. The transcript then goes in
// as one user message, and the result message carries structured_output. One
// process serves one request. The text stays out of the process list.
//
// The process can start before the transcript exists, so Sayso keeps one
// started process ready for each binary, model, thinking level, and config
// folder (a spare). A request takes the spare and starts the next one. A spare
// that waits for SpareLifetime is stopped. Prepare starts one when a dictation
// starts.
//
// Without --strict-mcp-config the CLI loads the user's MCP servers and
// plugins, which costs seconds and tokens and can turn the answer into prose.
// The CLI uses the user's own login: Sayso never reads Claude credentials.

import (
	"bytes"
	"encoding/json"
	"errors"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/store"
)

// SpareLifetime is how long a spare process waits for a request before Sayso stops it.
const SpareLifetime = 600 * time.Second

// claudeInitID is the request id of the initialize request; the answer carries it back.
const claudeInitID = "sayso-init"

// ClaudeCLI enhances with the claude command.
type ClaudeCLI struct {
	id string
	// program is "" when no binary was found; Enhance then fails with NotConfigured.
	program        string
	model          string
	DefaultTimeout time.Duration
	configDir      string
	thinking       string
	// spare keeps a started process ready for the next request.
	spare bool
}

// NewClaudeCLI makes the enhancer. path "" searches for the binary.
func NewClaudeCLI(id, path, model string) *ClaudeCLI {
	program := strings.TrimSpace(path)
	if program == "" {
		program = DetectClaudeCLI()
	}
	return &ClaudeCLI{id: id, program: program, model: model, DefaultTimeout: 8 * time.Second, thinking: config.DefaultCLIThinking, spare: true}
}

// WithConfigDir sets the folder with the login of the tool (CLAUDE_CONFIG_DIR). A leading ~/ is the home folder.
func (c *ClaudeCLI) WithConfigDir(dir string) *ClaudeCLI { c.configDir = dir; return c }

// WithThinking sets the --effort level; "" keeps the default, low.
func (c *ClaudeCLI) WithThinking(level string) *ClaudeCLI {
	if level == "" {
		level = config.DefaultCLIThinking
	}
	c.thinking = level
	return c
}

// WithSpare sets whether a request leaves a started process ready for the next one. On by default.
func (c *ClaudeCLI) WithSpare(spare bool) *ClaudeCLI { c.spare = spare; return c }

func (c *ClaudeCLI) ID() string { return c.id }

// IsCloud: the CLI sends text to Anthropic.
func (c *ClaudeCLI) IsCloud() bool { return true }

func (c *ClaudeCLI) launch(model string) (claudeLaunch, bool) {
	if c.program == "" {
		return claudeLaunch{}, false
	}
	return claudeLaunch{program: c.program, model: model, thinking: c.thinking, configDir: c.configDir}, true
}

// claudeLaunch is what a process is started with. Requests with the same launch can use the same spare.
type claudeLaunch struct {
	program, model, thinking, configDir string
}

// args are the command line. The system prompt, the schema, and the
// transcript come later, on stdin.
func (l claudeLaunch) args() []string {
	return []string{
		"--input-format", "stream-json",
		"--output-format", "stream-json",
		"--verbose",
		"--await-initialize",
		"--model", l.model,
		"--effort", l.thinking,
		"--tools", "",
		"--strict-mcp-config",
		"--no-session-persistence",
		"--disable-slash-commands",
		"--setting-sources", "",
	}
}

// env is the extra environment of the process.
func (l claudeLaunch) env() []string {
	var envs []string
	// A limit of 0 at a higher level would work against the choice of the user.
	if l.thinking == config.DefaultCLIThinking {
		envs = append(envs, "MAX_THINKING_TOKENS=0")
	}
	if dir := strings.TrimSpace(l.configDir); dir != "" {
		envs = append(envs, "CLAUDE_CONFIG_DIR="+expandDir(dir))
	}
	return envs
}

// start starts the process in an empty folder. timeout is its first deadline.
func (l claudeLaunch) start(timeout time.Duration) (*claudeStarted, error) {
	dir, err := os.MkdirTemp("", "sayso-claude-")
	if err != nil {
		return nil, &Error{Kind: ErrCLI, Message: "no temporary folder: " + err.Error()}
	}
	s, err := startSession(invocation{program: l.program, args: l.args(), envs: l.env(), dir: dir, timeout: timeout})
	if err != nil {
		os.RemoveAll(dir)
		return nil, err
	}
	s.onClose = func() { os.RemoveAll(dir) }
	return &claudeStarted{session: s, at: time.Now()}, nil
}

type claudeStarted struct {
	session *cliSession
	at      time.Time
}

type claudeSpare struct {
	started    *claudeStarted
	generation uint64
}

var spares = struct {
	sync.Mutex
	m   map[claudeLaunch]claudeSpare
	gen uint64
	// filling is the launches whose spare is starting, so two refills do not race.
	filling map[claudeLaunch]bool
}{m: map[claudeLaunch]claudeSpare{}, filling: map[claudeLaunch]bool{}}

// takeSpare takes the spare of a launch, when one is ready and still runs.
func takeSpare(l claudeLaunch) *claudeStarted {
	spares.Lock()
	sp, ok := spares.m[l]
	delete(spares.m, l)
	spares.Unlock()
	if !ok {
		return nil
	}
	if time.Since(sp.started.at) < SpareLifetime && sp.started.session.running() {
		return sp.started
	}
	sp.started.session.close()
	return nil
}

// refill starts a spare for a launch in the background, unless one is there.
// It is stopped when no request takes it within SpareLifetime.
func refill(l claudeLaunch) {
	spares.Lock()
	if _, ok := spares.m[l]; ok || spares.filling[l] {
		spares.Unlock()
		return
	}
	spares.filling[l] = true
	spares.Unlock()
	go func() {
		started, err := l.start(SpareLifetime + time.Minute)
		spares.Lock()
		delete(spares.filling, l)
		if err != nil {
			spares.Unlock()
			slog.Debug("enhance: could not start a spare claude process", "err", err)
			return
		}
		if _, ok := spares.m[l]; ok {
			spares.Unlock()
			started.session.close()
			return
		}
		spares.gen++
		gen := spares.gen
		spares.m[l] = claudeSpare{started: started, generation: gen}
		spares.Unlock()
		time.AfterFunc(SpareLifetime, func() {
			spares.Lock()
			sp, ok := spares.m[l]
			if ok && sp.generation == gen {
				delete(spares.m, l)
			} else {
				ok = false
			}
			spares.Unlock()
			if ok {
				sp.started.session.close()
			}
		})
	}()
}

// Prepare starts what a provider runs before its request comes: a spare
// claude process. Call it when the microphone opens. Does nothing for other
// kinds. Returns at once.
func Prepare(p config.Provider) {
	if p.Kind != config.KindClaudeCLI {
		return
	}
	path := ""
	if p.Path != nil {
		path = *p.Path
	}
	c := NewClaudeCLI(p.ID, path, claudeModel(p)).WithThinking(p.ThinkingSent()).WithConfigDir(p.ConfigDirSet())
	if l, ok := c.launch(c.model); ok {
		refill(l)
	}
}

// StopSpares stops all spare processes: for tests, and when the app quits.
func StopSpares() {
	spares.Lock()
	all := spares.m
	spares.m = map[claudeLaunch]claudeSpare{}
	spares.Unlock()
	for _, sp := range all {
		sp.started.session.close()
	}
}

// claudeModel is the provider's model; config.toml defaults it to haiku.
func claudeModel(p config.Provider) string {
	if m := p.ModelName(); m != "" {
		return m
	}
	return "haiku"
}

// claudeInitRequest carries the system prompt and the schema of the answer.
func claudeInitRequest(systemPrompt string) map[string]any {
	return map[string]any{
		"type":       "control_request",
		"request_id": claudeInitID,
		"request":    map[string]any{"subtype": "initialize", "systemPrompt": []string{systemPrompt}, "jsonSchema": OutputSchema()},
	}
}

// claudeUserRequest carries the transcript.
func claudeUserRequest(transcript string) map[string]any {
	return map[string]any{
		"type":               "user",
		"session_id":         "",
		"message":            map[string]any{"role": "user", "content": []map[string]any{{"type": "text", "text": UserMessage(transcript)}}},
		"parent_tool_use_id": nil,
	}
}

func decodeJSON(text string) (any, error) {
	d := json.NewDecoder(strings.NewReader(text))
	d.UseNumber()
	var v any
	if err := d.Decode(&v); err != nil {
		return nil, err
	}
	if d.More() {
		return nil, errors.New("more than one JSON value")
	}
	return v, nil
}

// claudeInitAnswer reads the answer to initialize: ok is false for other
// lines, err is set when the tool refused it.
func claudeInitAnswer(line string) (ok bool, err error) {
	v, jerr := decodeJSON(line)
	obj, _ := v.(map[string]any)
	if jerr != nil || obj["type"] != "control_response" {
		return false, nil
	}
	resp, _ := obj["response"].(map[string]any)
	if resp["request_id"] != claudeInitID {
		return false, nil
	}
	if resp["subtype"] == "error" {
		text, _ := resp["error"].(string)
		if text == "" {
			text = "no reason given"
		}
		if looksLikeLoginProblem(text) {
			return true, notConfigured("claude is not signed in: " + text)
		}
		return true, &Error{Kind: ErrCLI, Message: "claude refused the request: " + text}
	}
	return true, nil
}

// isClaudeResultLine is true for the result message.
func isClaudeResultLine(line string) bool {
	v, err := decodeJSON(line)
	obj, _ := v.(map[string]any)
	return err == nil && obj["type"] == "result"
}

func encodeLine(v any) string {
	var b bytes.Buffer
	enc := json.NewEncoder(&b)
	enc.SetEscapeHTML(false)
	_ = enc.Encode(v)
	return strings.TrimRight(b.String(), "\n")
}

// runClaude runs one request on a started process.
func runClaude(s *cliSession, r Request) (string, *store.ProviderReport, error) {
	if err := s.send(encodeLine(claudeInitRequest(r.SystemPrompt))); err != nil {
		return "", nil, err
	}
	var initErr error
	if err := s.readUntil(func(line string) bool {
		ok, err := claudeInitAnswer(line)
		initErr = err
		return ok
	}); err != nil {
		return "", nil, err
	}
	if initErr != nil {
		return "", nil, initErr
	}
	if err := s.send(encodeLine(claudeUserRequest(r.Transcript))); err != nil {
		return "", nil, err
	}
	var result string
	if err := s.readUntil(func(line string) bool {
		if isClaudeResultLine(line) {
			result = line
			return true
		}
		return false
	}); err != nil {
		return "", nil, err
	}
	return parseClaudeEnvelope(result)
}

// claudeStoppedEarly: a process that stopped early says why on its error output.
func claudeStoppedEarly(err error, s *cliSession) error {
	e, ok := err.(*Error)
	if !ok || e.Kind != ErrCLI || !strings.Contains(e.Message, "closed its output") {
		return err
	}
	stderr := s.stderrTail()
	if looksLikeLoginProblem(stderr) {
		return notConfigured("claude is not signed in: " + stderr)
	}
	return &Error{Kind: ErrCLI, Message: "claude stopped before it answered: " + stderr}
}

func (c *ClaudeCLI) Enhance(r Request) (Response, error) {
	timeout := r.Timeout
	if timeout == 0 {
		timeout = c.DefaultTimeout
	}
	model := r.Model
	if model == "" {
		model = c.model
	}
	l, ok := c.launch(model)
	if !ok {
		return Response{}, notConfigured("the claude command was not found")
	}
	startedAt := time.Now()
	var started *claudeStarted
	if c.spare {
		started = takeSpare(l)
	}
	warm := started != nil
	if started == nil {
		var err error
		if started, err = l.start(timeout); err != nil {
			return Response{}, err
		}
	}
	defer started.session.close()
	started.session.restartDeadline(max(timeout-time.Since(startedAt), 0), timeout)
	if c.spare {
		refill(l)
	}
	slog.Debug("enhance: claude process ready", "provider", c.id, "spare", warm, "ms", time.Since(startedAt).Milliseconds())
	text, report, err := runClaude(started.session, r)
	if err != nil {
		return Response{}, claudeStoppedEarly(err, started.session)
	}
	return Response{Text: text, ProviderID: c.id, Model: model, ElapsedMs: uint64(time.Since(startedAt).Milliseconds()), Mode: "claude_cli", Report: report}, nil
}

// parseClaudeEnvelope reads the result message and returns the validated
// text, with the numbers of the run. Fails on is_error, on a subtype other
// than success, and on a missing or invalid structured_output.
func parseClaudeEnvelope(stdout string) (string, *store.ProviderReport, error) {
	v, err := decodeJSON(strings.TrimSpace(stdout))
	if err != nil {
		return "", nil, invalidOutput("claude output is not JSON: " + err.Error())
	}
	var env map[string]any
	switch t := v.(type) {
	case []any:
		// Some versions print the whole message list. The result is the last "result" item.
		for i := len(t) - 1; i >= 0; i-- {
			if item, ok := t[i].(map[string]any); ok && item["type"] == "result" {
				env = item
				break
			}
		}
	case map[string]any:
		env = t
	}
	if env == nil {
		return "", nil, invalidOutput("claude output has no result message")
	}
	message, _ := env["result"].(string)
	if isErr, _ := env["is_error"].(bool); isErr {
		if looksLikeLoginProblem(message) {
			return "", nil, notConfigured("claude is not signed in: " + message)
		}
		return "", nil, &Error{Kind: ErrCLI, Message: "claude reported an error: " + message}
	}
	if subtype, ok := env["subtype"].(string); ok && subtype != "success" {
		return "", nil, &Error{Kind: ErrCLI, Message: "claude ended with \"" + subtype + "\": " + message}
	}
	structured, ok := env["structured_output"]
	if !ok || structured == nil {
		return "", nil, invalidOutput("claude returned no structured_output")
	}
	text, err := ParseValue(structured)
	if err != nil {
		return "", nil, err
	}
	return text, claudeReport(env), nil
}

// claudeReport is the numbers of the run. A field the tool leaves out is nil.
func claudeReport(env map[string]any) *store.ProviderReport {
	usage, _ := env["usage"].(map[string]any)
	details, _ := usage["output_tokens_details"].(map[string]any)
	return &store.ProviderReport{
		RunMs:            jsonU64(env["duration_ms"]),
		APIMs:            jsonU64(env["duration_api_ms"]),
		Turns:            jsonU64(env["num_turns"]),
		InputTokens:      jsonU64(usage["input_tokens"]),
		OutputTokens:     jsonU64(usage["output_tokens"]),
		ThinkingTokens:   jsonU64(details["thinking_tokens"]),
		CacheReadTokens:  jsonU64(usage["cache_read_input_tokens"]),
		CacheWriteTokens: jsonU64(usage["cache_creation_input_tokens"]),
	}
}

func jsonU64(v any) *uint64 {
	n, ok := v.(json.Number)
	if !ok {
		return nil
	}
	if strings.ContainsAny(string(n), ".eE-") {
		return nil
	}
	var u uint64
	if err := json.Unmarshal([]byte(n), &u); err != nil {
		return nil
	}
	return &u
}

// expandDir replaces a leading ~ with the home folder.
func expandDir(dir string) string {
	dir = strings.TrimSpace(dir)
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		return dir
	}
	if dir == "~" {
		return home
	}
	if rest, ok := strings.CutPrefix(dir, "~/"); ok {
		return filepath.Join(home, rest)
	}
	if filepath.Separator == '\\' {
		if rest, ok := strings.CutPrefix(dir, `~\`); ok {
			return filepath.Join(home, rest)
		}
	}
	return dir
}

// DetectClaudeCLI is the claude binary, searched in PATH and the usual install
// folders, or "".
func DetectClaudeCLI() string {
	dirs := append(filepath.SplitList(os.Getenv("PATH")), toolDirs()...)
	return findExecutable("claude", dirs)
}
