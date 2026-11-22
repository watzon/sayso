package config

import (
	"fmt"
	"log/slog"
	"slices"
	"strings"
)

// DefaultCLIThinking is the thinking level a CLI provider gets when the user chose none.
const DefaultCLIThinking = "low"

var thinkingOrder = []string{"none", "minimal", "low", "medium", "high", "xhigh", "max"}

// SortThinking orders levels from the least thinking to the most, without
// duplicates. Unknown names sort after the known ones.
func SortThinking(levels []string) []string {
	rank := func(l string) int {
		if i := slices.Index(thinkingOrder, l); i >= 0 {
			return i
		}
		return len(thinkingOrder)
	}
	out := append([]string(nil), levels...)
	slices.SortStableFunc(out, func(a, b string) int { return rank(a) - rank(b) })
	return slices.Compact(out)
}

// ThinkingLabel is the UI name of a level.
func ThinkingLabel(level string) string {
	if level == "xhigh" {
		return "Extra high"
	}
	if level == "" {
		return ""
	}
	r := []rune(level)
	return strings.ToUpper(string(r[0])) + string(r[1:])
}

// ProviderKind is the kind of an enhancement provider, as config.toml names it.
type ProviderKind string

const (
	KindOpenAICompatible  ProviderKind = "open_ai_compatible"
	KindClaudeCLI         ProviderKind = "claude_cli"
	KindChatGPT           ProviderKind = "chat_gpt"
	KindACP               ProviderKind = "acp"
	KindAppleIntelligence ProviderKind = "apple_intelligence"
	KindCleanupModel      ProviderKind = "cleanup_model"
)

// RegistryRef is an agent of the ACP registry.
type RegistryRef struct{ ID, Version string }

// Provider is an enhancement provider. Only the fields of its Kind are used.
type Provider struct {
	ID, Name string
	Kind     ProviderKind

	// OpenAICompatible: OpenRouter, OpenAI, Ollama, LM Studio, llama.cpp server.
	BaseURL           string
	APIKeyAccount     *string // secret store account; the key is never in the file
	ZeroDataRetention bool    // OpenRouter only

	// Model: a string for OpenAICompatible ("" until chosen), ClaudeCLI
	// (default "haiku"), and CleanupModel (the catalog id); optional for
	// ChatGPT and ACP (nil uses the provider's default).
	Model *string
	// Thinking: OpenAICompatible, ClaudeCLI, ChatGPT. nil sends no level
	// (OpenAICompatible) or DefaultCLIThinking.
	Thinking *string

	// ClaudeCLI.
	Path      *string
	ConfigDir *string // CLAUDE_CONFIG_DIR

	// ACP: an agent speaking the Agent Client Protocol on stdio.
	Command     string
	Args        []string
	Env         map[string]string
	SecretEnv   []string // values in the secret store under acp.<id>.<NAME>
	Registry    *RegistryRef
	KeepRunning bool // stop after 10 minutes without use when true
}

func strp(s string) *string { return &s }

// ModelName is the chosen model, or "" when none is chosen or the provider
// picks its own default.
func (p Provider) ModelName() string {
	if p.Kind == KindAppleIntelligence || p.Model == nil {
		return ""
	}
	if strings.TrimSpace(*p.Model) == "" {
		return ""
	}
	return *p.Model
}

// ConfigDirSet is the login folder of a CLI provider, or "" for the default.
func (p Provider) ConfigDirSet() string {
	if p.Kind == KindClaudeCLI && p.ConfigDir != nil && strings.TrimSpace(*p.ConfigDir) != "" {
		return *p.ConfigDir
	}
	return ""
}

func hasThinking(k ProviderKind) bool {
	return k == KindOpenAICompatible || k == KindClaudeCLI || k == KindChatGPT
}

// ThinkingChosen is the level the user chose, or "".
func (p Provider) ThinkingChosen() string {
	if !hasThinking(p.Kind) || p.Thinking == nil || strings.TrimSpace(*p.Thinking) == "" {
		return ""
	}
	return *p.Thinking
}

// ThinkingSent is the level that goes to the provider; "" sends none.
func (p Provider) ThinkingSent() string {
	if t := p.ThinkingChosen(); t != "" {
		return t
	}
	if p.Kind == KindClaudeCLI || p.Kind == KindChatGPT {
		return DefaultCLIThinking
	}
	return ""
}

// SetThinking sets the level for a kind that has levels.
func (p *Provider) SetThinking(level *string) {
	if hasThinking(p.Kind) {
		p.Thinking = level
	}
}

// FitThinking makes the level fit a model offering `levels` (least to most):
// an unknown level becomes the lowest; a model without levels gets none.
func (p *Provider) FitThinking(levels []string) {
	sent := p.ThinkingSent()
	fits := sent != "" && slices.Contains(levels, sent)
	if len(levels) == 0 {
		p.SetThinking(nil)
	} else if !fits {
		p.SetThinking(strp(levels[0]))
	}
}

// SetModel sets the model where the kind lets the user choose one.
func (p *Provider) SetModel(id string) {
	switch p.Kind {
	case KindOpenAICompatible, KindClaudeCLI, KindChatGPT, KindACP:
		p.Model = strp(id)
	}
}

// NeedsModel: an HTTP server has no default model.
func (p Provider) NeedsModel() bool { return p.Kind == KindOpenAICompatible && p.ModelName() == "" }

// IsCloud is true when the provider sends text off this computer.
func (p Provider) IsCloud() bool {
	switch p.Kind {
	case KindOpenAICompatible:
		return !IsLocalURL(p.BaseURL)
	case KindClaudeCLI, KindChatGPT, KindACP:
		// An ACP agent may run a local model, but Sayso cannot tell.
		return true
	}
	return false
}

// IsLocalURL is true for localhost, loopback, 0.0.0.0, and .local hosts.
func IsLocalURL(url string) bool {
	rest := url
	if _, after, ok := strings.Cut(url, "://"); ok {
		rest = after
	}
	host := rest
	if i := strings.IndexAny(rest, "/:"); i >= 0 {
		host = rest[:i]
	}
	switch host {
	case "localhost", "127.0.0.1", "::1", "[::1]", "0.0.0.0":
		return true
	}
	return strings.HasSuffix(host, ".local")
}

// AI is the [ai] section. In memory Providers holds every provider, also the
// cleanup models, ChatGPT accounts, and ACP agents, which the file keeps in
// their own lists so an older Sayso that reads [ai] in one piece still works.
type AI struct {
	// Enabled is off until the user sets up a provider; off means every style acts like Raw.
	Enabled         bool
	ActiveStyle     string
	DefaultProvider *string
	Providers       []Provider
	// UnknownProviders are entries of a kind this version does not know. They
	// stay in the file and never run.
	UnknownProviders []map[string]any
	HTTPTimeoutMs    uint64
	CLITimeoutMs     uint64
	StyleRules       StyleRules
}

func defaultAI() AI {
	return AI{ActiveStyle: DefaultStyleID, Providers: []Provider{}, HTTPTimeoutMs: 4000, CLITimeoutMs: 8000, StyleRules: StyleRules{Rules: []StyleRule{}}}
}

// Provider finds a provider by id, or the default provider when id is "".
func (a AI) Provider(id string) (Provider, bool) {
	if id == "" {
		if a.DefaultProvider == nil {
			return Provider{}, false
		}
		id = *a.DefaultProvider
	}
	for _, p := range a.Providers {
		if p.ID == id {
			return p, true
		}
	}
	return Provider{}, false
}

// migrateProvider turns a codex_cli entry into a chat_gpt entry with the same
// model and thinking level; the user signs in once more.
func migrateProvider(m map[string]any) map[string]any {
	if m["kind"] != "codex_cli" {
		return m
	}
	out := make(map[string]any, len(m))
	for k, v := range m {
		out[k] = v
	}
	out["kind"] = "chat_gpt"
	delete(out, "path")
	delete(out, "config_dir")
	if out["name"] == "Codex CLI" {
		out["name"] = "ChatGPT"
	}
	return out
}

var providerKindNames = []string{"open_ai_compatible", "claude_cli", "chat_gpt", "acp", "apple_intelligence", "cleanup_model"}

// readProvider reads one [[ai.providers]] entry; an error makes it unknown.
func readProvider(m map[string]any) (Provider, error) {
	var p Provider
	r := newReader(m)
	for _, key := range []string{"id", "name"} {
		if _, ok := m[key]; !ok {
			return p, fmt.Errorf("missing field `%s`", key)
		}
	}
	if _, ok := m["kind"]; !ok {
		return p, fmt.Errorf("missing field `kind`")
	}
	r.str("id", &p.ID)
	r.str("name", &p.Name)
	r.enum("kind", providerKindNames, func(i int) { p.Kind = ProviderKind(providerKindNames[i]) })
	if r.err != nil {
		return p, r.err
	}
	switch p.Kind {
	case KindOpenAICompatible:
		if _, ok := m["base_url"]; !ok {
			return p, fmt.Errorf("missing field `base_url`")
		}
		r.str("base_url", &p.BaseURL)
		model := ""
		r.str("model", &model)
		p.Model = &model
		r.optStr("api_key_account", &p.APIKeyAccount)
		r.boolean("zero_data_retention", &p.ZeroDataRetention)
		r.optStr("thinking", &p.Thinking)
	case KindClaudeCLI:
		r.optStr("path", &p.Path)
		model := "haiku"
		r.str("model", &model)
		p.Model = &model
		r.optStr("config_dir", &p.ConfigDir)
		r.optStr("thinking", &p.Thinking)
	case KindChatGPT:
		r.optStr("model", &p.Model)
		r.optStr("thinking", &p.Thinking)
	case KindACP:
		if _, ok := m["command"]; !ok {
			return p, fmt.Errorf("missing field `command`")
		}
		readACPFields(r, &p)
	case KindCleanupModel:
		if _, ok := m["model"]; !ok {
			return p, fmt.Errorf("missing field `model`")
		}
		model := ""
		r.str("model", &model)
		p.Model = &model
	}
	return p, r.err
}

func readACPFields(r *reader, p *Provider) {
	p.KeepRunning = true
	r.str("command", &p.Command)
	r.strings("args", &p.Args)
	r.stringMap("env", &p.Env)
	r.strings("secret_env", &p.SecretEnv)
	if v, ok := r.get("registry"); ok {
		rr := newReader(v)
		var ref RegistryRef
		if _, ok := rr.m["id"]; !ok && rr.err == nil {
			rr.err = fmt.Errorf("missing field `id`")
		}
		if _, ok := rr.m["version"]; !ok && rr.err == nil {
			rr.err = fmt.Errorf("missing field `version`")
		}
		rr.str("id", &ref.ID)
		rr.str("version", &ref.Version)
		if rr.err != nil {
			r.fail("registry", rr.err)
		} else {
			p.Registry = &ref
		}
	}
	r.optStr("model", &p.Model)
	r.boolean("keep_running", &p.KeepRunning)
}

func readAI(v any) (AI, error) {
	a := defaultAI()
	r := newReader(v)
	r.boolean("enabled", &a.Enabled)
	r.str("active_style", &a.ActiveStyle)
	r.optStr("default_provider", &a.DefaultProvider)
	r.u64("http_timeout_ms", &a.HTTPTimeoutMs)
	r.u64("cli_timeout_ms", &a.CLITimeoutMs)
	if sr, ok := r.table("style_rules"); ok {
		rules, err := readStyleRules(sr)
		if err != nil {
			r.fail("style_rules", err)
		} else {
			a.StyleRules = rules
		}
	}
	for _, e := range r.tables("providers") {
		m, ok := e.(map[string]any)
		if !ok {
			r.fail("providers", fmt.Errorf("invalid type: %s, expected a table", describe(e)))
			break
		}
		m = migrateProvider(m)
		p, err := readProvider(m)
		if err != nil {
			id, _ := m["id"].(string)
			kind, _ := m["kind"].(string)
			slog.Warn("keeping a provider as it is; it will not run", "id", id, "kind", kind, "err", err)
			a.UnknownProviders = append(a.UnknownProviders, m)
			continue
		}
		a.Providers = append(a.Providers, p)
	}
	for _, e := range r.tables("cleanup_models") {
		er := newReader(e)
		var id, name, model string
		er.str("id", &id)
		er.str("name", &name)
		er.str("model", &model)
		if er.err != nil {
			r.fail("cleanup_models", er.err)
			break
		}
		if id == "" || model == "" {
			slog.Warn("skipped a cleanup model with an empty id or model")
			continue
		}
		a.Providers = append(a.Providers, Provider{ID: id, Name: name, Kind: KindCleanupModel, Model: strp(model)})
	}
	for _, e := range r.tables("chatgpt_accounts") {
		er := newReader(e)
		p := Provider{Kind: KindChatGPT}
		er.str("id", &p.ID)
		er.str("name", &p.Name)
		er.optStr("model", &p.Model)
		er.optStr("thinking", &p.Thinking)
		if er.err != nil {
			r.fail("chatgpt_accounts", er.err)
			break
		}
		if p.ID == "" {
			slog.Warn("skipped a ChatGPT provider with an empty id")
			continue
		}
		a.Providers = append(a.Providers, p)
	}
	for _, e := range r.tables("agents") {
		er := newReader(e)
		p := Provider{Kind: KindACP}
		er.str("id", &p.ID)
		er.str("name", &p.Name)
		readACPFields(er, &p)
		if er.err != nil {
			r.fail("agents", er.err)
			break
		}
		if p.ID == "" || strings.TrimSpace(p.Command) == "" {
			slog.Warn("skipped an ACP agent with an empty id or command")
			continue
		}
		a.Providers = append(a.Providers, p)
	}
	if r.err != nil {
		return defaultAI(), r.err
	}
	return a, nil
}

func (a AI) toTable() *Table {
	t := newTable().Set("enabled", a.Enabled).Set("active_style", a.ActiveStyle)
	if a.DefaultProvider != nil {
		t.Set("default_provider", *a.DefaultProvider)
	}
	providers := []any{}
	var cleanup, chatgpt, agents []any
	for _, p := range a.Providers {
		switch p.Kind {
		case KindCleanupModel:
			cleanup = append(cleanup, newTable().Set("id", p.ID).Set("name", p.Name).Set("model", deref(p.Model)))
		case KindChatGPT:
			e := newTable().Set("id", p.ID).Set("name", p.Name)
			if p.Model != nil {
				e.Set("model", *p.Model)
			}
			if p.Thinking != nil {
				e.Set("thinking", *p.Thinking)
			}
			chatgpt = append(chatgpt, e)
		case KindACP:
			e := newTable().Set("id", p.ID).Set("name", p.Name).Set("command", p.Command)
			if len(p.Args) > 0 {
				e.Set("args", stringsToAny(p.Args))
			}
			if len(p.SecretEnv) > 0 {
				e.Set("secret_env", stringsToAny(p.SecretEnv))
			}
			if p.Model != nil {
				e.Set("model", *p.Model)
			}
			e.Set("keep_running", p.KeepRunning)
			if len(p.Env) > 0 {
				env := newTable()
				for _, k := range sortedKeys(p.Env) {
					env.Set(k, p.Env[k])
				}
				e.Set("env", env)
			}
			if p.Registry != nil {
				e.Set("registry", newTable().Set("id", p.Registry.ID).Set("version", p.Registry.Version))
			}
			agents = append(agents, e)
		default:
			providers = append(providers, providerTable(p))
		}
	}
	for _, u := range a.UnknownProviders {
		providers = append(providers, tableFromMap(u))
	}
	t.Set("providers", providers)
	t.Set("http_timeout_ms", a.HTTPTimeoutMs)
	t.Set("cli_timeout_ms", a.CLITimeoutMs)
	t.Set("style_rules", a.StyleRules.toTable())
	if len(cleanup) > 0 {
		t.Set("cleanup_models", cleanup)
	}
	if len(chatgpt) > 0 {
		t.Set("chatgpt_accounts", chatgpt)
	}
	if len(agents) > 0 {
		t.Set("agents", agents)
	}
	return t
}

func deref(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

func providerTable(p Provider) *Table {
	t := newTable().Set("id", p.ID).Set("name", p.Name).Set("kind", string(p.Kind))
	switch p.Kind {
	case KindOpenAICompatible:
		t.Set("base_url", p.BaseURL).Set("model", deref(p.Model))
		if p.APIKeyAccount != nil {
			t.Set("api_key_account", *p.APIKeyAccount)
		}
		t.Set("zero_data_retention", p.ZeroDataRetention)
		if p.Thinking != nil {
			t.Set("thinking", *p.Thinking)
		}
	case KindClaudeCLI:
		if p.Path != nil {
			t.Set("path", *p.Path)
		}
		t.Set("model", deref(p.Model))
		if p.ConfigDir != nil {
			t.Set("config_dir", *p.ConfigDir)
		}
		if p.Thinking != nil {
			t.Set("thinking", *p.Thinking)
		}
	}
	return t
}
