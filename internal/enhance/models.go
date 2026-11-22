package enhance

// List the models a provider offers, for the model picker
// (sayso-enhance models.rs).
//
//   - OpenAI-compatible servers: GET {base_url}/models.
//   - Claude Code has no "list models" command, but its stream-JSON control
//     protocol answers an initialize request with the model list. No prompt
//     is sent, so this costs nothing.
//   - ChatGPT: the model list of the Codex backend (chatgpt.go).
//   - Apple Intelligence: the one model that the engine reports.
//
// All calls block: run them off the UI thread.

import (
	"os"
	"strings"
	"time"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/providers"
)

// claudeModelsID is the request id of the initialize request that lists
// Claude's models. The answer carries it back.
const claudeModelsID = "sayso-models"

// ListProviderModels lists the models p offers. timeout bounds the whole call.
//
// HTTP lists are sorted by id. CLI lists keep the order of the tool. There
// are never two choices with the same id. Claude's "default" entry is left
// out: it means whatever Claude Code defaults to, which is usually its
// slowest model. A cleanup model has no list: the catalog fixes its model.
func ListProviderModels(p config.Provider, secrets providers.SecretStore, engine LanguageModel, timeout time.Duration) ([]ModelChoice, error) {
	switch p.Kind {
	case config.KindOpenAICompatible:
		// A missing key is not an error here: Ollama and LM Studio list models without one.
		key := ""
		if p.APIKeyAccount != nil && secrets != nil {
			key = secrets.Get(*p.APIKeyAccount)
		}
		return ListModels(p.BaseURL, key, timeout)
	case config.KindChatGPT:
		if secrets == nil {
			return nil, notConfigured("not signed in to ChatGPT")
		}
		return ChatGPTListModels(secrets, p.ID, timeout)
	case config.KindClaudeCLI:
		program := ""
		if p.Path != nil {
			program = strings.TrimSpace(*p.Path)
		}
		if program == "" {
			program = DetectClaudeCLI()
		}
		if program == "" {
			return nil, notConfigured("the claude command was not found")
		}
		return listClaudeModels(program, p.ConfigDirSet(), timeout)
	case config.KindAppleIntelligence:
		// macOS chooses the model, so the list has one entry.
		if engine == nil {
			return nil, notConfigured("the engine does not run")
		}
		name, err := engine.ModelName()
		if err != nil {
			return nil, err
		}
		return []ModelChoice{choice(name, "", i18n.T("enhance.runs_on_this_mac"))}, nil
	case config.KindCleanupModel:
		return []ModelChoice{}, nil
	case config.KindACP:
		return nil, notConfigured("ACP agents are not available in this build")
	}
	return nil, notConfigured("unknown provider kind " + string(p.Kind))
}

// claudeModelsArgs are the command line of the listing run: the same
// hygiene flags as a request, and no model, so nothing is loaded.
var claudeModelsArgs = []string{
	"-p",
	"--input-format", "stream-json",
	"--output-format", "stream-json",
	"--verbose",
	"--tools", "",
	"--strict-mcp-config",
	"--setting-sources", "",
	"--no-session-persistence",
	"--disable-slash-commands",
}

// listClaudeModels asks claude for its models with an initialize request.
func listClaudeModels(program, configDir string, timeout time.Duration) ([]ModelChoice, error) {
	dir, err := os.MkdirTemp("", "sayso-claude-")
	if err != nil {
		return nil, &Error{Kind: ErrCLI, Message: "no temporary folder: " + err.Error()}
	}
	envs := []string{"MAX_THINKING_TOKENS=0"}
	if d := strings.TrimSpace(configDir); d != "" {
		envs = append(envs, "CLAUDE_CONFIG_DIR="+expandDir(d))
	}
	s, err := startSession(invocation{program: program, args: claudeModelsArgs, envs: envs, dir: dir, timeout: timeout})
	if err != nil {
		os.RemoveAll(dir)
		return nil, err
	}
	s.onClose = func() { os.RemoveAll(dir) }
	// The input stays open until close: Claude Code exits when it closes.
	defer s.close()
	request := map[string]any{
		"type":       "control_request",
		"request_id": claudeModelsID,
		"request":    map[string]any{"subtype": "initialize"},
	}
	if err := s.send(encodeLine(request)); err != nil {
		return nil, err
	}
	var models []ModelChoice
	var answerErr error
	if err := s.readUntil(func(line string) bool {
		list, ok, err := claudeModelsAnswer(line)
		models, answerErr = list, err
		return ok
	}); err != nil {
		return nil, claudeStoppedEarly(err, s)
	}
	return models, answerErr
}

// claudeModelsAnswer reads the answer to the listing request. ok is false
// for any other line.
func claudeModelsAnswer(line string) (models []ModelChoice, ok bool, err error) {
	v, jerr := decodeJSON(line)
	obj, _ := v.(map[string]any)
	if jerr != nil || obj["type"] != "control_response" {
		return nil, false, nil
	}
	resp, _ := obj["response"].(map[string]any)
	if resp["request_id"] != claudeModelsID {
		return nil, false, nil
	}
	models, err = parseClaudeModels(resp)
	return models, true, err
}

// parseClaudeModels reads the response of a control_response message.
func parseClaudeModels(resp map[string]any) ([]ModelChoice, error) {
	if resp["subtype"] == "error" {
		text, _ := resp["error"].(string)
		if text == "" {
			text = "no reason given"
		}
		if looksLikeLoginProblem(text) {
			return nil, notConfigured("claude is not signed in: " + text)
		}
		return nil, &Error{Kind: ErrCLI, Message: "claude could not list its models: " + text}
	}
	inner, _ := resp["response"].(map[string]any)
	list, ok := inner["models"].([]any)
	if !ok {
		return nil, invalidOutput("claude answered without a model list")
	}
	out := []ModelChoice{}
	for _, raw := range list {
		m, _ := raw.(map[string]any)
		id, _ := m["value"].(string)
		if id == "" || id == "default" {
			continue
		}
		name, _ := m["displayName"].(string)
		desc, _ := m["description"].(string)
		c := choice(id, name, desc)
		c.Thinking = levels(m["supportedEffortLevels"])
		out = append(out, c)
	}
	return dedupChoices(out), nil
}

// choice is a ModelChoice. An empty name becomes the id, an empty
// description none.
func choice(id, name, description string) ModelChoice {
	if name == "" {
		name = id
	}
	c := ModelChoice{ID: id, Name: name}
	if description != "" {
		c.Description = &description
	}
	return c
}

// levels are the thinking levels of a JSON list of names, from the least
// thinking to the most. A missing list is a model without levels.
func levels(v any) []string {
	items, _ := v.([]any)
	var names []string
	for _, it := range items {
		if s, ok := it.(string); ok && s != "" {
			names = append(names, s)
		}
	}
	return config.SortThinking(names)
}

// dedupChoices keeps the first choice of each id, in order.
func dedupChoices(in []ModelChoice) []ModelChoice {
	seen := map[string]bool{}
	out := in[:0]
	for _, c := range in {
		if !seen[c.ID] {
			seen[c.ID] = true
			out = append(out, c)
		}
	}
	return out
}
